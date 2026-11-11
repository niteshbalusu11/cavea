//! One run: set up or find the event, connect subscribers, drive buyers and
//! the backend on a clock, then check what every party was told and audit it
//! against what the server has committed.
//!
//! Every request ends in one of three ways: a reply, certainly not applied
//! (no connection could be opened, or `503 overloaded`), or an unknown
//! outcome (the connection broke after the request was written, no reply came
//! within [`REQUEST_TIMEOUT`], or `503 unavailable`). The last two are outage
//! errors: the request is tried again by the rule in [`Retry`], and if every
//! attempt meets an outage and one of them was unknown, the request's outcome
//! stays unknown (`check::Unknown`), to be resolved by the audit.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, anyhow, bail};
use cavea_format::wire::{self, PROTOCOL_VERSION, ServerFrame, Subscribe, View};
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, sleep, timeout};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::check::{
    Ack, BACKEND, Failed, History, Kind, Observed, Party, Refusal, Resolution, Seen, Unknown,
    Violation,
};
use crate::hist::Hist;
use crate::http::{Conn, connect};
use crate::schedule::{Schedule, open_loop};
use crate::view::{Applied, Rng, SeatView};

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// A connection that has delivered nothing for this long is dead (the
/// server pings every 25 s; 05 §4.1).
const SILENCE: Duration = Duration::from_secs(60);
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
    /// Subscriptions started per second before the load.
    pub connect_rate: f64,
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
    /// Write every acknowledgement's `seq`, `op` and `oid`s here (CSV), for
    /// a comparison with the server's log.
    pub acks: Option<String>,
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
    pub reconnects: u64,
    pub resyncs: u64,
    pub closes: BTreeMap<u8, u64>,
    pub frames: u64,
    pub holds_refused: u64,
    /// Requests that met an outage and then got a reply.
    pub retried: BTreeMap<&'static str, u64>,
    /// Requests whose every attempt met an outage, by kind: given up.
    pub gave_up: BTreeMap<&'static str, u64>,
}

/// Generator and server CPU over the run, in seconds.
#[derive(Debug, Default, Clone, Copy)]
pub struct Cpu {
    pub generator_s: f64,
    pub server_s: Option<f64>,
    pub wall_s: f64,
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
    /// Views equal to a fresh snapshot, differing, never caught up.
    pub converged: (u64, u64, u64),
    pub series: Vec<Value>,
    pub wall: Duration,
    /// Each outage: from the first outage error to the next successful hold,
    /// Unix ms.
    pub outages: Vec<(i64, i64)>,
    /// Each period with a subscriber disconnected, Unix ms.
    pub gaps: Vec<(i64, i64)>,
    pub cpu: Cpu,
    /// The audit: objects and tokens read, and how unknown outcomes resolved.
    pub audit: Option<(usize, usize, Resolution)>,
    pub unknown: usize,
}

#[derive(Default)]
struct Outages {
    /// When the current outage's first failed attempt was sent.
    down: Option<i64>,
    /// When the last outage ended: attempts sent before it belong to it.
    up: i64,
    list: Vec<(i64, i64)>,
}

#[derive(Default)]
struct Gaps {
    since: Option<i64>,
    list: Vec<(i64, i64)>,
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
    idx_oid: Vec<u32>,
    ttl_ms: i64,
    limit: u32,
    /// Makes this run's idempotency keys and order references its own.
    run_id: i64,
    stats: Mutex<Stats>,
    history: Mutex<History>,
    violations: Mutex<(Vec<Violation>, u64)>,
    views: Mutex<Vec<Arc<Mutex<SeatView>>>>,
    /// Each buyer's hold token, for the audit; never reported.
    tokens: Mutex<HashMap<Party, String>>,
    /// Sampled hold acknowledgements, `seq` to local receive time.
    sampled: Mutex<BTreeMap<u64, i64>>,
    outages: Mutex<Outages>,
    gaps: Mutex<Gaps>,
    in_flight: AtomicI64,
    /// Subscribers following (connected or reconnecting), and connected.
    live: AtomicI64,
    open: AtomicI64,
    order: AtomicU64,
    unbooks: AtomicI64,
    /// Idle keep-alive connections for cancellations, as a backend's pool.
    unbook_pool: Mutex<Vec<Client>>,
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

/// The real SDK's reconnection delay (05 §4.1): `min(30 s, 500 ms ×
/// 2^(n−1))` for the n-th consecutive failure, times a random factor from 0.5
/// to 1.
pub fn backoff(n: u32, rng: &mut Rng) -> Duration {
    let base = 500u64
        .saturating_mul(1 << n.clamp(1, 7).saturating_sub(1))
        .min(30_000);
    let factor = 0.5 + 0.5 * ((rng.next() >> 11) as f64 / (1u64 << 53) as f64);
    Duration::from_millis((base as f64 * factor) as u64)
}

/// How long a request keeps being tried after outage errors.
#[derive(Debug, Clone, Copy)]
pub struct Retry {
    pub attempts: u32,
    pub within: Duration,
}

/// A buyer tries a request six times at most, within 20 s of the first
/// attempt, then gives up (its session ends).
pub const BUYER_RETRY: Retry = Retry {
    attempts: 6,
    within: Duration::from_secs(20),
};
/// The backend, whose commands carry `Idempotency-Key`s, keeps trying for a
/// minute: a retry of an applied command gets its original reply.
pub const BACKEND_RETRY: Retry = Retry {
    attempts: u32::MAX,
    within: Duration::from_secs(60),
};

/// What one attempt at a request came to.
#[derive(Debug)]
pub enum Attempt {
    Reply(u16, Value),
    /// Certainly not applied: no connection could be opened, or `503
    /// overloaded` (the actor's mailbox was full).
    NotApplied(&'static str),
    /// Perhaps applied: the connection broke after the request was written,
    /// no reply came in time, or `503 unavailable` (which a commit that
    /// failed or lost its reply also gives, 08 §4.6).
    Unknown(&'static str),
}

/// A request's method, path, credential, headers and body.
struct Req<'a> {
    method: &'static str,
    path: String,
    auth: &'a str,
    headers: Vec<(&'static str, String)>,
    body: Option<Value>,
}

impl<'a> Req<'a> {
    fn post(path: String, auth: &'a str, body: Value) -> Self {
        Self {
            method: "POST",
            path,
            auth,
            headers: vec![],
            body: Some(body),
        }
    }

    fn key(mut self, key: String) -> Self {
        self.headers.push(("Idempotency-Key", key));
        self
    }
}

/// One simulated client's keep-alive connection, opened again after a
/// failure, from the client's own address.
struct Client {
    addr: u64,
    conn: Option<Conn>,
}

impl Client {
    fn new(addr: u64) -> Self {
        Self { addr, conn: None }
    }

    async fn attempt(&mut self, sh: &Shared, req: &Req<'_>) -> Attempt {
        if self.conn.is_none() {
            match sh.http(self.addr).await {
                Ok(c) => self.conn = Some(c),
                Err(_) => return Attempt::NotApplied("not_sent"),
            }
        }
        let Some(conn) = self.conn.as_mut() else {
            return Attempt::NotApplied("not_sent");
        };
        let headers: Vec<(&str, &str)> =
            req.headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        match call(
            conn,
            req.method,
            &req.path,
            req.auth,
            &headers,
            req.body.as_ref(),
        )
        .await
        {
            Ok((503, v)) if v["code"] == "overloaded" => Attempt::NotApplied("overloaded"),
            Ok((503, _)) => Attempt::Unknown("unavailable"),
            Ok((s, v)) => Attempt::Reply(s, v),
            Err(e) => {
                self.conn = None;
                let kind = e
                    .chain()
                    .find_map(|c| c.downcast_ref::<std::io::Error>())
                    .map(std::io::Error::kind);
                Attempt::Unknown(match kind {
                    _ if e.to_string().contains("timed out") => "timeout",
                    Some(std::io::ErrorKind::ConnectionReset) => "transport_reset",
                    Some(std::io::ErrorKind::UnexpectedEof) => "transport_eof",
                    Some(std::io::ErrorKind::BrokenPipe) => "transport_pipe",
                    _ => "transport",
                })
            }
        }
    }
}

/// A request's last attempt, and what came before it.
struct Sent {
    last: Attempt,
    /// An earlier attempt's outcome was unknown.
    unknown_before: bool,
    first_send_ms: i64,
    send_ms: i64,
    recv_ms: i64,
    /// When the last attempt was sent.
    started: Instant,
}

impl Sent {
    /// Unknown: the last attempt, or an earlier one and then no reply.
    fn unknown(&self) -> bool {
        match self.last {
            Attempt::Unknown(_) => true,
            Attempt::NotApplied(_) => self.unknown_before,
            Attempt::Reply(..) => false,
        }
    }
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

    fn count(&self, f: impl FnOnce(&mut Stats)) {
        if let Ok(mut s) = self.stats.lock() {
            f(&mut s);
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

    /// `retried`: the reply may be a no-op's, naming a `seq` not its own.
    #[allow(clippy::too_many_arguments, reason = "one acknowledgement's fields")]
    fn ack(
        &self,
        party: Party,
        kind: Kind,
        seq: u64,
        objects: Vec<u32>,
        s: &Sent,
        retried: bool,
        order: Option<u64>,
    ) {
        self.record(|h| {
            h.acks.push(Ack {
                party,
                kind,
                seq,
                objects,
                // The first attempt may be the one that was applied.
                send_ms: s.first_send_ms,
                recv_ms: s.recv_ms,
                retried,
                order,
            });
        });
    }

    fn unknown(&self, party: Party, kind: Kind, objects: Vec<u32>, s: &Sent, order: Option<u64>) {
        self.count(|st| *st.gave_up.entry(kind.name()).or_default() += 1);
        self.record(|h| {
            h.unknown.push(Unknown {
                party,
                kind,
                objects,
                send_ms: s.first_send_ms,
                order,
            });
        });
    }

    /// An earlier attempt's unknown outcome, kept after a later one was
    /// refused.
    fn keep_unknown(&self, party: Party, kind: Kind, objects: Vec<u32>, s: &Sent) {
        self.record(|h| {
            h.unknown.push(Unknown {
                party,
                kind,
                objects,
                send_ms: s.first_send_ms,
                order: None,
            });
        });
    }

    fn failed(&self, party: Party, kind: Kind, objects: Vec<u32>, order: Option<u64>) {
        self.record(|h| {
            h.failed.push(Failed {
                party,
                kind,
                objects,
                order,
            });
        });
    }

    /// An attempt sent at `sent` met an outage error: an outage starts,
    /// unless one has or the attempt was sent before the last one ended.
    fn outage(&self, sent: i64) {
        if let Ok(mut o) = self.outages.lock()
            && o.down.is_none()
            && sent >= o.up
        {
            o.down = Some(sent);
        }
    }

    /// A hold sent at `sent` succeeded at `t`: an outage that began before
    /// it was sent ends.
    fn hold_ok(&self, sent: i64, t: i64) {
        if let Ok(mut o) = self.outages.lock()
            && let Some(d) = o.down
            && sent >= d
        {
            o.down = None;
            o.up = t;
            o.list.push((d, t));
        }
    }

    /// A subscriber connected (`true`) or lost its connection.
    fn connected(&self, up: bool) {
        let open = if up {
            self.open.fetch_add(1, Relaxed) + 1
        } else {
            self.open.fetch_sub(1, Relaxed) - 1
        };
        let live = self.live.load(Relaxed);
        if let Ok(mut g) = self.gaps.lock() {
            if open < live && g.since.is_none() {
                g.since = Some(now_ms());
            } else if open >= live
                && let Some(s) = g.since.take()
            {
                g.list.push((s, now_ms()));
            }
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

    /// A request, tried again after outage errors by `retry`; each outage
    /// error is counted under `op`.
    async fn send(
        &self,
        cl: &mut Client,
        rng: &mut Rng,
        op: &'static str,
        req: &Req<'_>,
        retry: Retry,
    ) -> Sent {
        let first = Instant::now();
        let first_send_ms = now_ms();
        let mut unknown_before = false;
        let mut n = 0;
        loop {
            n += 1;
            let started = Instant::now();
            let send_ms = now_ms();
            let last = cl.attempt(self, req).await;
            let recv_ms = now_ms();
            let code = match &last {
                Attempt::Reply(..) => None,
                Attempt::NotApplied(c) | Attempt::Unknown(c) => Some(*c),
            };
            let sent = |last, unknown_before| Sent {
                last,
                unknown_before,
                first_send_ms,
                send_ms,
                recv_ms,
                started,
            };
            let Some(code) = code else {
                if unknown_before || n > 1 {
                    self.count(|s| *s.retried.entry(op).or_default() += 1);
                }
                return sent(last, unknown_before);
            };
            self.op(op, started, Err(code.into()));
            self.outage(send_ms);
            let wait = backoff(n, rng);
            if n >= retry.attempts || first.elapsed() + wait > retry.within {
                return sent(last, unknown_before);
            }
            unknown_before |= matches!(last, Attempt::Unknown(_));
            sleep(wait).await;
        }
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

    fn order_ref(&self, n: u64) -> String {
        format!("lg-{}-{n}", self.run_id)
    }

    /// The booking number of one of this run's order references.
    fn order_of(&self, r: &str) -> Option<u64> {
        r.strip_prefix(&format!("lg-{}-", self.run_id))?
            .parse()
            .ok()
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
    let mut idx_oid = Vec::new();
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
            idx_oid.push(oid as u32);
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
        idx_oid,
        ttl_ms,
        limit,
        run_id: now_ms(),
        stats: Mutex::default(),
        history: Mutex::default(),
        violations: Mutex::default(),
        views: Mutex::default(),
        tokens: Mutex::default(),
        sampled: Mutex::default(),
        outages: Mutex::default(),
        gaps: Mutex::default(),
        in_flight: AtomicI64::new(0),
        live: AtomicI64::new(0),
        open: AtomicI64::new(0),
        order: AtomicU64::new(0),
        unbooks: AtomicI64::new(0),
        unbook_pool: Mutex::default(),
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

type Ws = WebSocketStream<TcpStream>;

/// Open subscriber `i`'s WebSocket and SUBSCRIBE from `seq`.
async fn open_ws(sh: &Shared, i: u64, seq: u64) -> Result<Ws> {
    let (source, forwarded) = sh.client(i);
    let ws = async {
        let stream = connect(sh.cfg.server, source).await?;
        let mut req = format!("ws://{}/v1/public/rt?key={}", sh.host, query_escape(&sh.pk))
            .into_client_request()?;
        if let Some(f) = forwarded {
            req.headers_mut().insert("x-forwarded-for", f.parse()?);
        }
        let config = WebSocketConfig::default()
            .read_buffer_size(4096)
            .write_buffer_size(0);
        let (mut ws, _) =
            tokio_tungstenite::client_async_with_config(req, stream, Some(config)).await?;
        let sub = wire::encode_subscribe(&Subscribe {
            version: PROTOCOL_VERSION,
            event_key: &sh.event,
            last_seq: seq,
            hold_token: None,
            session_token: None,
        })
        .map_err(|e| anyhow!("{e}"))?;
        ws.send(Message::Binary(sub.into())).await?;
        anyhow::Ok(ws)
    };
    match timeout(REQUEST_TIMEOUT, ws).await {
        Ok(r) => r,
        Err(_) => Err(anyhow!("timed out")),
    }
}

/// A subscriber: snapshot over HTTP, then the WebSocket from its `seq`,
/// applying every frame to its own view until the run stops. A dropped
/// connection (an error, silence, CLOSE 3 or 6 or an unknown reason) is
/// reconnected with the SDK's backoff and resumes from the view's `seq`.
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
    let ws = match open_ws(&sh, i, view.seq).await {
        Ok(ws) => ws,
        Err(e) => {
            let why: String = e.to_string().chars().take(60).collect();
            sh.op("connect", t, Err(why.clone()));
            sh.count(|s| s.failed += 1);
            bail!("connect: {why}");
        }
    };
    sh.op("connect", t, Ok(()));
    sh.count(|s| s.opened += 1);
    sh.live.fetch_add(1, Relaxed);
    sh.connected(true);
    let view = Arc::new(Mutex::new(view));
    if let Ok(mut v) = sh.views.lock() {
        v.push(view.clone());
    }
    tokio::spawn(follow(sh, i, probe, view.clone(), ws));
    Ok(view)
}

/// Why a connection ended.
enum Ended {
    /// The run is over.
    Stopped,
    /// Reconnect after a backoff.
    Dropped,
    /// Give up: CLOSE 1, 4 or 5, or a protocol violation.
    Final,
}

async fn follow(sh: Arc<Shared>, i: u64, probe: bool, mine: Arc<Mutex<SeatView>>, ws: Ws) {
    let mut ws = Some(ws);
    let mut rng = Rng(sh.cfg.seed ^ 0x5B5 ^ (i << 24));
    let mut stop = sh.stop.clone();
    let mut failures = 0u32;
    let mut down: Option<Instant> = None;
    let ended = loop {
        let mut conn = match ws.take() {
            Some(c) => c,
            None => {
                let wait = backoff(failures.max(1), &mut rng);
                tokio::select! {
                    _ = stop.wait_for(|s| *s) => {
                        sh.live.fetch_sub(1, Relaxed);
                        break Ended::Stopped;
                    }
                    () = sleep(wait) => {}
                }
                let seq = mine.lock().map(|v| v.seq).unwrap_or(0);
                match open_ws(&sh, i, seq).await {
                    Ok(c) => {
                        sh.count(|s| s.reconnects += 1);
                        sh.connected(true);
                        c
                    }
                    Err(_) => {
                        failures += 1;
                        continue;
                    }
                }
            }
        };
        let how = read(
            &sh,
            i,
            probe,
            &mine,
            &mut conn,
            &mut stop,
            &mut failures,
            &mut down,
        )
        .await;
        let _ = conn.close(None).await;
        match how {
            Ended::Dropped => {
                sh.connected(false);
                failures += 1;
                down.get_or_insert_with(Instant::now);
                if let Ok(mut v) = mine.lock() {
                    v.reconnecting();
                }
            }
            other => {
                sh.live.fetch_sub(1, Relaxed);
                sh.connected(false);
                break other;
            }
        }
    };
    if !matches!(ended, Ended::Stopped) {
        sh.count(|s| s.dropped += 1);
        // A view that no longer follows the server: leave it out.
        if let Ok(mut v) = sh.views.lock() {
            v.retain(|x| !Arc::ptr_eq(x, &mine));
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one connection's loop and its counters"
)]
async fn read(
    sh: &Shared,
    i: u64,
    probe: bool,
    mine: &Mutex<SeatView>,
    ws: &mut Ws,
    stop: &mut watch::Receiver<bool>,
    failures: &mut u32,
    down: &mut Option<Instant>,
) -> Ended {
    loop {
        let msg = tokio::select! {
            _ = stop.wait_for(|s| *s) => return Ended::Stopped,
            m = timeout(SILENCE, ws.next()) => m,
        };
        let Ok(Some(Ok(msg))) = msg else {
            return Ended::Dropped;
        };
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
        sh.count(|s| s.frames += 1);
        match applied {
            Ok(Applied::Hello) => {
                *failures = 0;
                if let Some(t) = down.take() {
                    sh.op("resubscribe", t, Ok(()));
                }
            }
            Ok(Applied::Pong(n)) => {
                if ws
                    .send(Message::Binary(wire::encode_pong(n).into()))
                    .await
                    .is_err()
                {
                    return Ended::Dropped;
                }
            }
            Ok(Applied::Delta { from, to }) if probe => {
                let samples: Vec<i64> = sh
                    .sampled
                    .lock()
                    .map(|m| m.range(from + 1..=to).map(|(_, t)| *t).collect())
                    .unwrap_or_default();
                sh.count(|s| {
                    for t in samples {
                        s.delta.record(u64::try_from(now - t).unwrap_or(0) * 1_000);
                    }
                });
            }
            Ok(Applied::Resync) => sh.count(|s| s.resyncs += 1),
            Ok(Applied::Close(reason)) => {
                sh.count(|s| *s.closes.entry(reason).or_default() += 1);
                // 08 §8.6: stop on 1, 4 and 5; reconnect otherwise.
                return if matches!(reason, 1 | 4 | 5) {
                    Ended::Final
                } else {
                    Ended::Dropped
                };
            }
            Ok(_) => {}
            Err(e) => {
                let detail = e.map_or("view lock poisoned".into(), |e| e.to_string());
                sh.violation("protocol", format!("subscriber {i}: {detail}"));
                return Ended::Final;
            }
        }
    }
}

type SharedView = Arc<Mutex<SeatView>>;

type Handoff = mpsc::UnboundedSender<(Party, String, Vec<u32>)>;

/// A buyer: a token, 2 to 4 seats its view shows available, a hold (another
/// pick after a refusal, twice at most), then release, booking or expiry.
async fn buy(
    sh: Arc<Shared>,
    party: Party,
    view: Option<Arc<Mutex<SeatView>>>,
    due: Instant,
    backend: Handoff,
) {
    // How late the generator started this arrival: its own lag, which the
    // token's latency (measured from `due`) includes.
    sh.op("start_lag", due, Ok(()));
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
    backend: &Handoff,
    rng: &mut Rng,
) -> Result<()> {
    let mut cl = Client::new(u64::from(party) + sh.cfg.subscribers);
    let req = Req::post(
        "/v1/public/hold-tokens".into(),
        &sh.pk,
        json!({"event": sh.event}),
    );
    let s = sh.send(&mut cl, rng, "token", &req, BUYER_RETRY).await;
    let v = match s.last {
        Attempt::Reply(201, ref v) => v.clone(),
        Attempt::Reply(_, ref v) => {
            sh.op("token", due, Err(code(v)));
            return Ok(());
        }
        _ => {
            if s.unknown() {
                sh.unknown(party, Kind::CreateHold, vec![], &s, None);
            }
            return Ok(());
        }
    };
    sh.op("token", due, Ok(()));
    let token = v["hold_token"].as_str().unwrap_or_default().to_owned();
    // Every attempt creates a token of its own, so a success's `seq` is its own.
    let created = s;
    sh.ack(
        party,
        Kind::CreateHold,
        v["seq"].as_u64().unwrap_or(0),
        vec![],
        &created,
        false,
        None,
    );
    if let Ok(mut t) = sh.tokens.lock() {
        t.insert(party, token.clone());
    }
    let expires = v["expires_at"]
        .as_str()
        .and_then(parse_rfc3339_ms)
        .or_else(|| v["expires_at"].as_i64());
    if let Some(e) = expires {
        sh.record(|h| {
            h.expiry.insert(party, e);
            h.clock.observe(
                e - sh.ttl_ms,
                e - sh.ttl_ms + 999,
                created.send_ms,
                created.recv_ms,
            );
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
    let path = format!("/v1/public/events/{}/hold", query_escape(&sh.event));
    for _ in 0..3 {
        let k = 2 + rng.below(3) as u32;
        let pick = view
            .lock()
            .ok()
            .and_then(|v| v.pick(&sh.segments, k, sh.cfg.clustered, rng));
        let Some(pick) = pick else {
            sh.op("hold", Instant::now(), Err("nothing_available".into()));
            return Ok(());
        };
        let req = Req::post(
            path.clone(),
            &sh.pk,
            json!({"hold_token": token, "objects": sh.objects(&pick)}),
        );
        let s = sh.send(&mut cl, rng, "hold", &req, BUYER_RETRY).await;
        let v = match &s.last {
            Attempt::Reply(_, v) => v.clone(),
            _ => {
                if s.unknown() {
                    sh.unknown(party, Kind::Hold, pick, &s, None);
                } else {
                    sh.failed(party, Kind::Hold, pick, None);
                }
                return Ok(());
            }
        };
        let status = match s.last {
            Attempt::Reply(st, _) => st,
            _ => 0,
        };
        // A refusal proves only that the last attempt failed: an earlier one
        // whose outcome is unknown may hold the seats.
        if status != 200 && s.unknown_before {
            sh.keep_unknown(party, Kind::Hold, pick.clone(), &s);
        }
        match (status, v["code"].as_str()) {
            (200, _) => {
                sh.op("hold", s.started, Ok(()));
                sh.op("hold_from_due", due, Ok(()));
                sh.hold_ok(s.send_ms, s.recv_ms);
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
                if !s.unknown_before
                    && let Ok(mut m) = sh.sampled.lock()
                {
                    m.insert(seq, s.recv_ms);
                }
                sh.ack(
                    party,
                    Kind::Hold,
                    seq,
                    held.clone(),
                    &s,
                    s.unknown_before,
                    None,
                );
                let fate = (rng.next() >> 11) as f64 / (1u64 << 53) as f64;
                if fate < sh.cfg.release_share {
                    sleep(Duration::from_millis(rng.below(1_000))).await;
                    release(sh, &mut cl, rng, party, &token, &held).await;
                } else if fate < sh.cfg.release_share + sh.cfg.book_share {
                    let _ = backend.send((party, token.clone(), held));
                }
                return Ok(());
            }
            (409, Some("object_not_free")) => {
                sh.op("hold", s.started, Err("object_not_free".into()));
                sh.count(|st| st.holds_refused += 1);
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
                // A refusal is all or nothing, and an earlier attempt that
                // was applied would have made this one a no-op success.
                sh.failed(party, Kind::Hold, pick, None);
                sh.record(|h| {
                    h.refusals.push(Refusal {
                        party,
                        objects: named,
                        send_ms: s.send_ms,
                        recv_ms: s.recv_ms,
                    });
                });
            }
            (422, Some("hold_limit_exceeded")) => {
                sh.op("hold", s.started, Err("hold_limit_exceeded".into()));
                sh.failed(party, Kind::Hold, pick, None);
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
            (404, Some("not_found")) => {
                sh.op("hold", s.started, Err("not_found".into()));
                sh.failed(party, Kind::Hold, pick, None);
                // The token was acknowledged; it may be gone only if expired.
                let live = sh
                    .history
                    .lock()
                    .is_ok_and(|h| h.certainly_live(party, s.send_ms));
                if live {
                    sh.violation(
                        "lost_ack",
                        format!("party {party}: its live hold token is unknown to the server"),
                    );
                }
                return Ok(());
            }
            (429, _) => {
                sh.op("hold", s.started, Err(code(&v)));
                sh.failed(party, Kind::Hold, pick, None);
                return Ok(());
            }
            (st, _) => {
                sh.op("hold", s.started, Err(code(&v)));
                sh.failed(party, Kind::Hold, pick, None);
                sh.violation(
                    "hold_reply",
                    format!("party {party}: HTTP {st} {}", code(&v)),
                );
                return Ok(());
            }
        }
    }
    Ok(())
}

async fn release(
    sh: &Arc<Shared>,
    cl: &mut Client,
    rng: &mut Rng,
    party: Party,
    token: &str,
    held: &[u32],
) {
    let path = format!("/v1/public/events/{}/release", query_escape(&sh.event));
    let req = Req::post(path, &sh.pk, json!({"hold_token": token}));
    let s = sh.send(cl, rng, "release", &req, BUYER_RETRY).await;
    match &s.last {
        Attempt::Reply(200, v) => {
            sh.op("release", s.started, Ok(()));
            let mut objects = sh.idx_of(&v["objects"]);
            objects.sort_unstable();
            if objects.is_empty() && s.unknown_before {
                // The earlier attempt was applied; its `seq` is not known.
                sh.keep_unknown(party, Kind::Release, held.to_vec(), &s);
                return;
            }
            if objects != held {
                sh.violation(
                    "release_reply",
                    format!("party {party}: released {objects:?}, held {held:?}"),
                );
            }
            // Releasing something means no earlier attempt was applied: the
            // `seq` is this one's.
            let seq = v["seq"].as_u64().unwrap_or(0);
            sh.ack(party, Kind::Release, seq, objects, &s, false, None);
        }
        Attempt::Reply(_, v) => {
            sh.op("release", s.started, Err(code(v)));
            sh.failed(party, Kind::Release, held.to_vec(), None);
            if s.unknown_before {
                sh.keep_unknown(party, Kind::Release, held.to_vec(), &s);
            }
        }
        _ if s.unknown() => sh.unknown(party, Kind::Release, held.to_vec(), &s, None),
        _ => sh.failed(party, Kind::Release, held.to_vec(), None),
    }
}

/// The backend: books with the secret key and an `Idempotency-Key`, sending a
/// share of bookings twice, which must get the same reply.
async fn book(
    sh: Arc<Shared>,
    cl: &mut Client,
    party: Party,
    token: String,
    held: Vec<u32>,
    rng: &mut Rng,
) {
    let n = sh.order.fetch_add(1, Relaxed);
    let body = json!({"hold_token": token, "order_ref": sh.order_ref(n)});
    let path = format!("/v1/events/{}/book", query_escape(&sh.event));
    let req = Req::post(path, &sh.sk, body).key(format!("lg-{}-book-{n}", sh.run_id));
    let tries = if rng.chance(sh.cfg.retry_book_share) {
        2
    } else {
        1
    };
    let mut first: Option<Value> = None;
    for _ in 0..tries {
        let s = sh.send(cl, rng, "book", &req, BACKEND_RETRY).await;
        match (&s.last, &first) {
            (Attempt::Reply(200, v), None) => {
                sh.op("book", s.started, Ok(()));
                let mut objects = sh.idx_of(&v["objects"]);
                objects.sort_unstable();
                if objects != held {
                    sh.violation(
                        "book_reply",
                        format!("party {party}: booked {objects:?}, held {held:?}"),
                    );
                }
                if !sh.cfg.unbook_after.is_zero() {
                    sh.unbooks.fetch_add(1, Relaxed);
                    tokio::spawn(unbook(sh.clone(), objects.clone(), n));
                }
                // A retry after an unknown attempt gets the original reply,
                // so the `seq` is the booking's own.
                let seq = v["seq"].as_u64().unwrap_or(0);
                first = Some(v.clone());
                sh.ack(party, Kind::Book, seq, objects, &s, false, Some(n));
            }
            (Attempt::Reply(st, v), Some(f)) => {
                sh.op("book", s.started, Ok(()));
                if *st != 200 || f["seq"] != v["seq"] || f["objects"] != v["objects"] {
                    sh.violation(
                        "idempotency",
                        format!(
                            "booking {n}: a repeat got HTTP {st} {} seq {}, the first seq {}",
                            code(v),
                            v["seq"],
                            f["seq"]
                        ),
                    );
                }
            }
            (Attempt::Reply(st, v), None) => {
                let c = code(v);
                sh.op("book", s.started, Err(c.clone()));
                sh.failed(party, Kind::Book, held.clone(), Some(n));
                // An expired hold is not found; anything else is wrong.
                if !matches!((*st, c.as_str()), (404, "not_found")) {
                    sh.violation("book_reply", format!("party {party}: HTTP {st} {c}"));
                }
                return;
            }
            (_, Some(_)) => return,
            (_, None) => {
                if s.unknown() {
                    sh.unknown(party, Kind::Book, held.clone(), &s, Some(n));
                } else {
                    sh.failed(party, Kind::Book, held.clone(), Some(n));
                }
                return;
            }
        }
    }
}

/// A cancellation: the backend releases a booking `unbook_after` after
/// making it, so that a sustained run does not fill the event.
async fn unbook(sh: Arc<Shared>, objects: Vec<u32>, n: u64) {
    sleep(sh.cfg.unbook_after).await;
    let mut rng = Rng(sh.cfg.seed ^ 0x0B00C ^ (n << 16));
    let pooled = sh.unbook_pool.lock().ok().and_then(|mut p| p.pop());
    let mut cl =
        pooled.unwrap_or_else(|| Client::new(BACKEND_ADDRESS + BACKEND_WORKERS + 2 + n % 64));
    let path = format!("/v1/events/{}/release", query_escape(&sh.event));
    let req = Req::post(path, &sh.sk, json!({ "objects": sh.objects(&objects) }))
        .key(format!("lg-{}-unbook-{n}", sh.run_id));
    let s = sh
        .send(&mut cl, &mut rng, "unbook", &req, BACKEND_RETRY)
        .await;
    match &s.last {
        Attempt::Reply(200, v) => {
            sh.op("unbook", s.started, Ok(()));
            let mut got = sh.idx_of(&v["objects"]);
            got.sort_unstable();
            if got != objects {
                sh.violation(
                    "unbook_reply",
                    format!("booking {n}: released {got:?}, booked {objects:?}"),
                );
            }
            let seq = v["seq"].as_u64().unwrap_or(0);
            sh.ack(BACKEND, Kind::Unbook, seq, got, &s, false, Some(n));
        }
        Attempt::Reply(st, v) => {
            sh.op("unbook", s.started, Err(code(v)));
            sh.failed(BACKEND, Kind::Unbook, objects.clone(), Some(n));
            sh.violation(
                "unbook_reply",
                format!("booking {n}: HTTP {st} {}", code(v)),
            );
        }
        _ if s.unknown() => sh.unknown(BACKEND, Kind::Unbook, objects.clone(), &s, Some(n)),
        _ => sh.failed(BACKEND, Kind::Unbook, objects.clone(), Some(n)),
    }
    if let Ok(mut p) = sh.unbook_pool.lock() {
        p.push(cl);
    }
    sh.unbooks.fetch_sub(1, Relaxed);
}

/// Block or unblock one seat, with an `Idempotency-Key`: the acknowledgement,
/// or `None`.
async fn change(
    sh: &Arc<Shared>,
    cl: &mut Client,
    rng: &mut Rng,
    idx: u32,
    kind: Kind,
    m: u64,
) -> bool {
    let path = format!("/v1/events/{}/change-status", query_escape(&sh.event));
    let status = if kind == Kind::Block {
        "blocked"
    } else {
        "free"
    };
    let req = Req::post(
        path,
        &sh.sk,
        json!({"objects": sh.objects(&[idx]), "status": status}),
    )
    .key(format!("lg-{}-{}-{m}", sh.run_id, kind.name()));
    let s = sh.send(cl, rng, kind.name(), &req, BACKEND_RETRY).await;
    match &s.last {
        Attempt::Reply(200, v) => {
            sh.op(kind.name(), s.started, Ok(()));
            let seq = v["seq"].as_u64().unwrap_or(0);
            sh.ack(BACKEND, kind, seq, vec![idx], &s, false, None);
            true
        }
        Attempt::Reply(_, v) => {
            sh.op(kind.name(), s.started, Err(code(v)));
            sh.failed(BACKEND, kind, vec![idx], None);
            false
        }
        _ if s.unknown() => {
            sh.unknown(BACKEND, kind, vec![idx], &s, None);
            false
        }
        _ => {
            sh.failed(BACKEND, kind, vec![idx], None);
            false
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
            let mut cl = Client::new(BACKEND_ADDRESS + w);
            loop {
                let job = rx.lock().await.recv().await;
                let Some((party, token, held)) = job else {
                    break;
                };
                book(sh.clone(), &mut cl, party, token, held, &mut rng).await;
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
    let mut cl = Client::new(BACKEND_ADDRESS + BACKEND_WORKERS);
    let mut rng = Rng(sh.cfg.seed ^ 0xB10C);
    let mut tick = tokio::time::interval(Duration::from_secs_f64(1.0 / sh.cfg.blocks_per_sec));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut m = 0;
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
        if let Some(&[idx]) = pick.as_deref() {
            m += 1;
            if change(&sh, &mut cl, &mut rng, idx, Kind::Block, m).await {
                sleep(Duration::from_millis(200)).await;
                change(&sh, &mut cl, &mut rng, idx, Kind::Unblock, m).await;
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

fn proc_status(pid: Option<u32>) -> Option<String> {
    let path = pid.map_or("/proc/self/status".to_owned(), |p| {
        format!("/proc/{p}/status")
    });
    std::fs::read_to_string(path).ok()
}

fn rss_mb(pid: Option<u32>) -> Option<f64> {
    let kb: f64 = proc_status(pid)?
        .lines()
        .find(|l| l.starts_with("VmRSS:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some(kb / 1024.0)
}

/// A process's user and system CPU time in seconds, from `/proc/PID/stat`
/// (in clock ticks of 1/100 s, Linux's `USER_HZ`).
fn cpu_s(pid: Option<u32>) -> Option<f64> {
    let path = pid.map_or("/proc/self/stat".to_owned(), |p| format!("/proc/{p}/stat"));
    let text = std::fs::read_to_string(path).ok()?;
    // Fields after the command, which is in parentheses and may hold spaces.
    let rest = text.get(text.rfind(')')? + 2..)?;
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ticks: f64 = f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?;
    Some(ticks / 100.0)
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
    let cpu0 = (
        cpu_s(None),
        sh.cfg.server_pid.and_then(|p| cpu_s(Some(p))),
        Instant::now(),
    );
    // The time series, until the run stops.
    let series = Arc::new(Mutex::new(Vec::new()));
    let sampler = {
        let (sh, series) = (sh.clone(), series.clone());
        let mut stop = sh.stop.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(sh.cfg.interval);
            let mut last = (
                cpu_s(None),
                sh.cfg.server_pid.and_then(|p| cpu_s(Some(p))),
                Instant::now(),
            );
            let mut last_hold = Hist::default();
            loop {
                tokio::select! { _ = tick.tick() => {}, _ = stop.wait_for(|s| *s) => break }
                let (holds, refused, errors, hold) = sh
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
                            h.map(|o| o.hist.clone()).unwrap_or_default(),
                        )
                    })
                    .unwrap_or_default();
                if let Ok(mut m) = sh.sampled.lock() {
                    // Probes have had 30 s to see these.
                    let old = now_ms() - 30_000;
                    m.retain(|_, t| *t > old);
                }
                let now = (
                    cpu_s(None),
                    sh.cfg.server_pid.and_then(|p| cpu_s(Some(p))),
                    Instant::now(),
                );
                let dt = (now.2 - last.2).as_secs_f64().max(1e-3);
                let interval = hold.since(&last_hold);
                last_hold = hold;
                let q =
                    |p: f64| (interval.count() > 0).then(|| interval.quantile(p) as f64 / 1_000.0);
                let cores = |a: Option<f64>, b: Option<f64>| Some((b? - a?) / dt);
                if let Ok(mut s) = series.lock() {
                    s.push(json!({
                        "t_s": started.elapsed().as_secs_f64(), "unix_ms": now_ms(),
                        "holds_ok": holds, "holds_refused": refused, "errors": errors,
                        "holds_in_interval": interval.count(),
                        "hold_p50_ms": q(0.5), "hold_p99_ms": q(0.99), "hold_max_ms": q(1.0),
                        "in_flight": sh.in_flight.load(Relaxed), "subscribers_open": sh.open.load(Relaxed),
                        "generator_rss_mb": rss_mb(None), "server_rss_mb": sh.cfg.server_pid.and_then(|p| rss_mb(Some(p))),
                        "generator_cores": cores(last.0, now.0), "server_cores": cores(last.1, now.1),
                    }));
                }
                last = now;
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
    let connect_rate = sh.cfg.connect_rate;
    let slots: Arc<Mutex<Vec<Option<SharedView>>>> = Arc::new(Mutex::new(vec![
        None;
        if stampede {
            sh.cfg.buyers as usize
        } else {
            0
        }
    ]));
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
    let deadline = Instant::now() + sh.cfg.unbook_after + BACKEND_RETRY.within + REQUEST_TIMEOUT;
    while sh.unbooks.load(Relaxed) > 0 && Instant::now() < deadline {
        sleep(Duration::from_millis(50)).await;
    }
    let cpu1 = (
        cpu_s(None),
        sh.cfg.server_pid.and_then(|p| cpu_s(Some(p))),
        Instant::now(),
    );
    sleep(sh.cfg.quiet).await;
    let converged = converge(&sh).await;
    let audit = observe(&sh).await;
    let _ = stop_tx.send(true);
    let _ = sampler.await;
    sleep(Duration::from_millis(200)).await;
    let mut violations = sh.violations.lock().map(|v| v.clone()).unwrap_or_default();
    let history = std::mem::take(
        &mut *sh
            .history
            .lock()
            .map_err(|_| anyhow!("history lock poisoned"))?,
    );
    let mut found = crate::check::check(&history);
    let audit = match audit {
        Ok(seen) => {
            let (v, resolution) = crate::check::audit(&history, &seen);
            found.extend(v);
            Some((seen.objects.len(), seen.tokens.len(), resolution))
        }
        Err(e) => {
            found.push(Violation {
                check: "audit",
                detail: format!("could not read the committed state: {e:#}"),
            });
            None
        }
    };
    violations.1 += found.len() as u64;
    let room = 100usize.saturating_sub(violations.0.len());
    violations.0.extend(found.into_iter().take(room));
    if let Some(path) = &sh.cfg.acks {
        write_acks(&sh, &history, path)?;
    }
    let stats = std::mem::take(
        &mut *sh
            .stats
            .lock()
            .map_err(|_| anyhow!("stats lock poisoned"))?,
    );
    let series = std::mem::take(&mut *series.lock().map_err(|_| anyhow!("series lock poisoned"))?);
    let outages = sh
        .outages
        .lock()
        .map(|o| o.list.clone())
        .unwrap_or_default();
    let gaps = sh.gaps.lock().map(|g| g.list.clone()).unwrap_or_default();
    let delta = |a: Option<f64>, b: Option<f64>| Some(b? - a?);
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
        outages,
        gaps,
        cpu: Cpu {
            generator_s: delta(cpu0.0, cpu1.0).unwrap_or(0.0),
            server_s: delta(cpu0.1, cpu1.1),
            wall_s: (cpu1.2 - cpu0.2).as_secs_f64(),
        },
        audit,
        unknown: history.unknown.len(),
    })
}

/// Every acknowledgement whose `seq` is its own, as `seq,op,oids` (CSV,
/// `oids` a PostgreSQL array), for `\copy` and a join with `event_log`.
fn write_acks(sh: &Shared, h: &History, path: &str) -> Result<()> {
    let mut out = String::from("seq,op,oids\n");
    let mut acks: Vec<&Ack> = h.acks.iter().filter(|a| !a.retried).collect();
    acks.sort_by_key(|a| a.seq);
    for a in acks {
        let mut oids: Vec<u32> = a
            .objects
            .iter()
            .filter_map(|&i| sh.idx_oid.get(i as usize).copied())
            .collect();
        oids.sort_unstable();
        let list: Vec<String> = oids.iter().map(|o| (*o as i32).to_string()).collect();
        out.push_str(&format!(
            "{},{},\"{{{}}}\"\n",
            a.seq,
            a.kind.op(),
            list.join(",")
        ));
    }
    std::fs::write(path, out).with_context(|| format!("cannot write {path}"))
}

/// Every live subscriber's view must equal a fresh snapshot once it reaches
/// that snapshot's `seq`. Returns (compared and equal, differing, never
/// caught up or always overtaken by later commits).
async fn converge(sh: &Arc<Shared>) -> (u64, u64, u64) {
    let views = sh.views.lock().map(|v| v.clone()).unwrap_or_default();
    let mut pending: Vec<usize> = (0..views.len()).collect();
    let (mut equal, mut differ) = (0, 0);
    let mut rng = Rng(sh.cfg.seed ^ 0xC0);
    let deadline = Instant::now() + Duration::from_secs(60);
    while !pending.is_empty() && Instant::now() < deadline {
        let path = format!(
            "/v1/public/events/{}/state?key={}",
            query_escape(&sh.event),
            query_escape(&sh.pk)
        );
        let Ok(mut c) = sh.http(BACKEND_ADDRESS + BACKEND_WORKERS + 1).await else {
            sleep(backoff(1, &mut rng)).await;
            continue;
        };
        let Ok(Ok(r)) = timeout(REQUEST_TIMEOUT, c.request("GET", &path, &[], None)).await else {
            sleep(backoff(1, &mut rng)).await;
            continue;
        };
        let truth = match wire::decode_server(&r.body, View::Public) {
            Ok(ServerFrame::Snapshot(snap)) if r.status == 200 => {
                let mut t = SeatView::new(sh.objects);
                if t.load(&snap).is_err() {
                    break;
                }
                t
            }
            _ => {
                sleep(backoff(1, &mut rng)).await;
                continue;
            }
        };
        let wait = (Instant::now() + Duration::from_secs(10)).min(deadline);
        while Instant::now() < wait
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
    }
    (equal, differ, pending.len() as u64)
}

/// Read the committed state for the audit: every object not free (the
/// objects query), and the hold of every token that may still hold
/// something.
async fn observe(sh: &Arc<Shared>) -> Result<Observed> {
    let start_ms = now_ms();
    let mut cl = Client::new(BACKEND_ADDRESS + BACKEND_WORKERS + 1);
    let mut rng = Rng(sh.cfg.seed ^ 0xA0D);
    let mut objects = HashMap::new();
    for status in ["held", "booked", "blocked"] {
        let mut after: Option<String> = None;
        loop {
            let mut path = format!(
                "/v1/events/{}/objects?status={status}&limit=100",
                query_escape(&sh.event)
            );
            if let Some(a) = &after {
                path.push_str(&format!("&after={}", query_escape(a)));
            }
            let req = Req {
                method: "GET",
                path,
                auth: &sh.sk,
                headers: vec![],
                body: None,
            };
            let s = sh
                .send(&mut cl, &mut rng, "audit", &req, BACKEND_RETRY)
                .await;
            let Attempt::Reply(200, page) = s.last else {
                bail!("listing {status} objects: {:?}", s.last);
            };
            for o in page["data"].as_array().into_iter().flatten() {
                let Some(&idx) = o["oid"]
                    .as_u64()
                    .and_then(|oid| sh.oid_idx.get(&(oid as u32)))
                else {
                    bail!("the objects query named an object not in the chart");
                };
                let seen = match o["status"].as_str() {
                    Some("held") => Seen::Held(
                        o["expires_at"]
                            .as_str()
                            .and_then(parse_rfc3339_ms)
                            .unwrap_or(0),
                    ),
                    Some("booked") => {
                        Seen::Booked(o["order_ref"].as_str().and_then(|r| sh.order_of(r)))
                    }
                    Some("blocked") => Seen::Blocked,
                    _ => Seen::Other,
                };
                objects.insert(idx, seen);
            }
            match page["next"].as_str() {
                Some(n) => after = Some(n.to_owned()),
                None => break,
            }
        }
    }
    let parties = sh
        .history
        .lock()
        .map(|h| h.parties_to_read(start_ms))
        .unwrap_or_default();
    let tokens = sh.tokens.lock().map(|t| t.clone()).unwrap_or_default();
    // Sixteen readers, each with its own connection and share of the parties.
    let mut readers = tokio::task::JoinSet::new();
    for w in 0..16u64 {
        let mine: Vec<(Party, String)> = parties
            .iter()
            .enumerate()
            .filter(|(i, _)| *i as u64 % 16 == w)
            .filter_map(|(_, p)| tokens.get(p).map(|t| (*p, t.clone())))
            .collect();
        let sh = sh.clone();
        readers.spawn(async move {
            let mut cl = Client::new(BACKEND_ADDRESS + BACKEND_WORKERS + 100 + w);
            let mut rng = Rng(sh.cfg.seed ^ 0xA0D ^ (w << 8));
            let mut read = Vec::new();
            for (p, token) in mine {
                let req = Req {
                    method: "GET",
                    path: format!("/v1/hold-tokens/{token}"),
                    auth: &sh.sk,
                    headers: vec![],
                    body: None,
                };
                let s = sh
                    .send(&mut cl, &mut rng, "audit", &req, BACKEND_RETRY)
                    .await;
                match s.last {
                    Attempt::Reply(200, v) => {
                        let mut held = sh.idx_of(&v["objects"]);
                        held.sort_unstable();
                        read.push((p, Some(held)));
                    }
                    Attempt::Reply(404, _) => read.push((p, None)),
                    other => bail!("reading a hold token: {other:?}"),
                }
            }
            anyhow::Ok(read)
        });
    }
    let mut read = HashMap::new();
    while let Some(r) = readers.join_next().await {
        read.extend(r.context("a token reader failed")??);
    }
    Ok(Observed {
        objects,
        tokens: read,
        start_ms,
        end_ms: now_ms(),
    })
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

    #[test]
    fn backoff_follows_the_sdk() {
        let mut rng = super::Rng(3);
        for (n, base) in [(1, 500), (2, 1_000), (3, 2_000), (7, 30_000), (20, 30_000)] {
            for _ in 0..50 {
                let ms = super::backoff(n, &mut rng).as_millis() as u64;
                assert!((base / 2..=base).contains(&ms), "{n}: {ms}");
            }
        }
    }
}
