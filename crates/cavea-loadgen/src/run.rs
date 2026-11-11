//! One run: set up or find the event, connect subscribers, drive buyers and
//! the backend on a clock, then check what every party was told.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, anyhow, bail};
use cavea_format::wire::{self, PROTOCOL_VERSION, ServerFrame, Subscribe, View};
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, sleep, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::check::{Ack, BACKEND, History, Kind, Party, Refusal, Violation};
use crate::hist::Hist;
use crate::http::{Conn, connect};
use crate::schedule::{Schedule, open_loop};
use crate::view::{Applied, Rng, SeatView};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Concurrent bookings; the backend's addresses follow the buyers'.
const BACKEND_WORKERS: u64 = 32;
const BACKEND_ADDRESS: u64 = 1 << 23;

/// How simulated clients get distinct addresses (08 §12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Bind each client to its own address in 127.1.0.0/16 and above: real
    /// per-address limits, and an ephemeral port range per address.
    Loopback,
    /// Send `X-Forwarded-For` with an address of 10.0.0.0/8; the server must
    /// list the generator in `CAVEA_TRUSTED_PROXIES`.
    Forwarded,
    /// One address for everything: the server's per-address limits must be off.
    Single,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub url: String,
    pub server: SocketAddr,
    /// `None`: set up a workspace, chart (this document) and event.
    pub chart: Option<String>,
    pub event: Option<String>,
    pub admin_key: Option<String>,
    pub secret_key: Option<String>,
    pub publishable_key: Option<String>,
    pub subscribers: u64,
    pub buyers: u64,
    /// Stampede: buyers' holds spread evenly over this.
    pub ramp: Duration,
    /// Sustained: arrivals per second for `duration`; 0 for a stampede.
    pub rate: f64,
    pub duration: Duration,
    pub clustered: bool,
    pub release_share: f64,
    pub book_share: f64,
    pub retry_book_share: f64,
    pub blocks_per_sec: f64,
    /// The backend cancels each booking this long after making it; zero
    /// keeps bookings.
    pub unbook_after: Duration,
    pub hold_ttl: u64,
    pub max_objects: u32,
    pub source: Source,
    pub addresses: u64,
    pub max_in_flight: usize,
    pub probes: u64,
    pub seed: u64,
    pub interval: Duration,
    pub quiet: Duration,
    pub server_pid: Option<u32>,
}

#[derive(Default)]
pub struct Op {
    pub ok: u64,
    pub errors: BTreeMap<String, u64>,
    pub hist: Hist,
}

#[derive(Default)]
pub struct Stats {
    pub ops: BTreeMap<&'static str, Op>,
    pub delta: Hist,
    pub opened: u64,
    pub failed: u64,
    pub dropped: u64,
    pub resyncs: u64,
    pub closes: BTreeMap<u8, u64>,
    pub frames: u64,
    pub holds_refused: u64,
}

/// What the report needs from a run.
pub struct Outcome {
    pub workspace: Option<String>,
    pub event: String,
    pub objects: u32,
    pub stats: Stats,
    pub schedule: Schedule,
    pub violations: Vec<Violation>,
    pub violations_total: u64,
    pub converged: (u64, u64, u64),
    pub series: Vec<Value>,
    pub wall: Duration,
}

struct Shared {
    cfg: Config,
    host: String,
    event: String,
    pk: String,
    sk: String,
    hash: String,
    objects: u32,
    segments: Vec<(u32, u32)>,
    oid_idx: HashMap<u32, u32>,
    ttl_ms: i64,
    limit: u32,
    stats: Mutex<Stats>,
    history: Mutex<History>,
    violations: Mutex<(Vec<Violation>, u64)>,
    views: Mutex<Vec<Arc<Mutex<SeatView>>>>,
    /// Sampled hold acknowledgements, `seq` to local receive time.
    sampled: Mutex<BTreeMap<u64, i64>>,
    in_flight: AtomicI64,
    open: AtomicI64,
    order: AtomicU64,
    unbooks: AtomicI64,
    stop: watch::Receiver<bool>,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn micros(d: Duration) -> u64 {
    d.as_micros().min(u128::from(u64::MAX)) as u64
}

impl Shared {
    fn op(&self, name: &'static str, started: Instant, result: std::result::Result<(), String>) {
        if let Ok(mut s) = self.stats.lock() {
            let op = s.ops.entry(name).or_default();
            match result {
                Ok(()) => {
                    op.ok += 1;
                    op.hist.record(micros(started.elapsed()));
                }
                Err(code) => *op.errors.entry(code).or_default() += 1,
            }
        }
    }

    fn violation(&self, check: &'static str, detail: String) {
        if let Ok(mut v) = self.violations.lock() {
            v.1 += 1;
            if v.0.len() < 100 {
                v.0.push(Violation { check, detail });
            }
        }
    }

    fn record(&self, f: impl FnOnce(&mut History)) {
        if let Ok(mut h) = self.history.lock() {
            f(&mut h);
        }
    }

    /// Client `i`'s address and `X-Forwarded-For`.
    fn client(&self, i: u64) -> (Option<IpAddr>, Option<String>) {
        let n = (i % self.cfg.addresses.max(1)) as u32;
        match self.cfg.source {
            Source::Loopback => {
                let base = u32::from(Ipv4Addr::new(127, 1, 0, 1));
                (Some(IpAddr::V4(Ipv4Addr::from(base + n))), None)
            }
            Source::Forwarded => {
                let [_, a, b, c] = (0x0A00_0001 + n).to_be_bytes();
                (None, Some(format!("10.{a}.{b}.{c}")))
            }
            Source::Single => (None, None),
        }
    }

    async fn http(&self, i: u64) -> Result<Conn> {
        let (source, forwarded) = self.client(i);
        let stream = timeout(REQUEST_TIMEOUT, connect(self.cfg.server, source))
            .await
            .context("connect timed out")??;
        Ok(Conn::new(stream, self.host.clone(), forwarded))
    }

    fn objects(&self, idx: &[u32]) -> Value {
        json!({ "version": self.hash, "idx": idx })
    }

    /// Objects named in a reply or refusal, as `idx`.
    fn idx_of(&self, list: &Value) -> Vec<u32> {
        list.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|o| o["oid"].as_u64())
                    .filter_map(|oid| self.oid_idx.get(&(oid as u32)).copied())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// A JSON request; `Err` only for transport failures.
async fn call(
    conn: &mut Conn,
    method: &str,
    path: &str,
    auth: &str,
    extra: &[(&str, &str)],
    body: Option<&Value>,
) -> Result<(u16, Value)> {
    let bearer = format!("Bearer {auth}");
    let mut headers = vec![("Authorization", bearer.as_str())];
    headers.extend_from_slice(extra);
    let body = body.map(serde_json::to_vec).transpose()?;
    let r = timeout(
        REQUEST_TIMEOUT,
        conn.request(method, path, &headers, body.as_deref()),
    )
    .await
    .context("request timed out")??;
    let v = if r.body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&r.body).unwrap_or(Value::Null)
    };
    Ok((r.status, v))
}

fn code(v: &Value) -> String {
    v["code"]
        .as_str()
        .unwrap_or("unexpected_response")
        .to_owned()
}

fn expect(status: u16, v: &Value, ok: u16, what: &str) -> Result<()> {
    if status != ok {
        bail!("{what}: HTTP {status} {}", code(v));
    }
    Ok(())
}

/// Percent-encode a label for a query string.
fn query_escape(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Create a workspace, keys, a published chart and an event, or check the
/// given ones; then learn the chart's rows and every object's `oid`.
async fn prepare(
    cfg: Config,
    stop: watch::Receiver<bool>,
) -> Result<(Arc<Shared>, Option<String>, Vec<u8>)> {
    let host = cfg.server.to_string();
    let mut c = Conn::new(
        connect(cfg.server, None)
            .await
            .context("cannot reach the server")?,
        host.clone(),
        None,
    );
    let (mut sk, mut pk, mut event, mut workspace) = (
        cfg.secret_key.clone(),
        cfg.publishable_key.clone(),
        cfg.event.clone(),
        None,
    );
    if let Some(chart) = &cfg.chart {
        let admin = cfg
            .admin_key
            .as_deref()
            .ok_or_else(|| anyhow!("setting up needs CAVEA_ADMIN_KEY"))?;
        let document: Value =
            serde_json::from_str(chart).context("the chart document is not JSON")?;
        let key = format!("lg-{}-{}", cfg.seed, now_ms());
        let (s, v) = call(
            &mut c,
            "POST",
            "/v1/workspaces",
            admin,
            &[],
            Some(&json!({"key": key, "name": "cavea-loadgen"})),
        )
        .await?;
        expect(s, &v, 201, "creating the workspace")?;
        let secret = v["secret_key"]["key"]
            .as_str()
            .ok_or_else(|| anyhow!("no secret key returned"))?
            .to_owned();
        let (s, v) = call(
            &mut c,
            "POST",
            "/v1/charts",
            &secret,
            &[],
            Some(&json!({"key": "loadgen", "name": "loadgen", "document": document})),
        )
        .await?;
        expect(s, &v, 201, "creating the chart")?;
        let (s, v) = call(
            &mut c,
            "POST",
            "/v1/charts/loadgen/publish",
            &secret,
            &[],
            None,
        )
        .await?;
        if s != 200 && s != 201 {
            bail!("publishing the chart: HTTP {s} {}", code(&v));
        }
        let settings = json!({"hold_ttl_seconds": cfg.hold_ttl, "max_objects": cfg.max_objects});
        let (s, v) = call(
            &mut c,
            "POST",
            "/v1/events",
            &secret,
            &[],
            Some(&json!({"key": "loadgen", "chart": "loadgen", "settings": settings})),
        )
        .await?;
        expect(s, &v, 201, "creating the event")?;
        let (s, v) = call(
            &mut c,
            "POST",
            "/v1/keys",
            &secret,
            &[],
            Some(&json!({"kind": "publishable", "name": "loadgen"})),
        )
        .await?;
        expect(s, &v, 201, "creating a publishable key")?;
        pk = v["key"].as_str().map(str::to_owned);
        sk = Some(secret);
        event = Some("loadgen".into());
        workspace = Some(key);
    }
    let (sk, pk, event) = match (sk, pk, event) {
        (Some(s), Some(p), Some(e)) => (s, p, e),
        _ => bail!("an existing event needs --event, CAVEA_SECRET_KEY and CAVEA_PUBLISHABLE_KEY"),
    };
    let (s, info) = call(
        &mut c,
        "GET",
        &format!("/v1/public/events/{}", query_escape(&event)),
        &pk,
        &[],
        None,
    )
    .await?;
    expect(s, &info, 200, "reading the public event")?;
    let hash = info["chart"]["hash"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let url = info["chart"]["url"].as_str().unwrap_or_default().to_owned();
    let ttl_ms = info["hold_ttl_seconds"].as_i64().unwrap_or(900) * 1_000;
    let limit = info["max_objects"].as_u64().unwrap_or(10) as u32;
    let file = c.request("GET", &url, &[], None).await?;
    if file.status != 200 {
        bail!("fetching the chart file: HTTP {}", file.status);
    }
    let chart = cavea_format::decode(&file.body, cavea_format::Artefact::Public)
        .map_err(|e| anyhow!("decoding the chart: {e:?}"))?;
    let segments = chart
        .segments
        .iter()
        .map(|s| (s.seat_first, u32::from(s.seat_count)))
        .collect();
    // Every object in `idx` order, for `oid` to `idx`.
    let mut oid_idx = HashMap::new();
    let mut after: Option<String> = None;
    loop {
        let path = match &after {
            Some(a) => format!(
                "/v1/events/{}/objects?limit=100&after={}",
                query_escape(&event),
                query_escape(a)
            ),
            None => format!("/v1/events/{}/objects?limit=100", query_escape(&event)),
        };
        let (s, page) = call(&mut c, "GET", &path, &sk, &[], None).await?;
        expect(s, &page, 200, "listing the event's objects")?;
        for o in page["data"].as_array().into_iter().flatten() {
            let oid = o["oid"]
                .as_u64()
                .ok_or_else(|| anyhow!("an object without an oid"))?;
            let n = oid_idx.len() as u32;
            oid_idx.insert(oid as u32, n);
        }
        match page["next"].as_str() {
            Some(n) => after = Some(n.to_owned()),
            None => break,
        }
    }
    let (s, snapshot) = state(&mut c, &event, &pk).await?;
    if s != 200 {
        bail!("fetching the snapshot: HTTP {s}");
    }
    let objects = match wire::decode_server(&snapshot, View::Public) {
        Ok(ServerFrame::Snapshot(snap)) => snap.objects,
        _ => bail!("the snapshot is not a SNAPSHOT frame"),
    };
    if objects as usize != oid_idx.len() {
        bail!(
            "the snapshot has {objects} objects, the objects query {}",
            oid_idx.len()
        );
    }
    let shared = Shared {
        cfg,
        host,
        event,
        pk,
        sk,
        hash,
        objects,
        segments,
        oid_idx,
        ttl_ms,
        limit,
        stats: Mutex::default(),
        history: Mutex::default(),
        violations: Mutex::default(),
        views: Mutex::default(),
        sampled: Mutex::default(),
        in_flight: AtomicI64::new(0),
        open: AtomicI64::new(0),
        order: AtomicU64::new(0),
        unbooks: AtomicI64::new(0),
        stop,
    };
    Ok((Arc::new(shared), workspace, snapshot))
}

async fn state(c: &mut Conn, event: &str, pk: &str) -> Result<(u16, Vec<u8>)> {
    let path = format!(
        "/v1/public/events/{}/state?key={}",
        query_escape(event),
        query_escape(pk)
    );
    let r = timeout(REQUEST_TIMEOUT, c.request("GET", &path, &[], None))
        .await
        .context("snapshot timed out")??;
    Ok((r.status, r.body))
}

/// A subscriber: snapshot over HTTP, then the WebSocket from its `seq`,
/// applying every frame to its own view until the run stops.
async fn subscribe(sh: Arc<Shared>, i: u64, probe: bool) -> Result<Arc<Mutex<SeatView>>> {
    let mut c = sh.http(i).await?;
    let t = Instant::now();
    let (s, body) = state(&mut c, &sh.event, &sh.pk).await?;
    let mut view = SeatView::new(sh.objects);
    match (s, wire::decode_server(&body, View::Public)) {
        (200, Ok(ServerFrame::Snapshot(snap))) => {
            view.load(&snap).map_err(|e| anyhow!("{e}"))?;
            sh.op("snapshot", t, Ok(()));
        }
        (s, _) => {
            sh.op("snapshot", t, Err(format!("http_{s}")));
            bail!("snapshot: HTTP {s}");
        }
    }
    drop(c);
    let t = Instant::now();
    let (source, forwarded) = sh.client(i);
    let stream = connect(sh.cfg.server, source).await;
    let ws = async {
        let mut req = format!("ws://{}/v1/public/rt?key={}", sh.host, query_escape(&sh.pk))
            .into_client_request()?;
        if let Some(f) = forwarded {
            req.headers_mut().insert("x-forwarded-for", f.parse()?);
        }
        let config = WebSocketConfig::default()
            .read_buffer_size(4096)
            .write_buffer_size(0);
        let (ws, _) =
            tokio_tungstenite::client_async_with_config(req, stream?, Some(config)).await?;
        anyhow::Ok(ws)
    };
    let mut ws = match timeout(REQUEST_TIMEOUT, ws).await {
        Ok(Ok(ws)) => ws,
        other => {
            let why = match other {
                Ok(Err(e)) => e.to_string(),
                _ => "timed out".into(),
            };
            sh.op("connect", t, Err(why.chars().take(60).collect()));
            if let Ok(mut s) = sh.stats.lock() {
                s.failed += 1;
            }
            bail!("connect: {why}");
        }
    };
    let sub = wire::encode_subscribe(&Subscribe {
        version: PROTOCOL_VERSION,
        event_key: &sh.event,
        last_seq: view.seq,
        hold_token: None,
        session_token: None,
    })
    .map_err(|e| anyhow!("{e}"))?;
    ws.send(Message::Binary(sub.into())).await?;
    sh.op("connect", t, Ok(()));
    sh.open.fetch_add(1, Relaxed);
    if let Ok(mut s) = sh.stats.lock() {
        s.opened += 1;
    }
    let view = Arc::new(Mutex::new(view));
    if let Ok(mut v) = sh.views.lock() {
        v.push(view.clone());
    }
    let mine = view.clone();
    let mut stop = sh.stop.clone();
    tokio::spawn(async move {
        let mut dropped = true;
        loop {
            let msg = tokio::select! {
                _ = stop.wait_for(|s| *s) => { dropped = false; break; }
                m = ws.next() => m,
            };
            let Some(Ok(msg)) = msg else { break };
            let Message::Binary(bytes) = msg else {
                continue;
            };
            let now = now_ms();
            let applied = match wire::decode_server(&bytes, View::Public) {
                Ok(frame) => mine
                    .lock()
                    .map_err(|_| None)
                    .and_then(|mut v| v.apply(&frame).map_err(Some)),
                Err(e) => Err(Some(crate::view::ViewError::Wire(e))),
            };
            if let Ok(mut s) = sh.stats.lock() {
                s.frames += 1;
            }
            match applied {
                Ok(Applied::Pong(n)) => {
                    if ws
                        .send(Message::Binary(wire::encode_pong(n).into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(Applied::Delta { from, to }) if probe => {
                    let samples: Vec<i64> = sh
                        .sampled
                        .lock()
                        .map(|m| m.range(from + 1..=to).map(|(_, t)| *t).collect())
                        .unwrap_or_default();
                    if let Ok(mut s) = sh.stats.lock() {
                        for t in samples {
                            s.delta.record(u64::try_from(now - t).unwrap_or(0) * 1_000);
                        }
                    }
                }
                Ok(Applied::Resync) => {
                    if let Ok(mut s) = sh.stats.lock() {
                        s.resyncs += 1;
                    }
                }
                Ok(Applied::Close(reason)) => {
                    if let Ok(mut s) = sh.stats.lock() {
                        *s.closes.entry(reason).or_default() += 1;
                    }
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    let detail = e.map_or("view lock poisoned".into(), |e| e.to_string());
                    sh.violation("protocol", format!("subscriber {i}: {detail}"));
                    break;
                }
            }
        }
        sh.open.fetch_sub(1, Relaxed);
        if dropped {
            if let Ok(mut s) = sh.stats.lock() {
                s.dropped += 1;
            }
            // A dropped view no longer follows the server: leave it out.
            if let Ok(mut v) = sh.views.lock() {
                v.retain(|x| !Arc::ptr_eq(x, &mine));
            }
        }
        let _ = ws.close(None).await;
    });
    Ok(view)
}

/// A buyer: a token, 2 to 4 seats its view shows available, a hold (another
/// pick after a refusal, twice at most), then release, booking or expiry.
async fn buy(
    sh: Arc<Shared>,
    party: Party,
    view: Option<Arc<Mutex<SeatView>>>,
    due: Instant,
    backend: mpsc::UnboundedSender<(Party, String, Vec<u32>)>,
) {
    sh.in_flight.fetch_add(1, Relaxed);
    let mut rng = Rng(sh.cfg.seed ^ (u64::from(party) << 20) ^ 0xB0B);
    if let Err(e) = buy_inner(&sh, party, view, due, &backend, &mut rng).await {
        sh.op("buyer", due, Err(e.to_string().chars().take(60).collect()));
    }
    sh.in_flight.fetch_sub(1, Relaxed);
}

async fn buy_inner(
    sh: &Arc<Shared>,
    party: Party,
    view: Option<Arc<Mutex<SeatView>>>,
    due: Instant,
    backend: &mpsc::UnboundedSender<(Party, String, Vec<u32>)>,
    rng: &mut Rng,
) -> Result<()> {
    let mut c = sh.http(u64::from(party) + sh.cfg.subscribers).await?;
    let send = now_ms();
    let (s, v) = call(
        &mut c,
        "POST",
        "/v1/public/hold-tokens",
        &sh.pk,
        &[],
        Some(&json!({"event": sh.event})),
    )
    .await?;
    if s != 201 {
        sh.op("token", due, Err(code(&v)));
        return Ok(());
    }
    sh.op("token", due, Ok(()));
    let recv = now_ms();
    let token = v["hold_token"].as_str().unwrap_or_default().to_owned();
    let expires = v["expires_at"]
        .as_str()
        .and_then(parse_rfc3339_ms)
        .or_else(|| v["expires_at"].as_i64());
    if let Some(e) = expires {
        sh.record(|h| {
            h.expiry.insert(party, e);
            h.clock
                .observe(e - sh.ttl_ms, e - sh.ttl_ms + 999, send, recv);
        });
    }
    let view = match view {
        Some(v) => v,
        None => {
            let views = sh.views.lock().map(|v| v.clone()).unwrap_or_default();
            views
                .get(rng.below(views.len() as u64) as usize)
                .cloned()
                .ok_or_else(|| anyhow!("no subscriber to pick from"))?
        }
    };
    for _ in 0..3 {
        let k = 2 + rng.below(3) as u32;
        let pick = view
            .lock()
            .ok()
            .and_then(|v| v.pick(&sh.segments, k, sh.cfg.clustered, rng));
        let Some(pick) = pick else {
            sh.op("hold", due, Err("nothing_available".into()));
            return Ok(());
        };
        let started = Instant::now();
        let send = now_ms();
        let body = json!({"hold_token": token, "objects": sh.objects(&pick)});
        let path = format!("/v1/public/events/{}/hold", query_escape(&sh.event));
        let reply = call(&mut c, "POST", &path, &sh.pk, &[], Some(&body)).await;
        let recv = now_ms();
        let (s, v) = match reply {
            Ok(r) => r,
            Err(e) => {
                sh.op("hold", started, Err("transport".into()));
                sh.record(|h| h.unknown.push((party, pick, send)));
                return Err(e);
            }
        };
        match (s, v["code"].as_str()) {
            (200, _) => {
                sh.op("hold", started, Ok(()));
                let held = sh.idx_of(&v["objects"]);
                let seq = v["seq"].as_u64().unwrap_or(0);
                if held != pick {
                    sh.violation(
                        "hold_reply",
                        format!("party {party}: asked for {pick:?}, told it holds {held:?}"),
                    );
                }
                if held.len() as u32 > sh.limit {
                    sh.violation(
                        "limit",
                        format!(
                            "party {party}: holds {} over its limit {}",
                            held.len(),
                            sh.limit
                        ),
                    );
                }
                if let Ok(mut m) = sh.sampled.lock() {
                    m.insert(seq, recv);
                }
                sh.record(|h| {
                    h.acks.push(Ack {
                        party,
                        kind: Kind::Hold,
                        seq,
                        objects: held.clone(),
                        send_ms: send,
                        recv_ms: recv,
                    })
                });
                let fate = (rng.next() >> 11) as f64 / (1u64 << 53) as f64;
                if fate < sh.cfg.release_share {
                    sleep(Duration::from_millis(rng.below(1_000))).await;
                    release(sh, &mut c, party, &token, &held).await;
                } else if fate < sh.cfg.release_share + sh.cfg.book_share {
                    let _ = backend.send((party, token.clone(), held));
                }
                return Ok(());
            }
            (409, Some("object_not_free")) => {
                sh.op("hold", started, Err("object_not_free".into()));
                if let Ok(mut st) = sh.stats.lock() {
                    st.holds_refused += 1;
                }
                let listed = v["details"]["objects"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let named = sh.idx_of(&v["details"]["objects"]);
                if listed.is_empty()
                    || listed.iter().any(|o| o["reason"] != "not_free")
                    || named.iter().any(|o| !pick.contains(o))
                {
                    sh.violation(
                        "refusal",
                        format!("party {party}: object_not_free naming {listed:?} for {pick:?}"),
                    );
                }
                sh.record(|h| {
                    h.refusals.push(Refusal {
                        party,
                        objects: named,
                        send_ms: send,
                        recv_ms: recv,
                    })
                });
            }
            (422, Some("hold_limit_exceeded")) => {
                sh.op("hold", started, Err("hold_limit_exceeded".into()));
                // Nothing held yet: only a request over the limit may get it.
                if k <= sh.limit {
                    sh.violation(
                        "refusal",
                        format!(
                            "party {party}: hold_limit_exceeded for {k} objects, limit {}",
                            sh.limit
                        ),
                    );
                }
                return Ok(());
            }
            (503, Some("unavailable")) => {
                sh.op("hold", started, Err("unavailable".into()));
                sh.record(|h| h.unknown.push((party, pick, send)));
                return Ok(());
            }
            (429 | 503, _) => {
                sh.op("hold", started, Err(code(&v)));
                return Ok(());
            }
            (s, _) => {
                sh.op("hold", started, Err(code(&v)));
                sh.violation(
                    "hold_reply",
                    format!("party {party}: HTTP {s} {}", code(&v)),
                );
                return Ok(());
            }
        }
    }
    Ok(())
}

async fn release(sh: &Arc<Shared>, c: &mut Conn, party: Party, token: &str, held: &[u32]) {
    let t = Instant::now();
    let send = now_ms();
    let path = format!("/v1/public/events/{}/release", query_escape(&sh.event));
    match call(
        c,
        "POST",
        &path,
        &sh.pk,
        &[],
        Some(&json!({"hold_token": token})),
    )
    .await
    {
        Ok((200, v)) => {
            sh.op("release", t, Ok(()));
            let mut objects = sh.idx_of(&v["objects"]);
            objects.sort_unstable();
            if objects != held {
                sh.violation(
                    "release_reply",
                    format!("party {party}: released {objects:?}, held {held:?}"),
                );
            }
            let seq = v["seq"].as_u64().unwrap_or(0);
            sh.record(|h| {
                h.acks.push(Ack {
                    party,
                    kind: Kind::Release,
                    seq,
                    objects,
                    send_ms: send,
                    recv_ms: now_ms(),
                })
            });
        }
        Ok((_, v)) => sh.op("release", t, Err(code(&v))),
        Err(_) => sh.op("release", t, Err("transport".into())),
    }
}

/// The backend: books with the secret key and an `Idempotency-Key`, sending a
/// share of bookings twice, which must get the same reply.
async fn book(
    sh: Arc<Shared>,
    c: &mut Conn,
    party: Party,
    token: String,
    held: Vec<u32>,
    rng: &mut Rng,
) -> bool {
    let n = sh.order.fetch_add(1, Relaxed);
    let key = format!("lg-{}-{n}", sh.cfg.seed);
    let body = json!({"hold_token": token, "order_ref": format!("loadgen-{n}")});
    let path = format!("/v1/events/{}/book", query_escape(&sh.event));
    let tries = if rng.chance(sh.cfg.retry_book_share) {
        2
    } else {
        1
    };
    let mut first: Option<Value> = None;
    for _ in 0..tries {
        let t = Instant::now();
        let send = now_ms();
        match call(
            c,
            "POST",
            &path,
            &sh.sk,
            &[("Idempotency-Key", &key)],
            Some(&body),
        )
        .await
        {
            Ok((200, v)) => {
                sh.op("book", t, Ok(()));
                match &first {
                    Some(f) if f["seq"] != v["seq"] || f["objects"] != v["objects"] => {
                        sh.violation(
                            "idempotency",
                            format!(
                                "booking {n}: a retry got seq {}, the first {}",
                                v["seq"], f["seq"]
                            ),
                        );
                    }
                    Some(_) => {}
                    None => {
                        let mut objects = sh.idx_of(&v["objects"]);
                        objects.sort_unstable();
                        if objects != held {
                            sh.violation(
                                "book_reply",
                                format!("party {party}: booked {objects:?}, held {held:?}"),
                            );
                        }
                        let seq = v["seq"].as_u64().unwrap_or(0);
                        if !sh.cfg.unbook_after.is_zero() {
                            sh.unbooks.fetch_add(1, Relaxed);
                            tokio::spawn(unbook(sh.clone(), objects.clone(), n));
                        }
                        sh.record(|h| {
                            h.acks.push(Ack {
                                party,
                                kind: Kind::Book,
                                seq,
                                objects,
                                send_ms: send,
                                recv_ms: now_ms(),
                            })
                        });
                        first = Some(v);
                    }
                }
            }
            Ok((s, v)) => {
                let code = code(&v);
                sh.op("book", t, Err(code.clone()));
                // An expired hold is not found; anything else is wrong.
                if !matches!(
                    (s, code.as_str()),
                    (404, "not_found") | (503, "overloaded" | "unavailable")
                ) {
                    sh.violation("book_reply", format!("party {party}: HTTP {s} {code}"));
                }
                return true;
            }
            Err(_) => {
                sh.op("book", t, Err("transport".into()));
                return false;
            }
        }
    }
    true
}

/// A cancellation: the backend releases a booking `unbook_after` after
/// making it, so that a sustained run does not fill the event.
async fn unbook(sh: Arc<Shared>, objects: Vec<u32>, n: u64) {
    sleep(sh.cfg.unbook_after).await;
    let t = Instant::now();
    let send = now_ms();
    let path = format!("/v1/events/{}/release", query_escape(&sh.event));
    let body = json!({ "objects": sh.objects(&objects) });
    let reply = match sh
        .http(BACKEND_ADDRESS + BACKEND_WORKERS + 2 + n % 64)
        .await
    {
        Ok(mut c) => call(&mut c, "POST", &path, &sh.sk, &[], Some(&body)).await,
        Err(e) => Err(e),
    };
    match reply {
        Ok((200, v)) => {
            sh.op("unbook", t, Ok(()));
            let mut got = sh.idx_of(&v["objects"]);
            got.sort_unstable();
            if got != objects {
                sh.violation(
                    "unbook_reply",
                    format!("booking {n}: released {got:?}, booked {objects:?}"),
                );
            }
            let seq = v["seq"].as_u64().unwrap_or(0);
            sh.record(|h| {
                h.acks.push(Ack {
                    party: BACKEND,
                    kind: Kind::Unbook,
                    seq,
                    objects: got,
                    send_ms: send,
                    recv_ms: now_ms(),
                })
            });
        }
        Ok((s, v)) => {
            sh.op("unbook", t, Err(code(&v)));
            if !matches!((s, code(&v).as_str()), (503, "overloaded" | "unavailable")) {
                sh.violation(
                    "unbook_reply",
                    format!("booking {n}: HTTP {s} {}", code(&v)),
                );
            }
        }
        Err(_) => sh.op("unbook", t, Err("transport".into())),
    }
    sh.unbooks.fetch_sub(1, Relaxed);
}

async fn change(sh: &Arc<Shared>, c: &mut Conn, idx: u32, status: &str) -> Option<(u64, i64, i64)> {
    let t = Instant::now();
    let send = now_ms();
    let path = format!("/v1/events/{}/change-status", query_escape(&sh.event));
    let name = if status == "blocked" {
        "block"
    } else {
        "unblock"
    };
    match call(
        c,
        "POST",
        &path,
        &sh.sk,
        &[],
        Some(&json!({"objects": sh.objects(&[idx]), "status": status})),
    )
    .await
    {
        Ok((200, v)) => {
            sh.op(name, t, Ok(()));
            Some((v["seq"].as_u64().unwrap_or(0), send, now_ms()))
        }
        Ok((_, v)) => {
            sh.op(name, t, Err(code(&v)));
            None
        }
        Err(_) => {
            sh.op(name, t, Err("transport".into()));
            None
        }
    }
}

/// Booking workers, each with its own connection, sharing one queue.
async fn backend(sh: Arc<Shared>, rx: mpsc::UnboundedReceiver<(Party, String, Vec<u32>)>) {
    let rx = Arc::new(tokio::sync::Mutex::new(rx));
    let mut workers = tokio::task::JoinSet::new();
    for w in 0..BACKEND_WORKERS {
        let (sh, rx) = (sh.clone(), rx.clone());
        workers.spawn(async move {
            let mut rng = Rng(sh.cfg.seed ^ 0xBAC ^ (w << 32));
            let mut conn: Option<Conn> = None;
            loop {
                let job = rx.lock().await.recv().await;
                let Some((party, token, held)) = job else {
                    break;
                };
                if conn.is_none() {
                    conn = sh.http(BACKEND_ADDRESS + w).await.ok();
                }
                let Some(c) = conn.as_mut() else {
                    sh.op("book", Instant::now(), Err("transport".into()));
                    continue;
                };
                if !book(sh.clone(), c, party, token, held, &mut rng).await {
                    conn = None;
                }
            }
        });
    }
    while workers.join_next().await.is_some() {}
}

/// The backend blocking a free seat and unblocking it 200 ms later, at
/// `blocks_per_sec`, until the load ends.
async fn blocker(sh: Arc<Shared>, mut done: watch::Receiver<bool>) {
    if sh.cfg.blocks_per_sec <= 0.0 {
        return;
    }
    let Ok(mut c) = sh.http(BACKEND_ADDRESS + BACKEND_WORKERS).await else {
        return;
    };
    let mut rng = Rng(sh.cfg.seed ^ 0xB10C);
    let mut tick = tokio::time::interval(Duration::from_secs_f64(1.0 / sh.cfg.blocks_per_sec));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = done.wait_for(|d| *d) => break,
        }
        let views = sh.views.lock().map(|v| v.clone()).unwrap_or_default();
        let pick = views.first().and_then(|v| {
            v.lock()
                .ok()
                .and_then(|v| v.pick(&sh.segments, 1, false, &mut rng))
        });
        if let Some(&[idx]) = pick.as_deref()
            && let Some((seq, send, recv)) = change(&sh, &mut c, idx, "blocked").await
        {
            sh.record(|h| {
                h.acks.push(Ack {
                    party: BACKEND,
                    kind: Kind::Block,
                    seq,
                    objects: vec![idx],
                    send_ms: send,
                    recv_ms: recv,
                })
            });
            sleep(Duration::from_millis(200)).await;
            if let Some((seq, send, recv)) = change(&sh, &mut c, idx, "free").await {
                sh.record(|h| {
                    h.acks.push(Ack {
                        party: BACKEND,
                        kind: Kind::Unblock,
                        seq,
                        objects: vec![idx],
                        send_ms: send,
                        recv_ms: recv,
                    })
                });
            }
        }
    }
}

/// `2026-11-11T10:00:00.123Z` (UTC, as the server writes it) as Unix ms.
pub fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b.last() != Some(&b'Z') {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let frac = s.get(19..s.len() - 1).unwrap_or("");
    let ms = match frac.strip_prefix('.') {
        Some(f) if !f.is_empty() => format!("{f:0<3}").get(..3)?.parse::<i64>().ok()?,
        _ => 0,
    };
    // Days from 1970-01-01 (Howard Hinnant's algorithm).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + h) * 60 + mi) * 60_000 + sec * 1_000 + ms)
}

fn rss_mb(pid: Option<u32>) -> Option<f64> {
    let path = pid.map_or("/proc/self/status".to_owned(), |p| {
        format!("/proc/{p}/status")
    });
    let text = std::fs::read_to_string(path).ok()?;
    let kb: f64 = text
        .lines()
        .find(|l| l.starts_with("VmRSS:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some(kb / 1024.0)
}

pub async fn run(cfg: Config) -> Result<Outcome> {
    let started = Instant::now();
    let (stop_tx, stop_rx) = watch::channel(false);
    let (sh, workspace, before) = prepare(cfg, stop_rx).await?;
    if let Ok(ServerFrame::Snapshot(snap)) = wire::decode_server(&before, View::Public) {
        let mut v = SeatView::new(sh.objects);
        if v.load(&snap).is_ok() {
            sh.record(|h| h.before = (0..sh.objects).filter(|&i| !v.available(i)).collect());
        }
    }
    eprintln!(
        "event {}{} with {} objects; {} subscribers, {} buyers",
        workspace
            .as_deref()
            .map(|w| format!("{w}/"))
            .unwrap_or_default(),
        sh.event,
        sh.objects,
        sh.cfg.subscribers,
        sh.cfg.buyers
    );
    // The time series, until the run stops.
    let series = Arc::new(Mutex::new(Vec::new()));
    let sampler = {
        let (sh, series) = (sh.clone(), series.clone());
        let mut stop = sh.stop.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(sh.cfg.interval);
            loop {
                tokio::select! { _ = tick.tick() => {}, _ = stop.wait_for(|s| *s) => break }
                let (holds, refused, errors) = sh
                    .stats
                    .lock()
                    .map(|s| {
                        let h = s.ops.get("hold");
                        (
                            h.map_or(0, |o| o.ok),
                            s.holds_refused,
                            s.ops
                                .values()
                                .map(|o| o.errors.values().sum::<u64>())
                                .sum::<u64>(),
                        )
                    })
                    .unwrap_or_default();
                if let Ok(mut m) = sh.sampled.lock() {
                    // Probes have had 30 s to see these.
                    let old = now_ms() - 30_000;
                    m.retain(|_, t| *t > old);
                }
                if let Ok(mut s) = series.lock() {
                    s.push(json!({
                        "t_s": started.elapsed().as_secs_f64(),
                        "holds_ok": holds, "holds_refused": refused, "errors": errors,
                        "in_flight": sh.in_flight.load(Relaxed), "subscribers_open": sh.open.load(Relaxed),
                        "generator_rss_mb": rss_mb(None), "server_rss_mb": sh.cfg.server_pid.and_then(|p| rss_mb(Some(p))),
                    }));
                }
            }
        })
    };
    let (tx, rx) = mpsc::unbounded_channel();
    let backend_task = tokio::spawn(backend(sh.clone(), rx));
    let (done_tx, done_rx) = watch::channel(false);
    let blocker_task = tokio::spawn(blocker(sh.clone(), done_rx));
    // Subscribers (watchers first, the first `probes` of them measuring
    // delta delivery), then, for a stampede, each buyer's own subscription.
    let stampede = sh.cfg.rate <= 0.0;
    let subscriptions = sh.cfg.subscribers + if stampede { sh.cfg.buyers } else { 0 };
    let connect_rate = 2_000.0;
    let mut buyer_views: Vec<Option<Arc<Mutex<SeatView>>>> =
        vec![None; if stampede { sh.cfg.buyers as usize } else { 0 }];
    let slots = Arc::new(Mutex::new(std::mem::take(&mut buyer_views)));
    open_loop(
        Instant::now(),
        connect_rate,
        subscriptions,
        sh.cfg.max_in_flight,
        |k, _| {
            let (sh, slots) = (sh.clone(), slots.clone());
            async move {
                let probe = k < sh.cfg.probes;
                match subscribe(sh.clone(), k, probe).await {
                    Ok(v) if k >= sh.cfg.subscribers => {
                        if let Ok(mut s) = slots.lock()
                            && let Some(slot) = s.get_mut((k - sh.cfg.subscribers) as usize)
                        {
                            *slot = Some(v);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("subscriber {k}: {e:#}"),
                }
            }
        },
    )
    .await;
    let buyer_views = slots.lock().map(|s| s.clone()).unwrap_or_default();
    // The measured load.
    let (rate, count) = if stampede {
        (
            sh.cfg.buyers as f64 / sh.cfg.ramp.as_secs_f64().max(1e-3),
            sh.cfg.buyers,
        )
    } else {
        (
            sh.cfg.rate,
            (sh.cfg.rate * sh.cfg.duration.as_secs_f64()) as u64,
        )
    };
    let schedule = open_loop(
        Instant::now(),
        rate,
        count,
        sh.cfg.max_in_flight,
        |k, due| {
            let (sh, tx) = (sh.clone(), tx.clone());
            let view = buyer_views.get(k as usize).cloned().flatten();
            async move { buy(sh, k as Party, view, due, tx).await }
        },
    )
    .await;
    drop(tx);
    let _ = done_tx.send(true);
    let _ = blocker_task.await;
    let _ = backend_task.await;
    let deadline = Instant::now() + sh.cfg.unbook_after + REQUEST_TIMEOUT;
    while sh.unbooks.load(Relaxed) > 0 && Instant::now() < deadline {
        sleep(Duration::from_millis(50)).await;
    }
    sleep(sh.cfg.quiet).await;
    let converged = converge(&sh).await;
    let _ = stop_tx.send(true);
    let _ = sampler.await;
    sleep(Duration::from_millis(200)).await;
    let mut violations = sh.violations.lock().map(|v| v.clone()).unwrap_or_default();
    let found = sh
        .history
        .lock()
        .map(|h| crate::check::check(&h))
        .unwrap_or_default();
    violations.1 += found.len() as u64;
    violations.0.extend(found.into_iter().take(100));
    let stats = std::mem::take(
        &mut *sh
            .stats
            .lock()
            .map_err(|_| anyhow!("stats lock poisoned"))?,
    );
    let series = std::mem::take(&mut *series.lock().map_err(|_| anyhow!("series lock poisoned"))?);
    Ok(Outcome {
        workspace,
        event: sh.event.clone(),
        objects: sh.objects,
        stats,
        schedule,
        violations: violations.0,
        violations_total: violations.1,
        converged,
        series,
        wall: started.elapsed(),
    })
}

/// Every live subscriber's view must equal a fresh snapshot once it reaches
/// that snapshot's `seq`. Returns (compared and equal, differing, never
/// caught up or always overtaken by later commits).
async fn converge(sh: &Arc<Shared>) -> (u64, u64, u64) {
    let views = sh.views.lock().map(|v| v.clone()).unwrap_or_default();
    let mut pending: Vec<usize> = (0..views.len()).collect();
    let (mut equal, mut differ) = (0, 0);
    let Ok(mut c) = sh.http(BACKEND_ADDRESS + BACKEND_WORKERS + 1).await else {
        return (0, 0, views.len() as u64);
    };
    for _ in 0..5 {
        let Ok((200, body)) = state(&mut c, &sh.event, &sh.pk).await else {
            break;
        };
        let Ok(ServerFrame::Snapshot(snap)) = wire::decode_server(&body, View::Public) else {
            break;
        };
        let mut truth = SeatView::new(sh.objects);
        if truth.load(&snap).is_err() {
            break;
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline
            && pending
                .iter()
                .any(|&i| views[i].lock().is_ok_and(|v| v.seq < truth.seq))
        {
            sleep(Duration::from_millis(50)).await;
        }
        pending.retain(|&i| {
            let Ok(v) = views[i].lock() else { return false };
            if v.seq != truth.seq {
                return true;
            }
            let diff = v.differences(&truth, 5);
            if diff.is_empty() {
                equal += 1;
            } else {
                differ += 1;
                sh.violation(
                    "convergence",
                    format!(
                        "a subscriber at seq {} differs from the snapshot at {diff:?}",
                        v.seq
                    ),
                );
            }
            false
        });
        if pending.is_empty() {
            break;
        }
    }
    (equal, differ, pending.len() as u64)
}

#[cfg(test)]
mod tests {
    #[test]
    fn rfc3339() {
        assert_eq!(super::parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            super::parse_rfc3339_ms("2026-11-11T10:00:00.5Z"),
            Some(1_794_391_200_500)
        );
        assert_eq!(
            super::parse_rfc3339_ms("2000-02-29T23:59:59.999Z"),
            Some(951_868_799_999)
        );
    }
}
