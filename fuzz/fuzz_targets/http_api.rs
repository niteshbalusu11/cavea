//! Requests generated from the OpenAPI document into the server's router, in
//! process, over a PostgreSQL database of the run's own (13 §5). The router's
//! handlers query PostgreSQL directly, so a database it must be; there is no
//! socket, and nothing else is shared.
//!
//! Each input runs in a world of its own: two fresh workspaces, A and B, made
//! through the API with the same chart and event keys (each also has keys of
//! its own), a publishable key, a revoked secret key, a published chart, an
//! archived chart, events (one requiring session tokens, one archived) and a
//! hold token holding a seat. Then the input's records (`cavea_fuzz::http`),
//! each one request: an operation of the document; a credential (mostly the
//! right one; also the other workspace's, none, malformed, unknown, revoked,
//! the wrong kind); path, query and header values drawn mostly from the
//! world (its keys, labels, hold tokens, the draft's etag, signed session
//! tokens, three idempotency keys so replays and mismatches happen) and
//! sometimes from the input's bytes; a body generated from the operation's
//! schema, with fields named like the world's things taken from it, and now
//! and then a field dropped, added or of the wrong type, or raw bytes.
//!
//! Every answer must satisfy, else the run stops on a panic:
//! - no panic anywhere in the process (libFuzzer's hook aborts on any, also
//!   one the router's panic layer would answer `500`, or an actor's) and no
//!   `5xx`;
//! - the status is one the document lists for the operation, and the body
//!   validates against its schema (`cavea_server::http::openapi::check`, the
//!   router tests' own check); a JSON body answered `2xx` validates against
//!   the request schema;
//! - a `2xx` only with a credential the operation accepts: a live key of the
//!   right kind, or the admin key; never with none, a malformed, unknown or
//!   revoked one, the wrong kind, or a cookie route without its cookie; on a
//!   route that takes a session token, never with an expired one, one signed
//!   by a revoked key or the other workspace's, garbage, one for another
//!   event, or none for an event that requires one;
//! - nothing of the other workspace in an answer to a workspace's credential:
//!   its id, key ids, keys, hold tokens and names (each name carries the
//!   workspace's marker), unless the request itself carried it;
//! - an answer within libFuzzer's `-timeout`.
//!
//! The engine runs on the system clock: expiry is also decided in SQL on the
//! database's clock, and a frozen engine clock would disagree with it.

#![no_main]

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, header};
use cavea_fuzz::http::{Operation, Place, operations};
use cavea_server::http::openapi::check;
use hmac::{Hmac, KeyInit as _, Mac as _};
use libfuzzer_sys::fuzz_target;
use serde_json::{Map, Value, json};
use sha2::Sha256;
use sqlx::postgres::PgConnectOptions;
use sqlx::{AssertSqlSafe, ConnectOptions as _, Connection as _, Executor as _, PgConnection};
use tower::ServiceExt as _;

/// Obviously fake; 32 characters at least.
const ADMIN: &str = "fuzz-admin-key-not-a-real-secret-0000";
/// Most requests one input makes.
const MAX_REQUESTS: usize = 12;
/// What a workspace's names carry: never produced from an input's bytes,
/// which have it removed (`text`).
const MARKS: [char; 2] = ['\u{2603}', '\u{2602}'];
/// A 1 × 1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64, 0x60, 0xf8, 0x5f,
    0x0f, 0x00, 0x02, 0x87, 0x01, 0x80, 0xeb, 0x47, 0xba, 0x92, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

// ------------------------------------------------------------------ choices

/// The bytes of one record, read in order; past the end, 0.
struct Choices<'a> {
    bytes: &'a [u8],
}

impl Choices<'_> {
    fn byte(&mut self) -> u8 {
        match self.bytes.split_first() {
            Some((&b, rest)) => {
                self.bytes = rest;
                b
            }
            None => 0,
        }
    }

    /// One of `n`, 0 when the bytes ran out.
    fn pick(&mut self, n: usize) -> usize {
        usize::from(self.byte()) % n.max(1)
    }

    /// True about once in `n` bytes, never on 0.
    fn rare(&mut self, n: u8) -> bool {
        let b = self.byte();
        b != 0 && b.is_multiple_of(n)
    }

    /// Up to 32 of the record's bytes, length first.
    fn raw(&mut self) -> Vec<u8> {
        let n = usize::from(self.byte() % 33);
        let take = n.min(self.bytes.len());
        let (head, rest) = self.bytes.split_at(take);
        self.bytes = rest;
        head.to_vec()
    }

    /// Text from the record's bytes, without the workspaces' marks.
    fn text(&mut self) -> String {
        String::from_utf8_lossy(&self.raw())
            .chars()
            .filter(|c| !MARKS.contains(c))
            .collect()
    }

    fn number(&mut self) -> i64 {
        const INTERESTING: [i64; 14] = [
            1,
            0,
            -1,
            2,
            59,
            60,
            100,
            101,
            999,
            1_000,
            7_200,
            7_201,
            1 << 31,
            9_007_199_254_740_993,
        ];
        INTERESTING[self.pick(INTERESTING.len())]
    }
}

// ------------------------------------------------------------------ the world

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cred {
    /// The operation's right credential of workspace `t`.
    Right(usize),
    None,
    Malformed(u8),
    /// A secret key never issued.
    Unknown,
    Revoked(usize),
    Admin,
    Secret(usize),
    Publishable(usize),
}

#[derive(Debug, Default)]
struct Tenant {
    mark: char,
    ws: String,
    secret: String,
    secret_id: String,
    publishable: String,
    publishable_id: String,
    revoked: String,
    revoked_id: String,
    hash: String,
    etag: String,
    labels: Vec<String>,
    tokens: Vec<String>,
    /// A chart and an event only this workspace has.
    own_chart: String,
    own_event: String,
    /// The events known to require a session token (08 §11): `e2`, and any
    /// an answer has since shown with `require_session`.
    requires_session: Vec<String>,
}

impl Tenant {
    fn name(&self, text: &str) -> String {
        format!("{text}{}", self.mark)
    }

    /// What must never reach the other workspace.
    fn secrets(&self) -> Vec<String> {
        let mut out = vec![
            self.ws.clone(),
            self.secret.clone(),
            self.secret_id.clone(),
            self.publishable.clone(),
            self.publishable_id.clone(),
            self.revoked.clone(),
            self.revoked_id.clone(),
            self.mark.to_string(),
        ];
        out.extend(self.tokens.iter().cloned());
        out
    }
}

struct World {
    /// The world's number in the run, which makes what the admin key
    /// creates (workspaces, whose keys are global) its own.
    tag: u32,
    t: [Tenant; 2],
    /// Session tokens of each workspace's secret key for its events: valid,
    /// expired, wrong event; then garbage.
    sessions: [Vec<(String, Session)>; 2],
}

struct Harness {
    runtime: tokio::runtime::Runtime,
    app: Router,
    doc: Value,
    ops: Vec<Operation>,
    /// A connection to the run's database for as long as the process
    /// lives: while it is open, no other run takes the database for stale.
    _owner: std::sync::Mutex<PgConnection>,
    next: AtomicU32,
    /// Worlds made ahead, each used by one input only.
    ready: std::sync::Mutex<Vec<World>>,
}

/// The run's database, dropped at exit.
static DATABASE: OnceLock<(PgConnectOptions, String)> = OnceLock::new();

extern "C" fn drop_database() {
    let Some((server, name)) = DATABASE.get() else {
        return;
    };
    // On a thread of its own: at exit, this thread's runtime context is gone.
    let drop = std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        runtime.block_on(async {
            if let Ok(mut conn) = PgConnection::connect_with(server).await {
                // The name is generated by `setup`, never input.
                let _ = conn
                    .execute(AssertSqlSafe(format!(
                        "DROP DATABASE IF EXISTS {name} WITH (FORCE)"
                    )))
                    .await;
            }
        });
    });
    let _ = drop.join();
}

fn harness() -> &'static Harness {
    static HARNESS: OnceLock<Harness> = OnceLock::new();
    HARNESS.get_or_init(setup)
}

fn setup() -> Harness {
    let url = std::env::var("CAVEA_FUZZ_DATABASE_URL").expect(
        "set CAVEA_FUZZ_DATABASE_URL to a PostgreSQL server URL, such as \
         postgres://cavea:cavea@127.0.0.1:5432/cavea: the run creates and drops a database of its own",
    );
    let server: PgConnectOptions = url.parse().expect("CAVEA_FUZZ_DATABASE_URL parses");
    let server = server.disable_statement_logging();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let name = format!(
        "cavea_fuzz_{}_{}_{}",
        now.as_secs(),
        std::process::id(),
        now.subsec_nanos()
    );
    let (pool, owner) = runtime.block_on(async {
        let mut conn = PgConnection::connect_with(&server)
            .await
            .expect("PostgreSQL answers");
        // A run that ended without `exit` (a finding, a timeout, an
        // interrupt) leaves its database. A live run keeps a connection to
        // its own from a moment after creating it until it ends, so a
        // database of this target's name that is over a minute old and that
        // nothing is connected to is stale, whichever host or process
        // namespace made it.
        let unused: Vec<(String,)> = sqlx::query_as(
            "SELECT datname FROM pg_database d WHERE datname LIKE 'cavea\\_fuzz\\_%'
               AND NOT EXISTS (SELECT FROM pg_stat_activity a WHERE a.datname = d.datname)",
        )
        .fetch_all(&mut conn)
        .await
        .expect("databases listed");
        for (db,) in unused {
            let made = db.split('_').nth(2).and_then(|p| p.parse::<u64>().ok());
            if made.is_some_and(|made| made + 60 < now.as_secs())
                && db.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                let _ = conn
                    .execute(AssertSqlSafe(format!(
                        "DROP DATABASE IF EXISTS {db} WITH (FORCE)"
                    )))
                    .await;
            }
        }
        conn.execute(AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .await
            .expect("database created");
        // Held until the process ends: what marks the database as in use.
        let owner = PgConnection::connect_with(&server.clone().database(&name))
            .await
            .expect("the run's database answers");
        // Commits need not wait for the disk: the run's data is thrown away.
        conn.execute(AssertSqlSafe(format!(
            "ALTER DATABASE {name} SET synchronous_commit = off"
        )))
        .await
        .expect("database configured");
        let _ = DATABASE.set((server.clone(), name.clone()));
        // SAFETY: registering a plain `extern "C"` function with no
        // arguments, as `atexit` expects; it runs once, at exit.
        unsafe {
            libc::atexit(drop_database);
        }
        let options = server.clone().database(&name);
        cavea_server::store::migrate(&options)
            .await
            .expect("migrations apply");
        let pool = cavea_server::store::connect(options, 4)
            .await
            .expect("the pool connects");
        (pool, owner)
    });
    let config = cavea_server::config::Config {
        listen: ([127, 0, 0, 1], 0).into(),
        log: "off".to_owned(),
        log_format: cavea_telemetry::LogFormat::Json,
        database_url: url.parse().expect("the URL parses"),
        db_pool: std::num::NonZeroU32::new(4).expect("not zero"),
        node_id: "fuzz".to_owned(),
        lease_secs: std::num::NonZeroU32::new(15).expect("not zero"),
        heartbeat_secs: std::num::NonZeroU32::new(5).expect("not zero"),
        admin_key: Some(ADMIN.parse().expect("the admin key parses")),
        cors_origins: cavea_server::config::CorsOrigins::Any,
        snapshot_max_age: 1,
        rt_max_connections: std::num::NonZeroU32::new(100).expect("not zero"),
        rt_max_per_ip: std::num::NonZeroU32::new(10).expect("not zero"),
        trusted_proxies: cavea_server::net::TrustedProxies::default(),
        rate_limit_holds: 120,
        rate_limit_reads: 3_000,
        asset_max_bytes: std::num::NonZeroU32::new(1 << 20).expect("not zero"),
        public_url: None,
        console_cookie_secure: true,
        rate_limit_sign_in: 10,
    };
    let state = runtime.block_on(async {
        cavea_server::http::AppState {
            seats: cavea_server::seats::Seats::new(
                pool.clone(),
                &config,
                cavea_server::seats::ServerClock::System,
                None,
            ),
            realtime: cavea_server::realtime::Realtime::new(
                100,
                10,
                cavea_server::realtime::Timing::default(),
            ),
            limiter: std::sync::Arc::new(cavea_server::limiter::Limiter::new(120, 3_000)),
            asset_memory: cavea_server::assets::Memory::new(1 << 20),
            sign_in: std::sync::Arc::default(),
            config: std::sync::Arc::new(config),
            db: pool,
            health: cavea_server::http::Health::default(),
            console: cavea_server::http::web::Files::new(&[]),
        }
    });
    let doc = cavea_server::http::openapi::document();
    Harness {
        runtime,
        app: cavea_server::http::router(state),
        ops: operations(&doc),
        doc,
        _owner: std::sync::Mutex::new(owner),
        next: AtomicU32::new(0),
        ready: std::sync::Mutex::default(),
    }
}

// ------------------------------------------------------------------ requests

/// A request as built, with what the checks need.
struct Built {
    method: String,
    uri: String,
    headers: Vec<(String, String)>,
    body: Option<(Vec<u8>, &'static str)>,
    cred: Cred,
    /// The operation the request was built for, unless its method or path
    /// was changed.
    op: Option<usize>,
    /// The `Cavea-Session` header sent, as the oracle sees it.
    session: Session,
    /// The event the request names, in its path or its body's `event`.
    event: Option<String>,
}

/// A session token as sent (08 §11: verified whenever sent).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Session {
    None,
    /// Signed by the credential's workspace with a live key, unexpired, for
    /// this event.
    For(String),
    /// Expired, signed by a revoked key or another workspace's, or garbage.
    Invalid,
    /// Not known to be either (an empty header).
    Unknown,
}

impl Built {
    fn new(method: &str, uri: String, cred: Cred) -> Self {
        Self {
            method: method.to_owned(),
            uri,
            headers: Vec::new(),
            body: None,
            cred,
            op: None,
            session: Session::None,
            event: None,
        }
    }

    fn json(mut self, body: &Value) -> Self {
        self.body = Some((body.to_string().into_bytes(), "application/json"));
        self
    }
}

/// Percent-encodes everything but unreserved characters.
fn encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]));
        }
    }
    out
}

/// A session token (08 §11) signed with `key`.
fn session(key: &str, kid: &str, event: &str, exp: i64) -> String {
    let head = base64url(json!({ "alg": "HS256", "kid": kid }).to_string().as_bytes());
    let body = base64url(
        json!({ "sub": "buyer", "event": event, "exp": exp })
            .to_string()
            .as_bytes(),
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("any key length");
    mac.update(format!("{head}.{body}").as_bytes());
    let sig = base64url(&mac.finalize().into_bytes());
    format!("{head}.{body}.{sig}")
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// An answer as received.
struct Answer {
    status: u16,
    body: Vec<u8>,
    json: Value,
    etag: Option<String>,
}

/// The workspace whose key a credential is.
fn acting(cred: Cred) -> Option<usize> {
    match cred {
        Cred::Secret(t) | Cred::Publishable(t) => Some(t),
        _ => None,
    }
}

/// Sends `req` and checks the answer (module docs).
async fn send(h: &Harness, world: &World, req: Built) -> Answer {
    let mut builder = Request::builder().method(req.method.as_str()).uri(&req.uri);
    for (name, value) in &req.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    let (bytes, content_type) = match &req.body {
        Some((bytes, ty)) => (bytes.clone(), Some(*ty)),
        None => (Vec::new(), None),
    };
    if let Some(ty) = content_type {
        builder = builder.header(header::CONTENT_TYPE, ty);
    }
    let Ok(request) = builder.body(Body::from(bytes.clone())) else {
        // Not an HTTP request (a header value the http crate refuses): no
        // client can send it.
        return Answer {
            status: 0,
            body: Vec::new(),
            json: Value::Null,
            etag: None,
        };
    };
    let path = request.uri().path().to_owned();
    let response = h
        .app
        .clone()
        .oneshot(request)
        .await
        .unwrap_or_else(|e| match e {});
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let ty = headers
        .get(header::CONTENT_TYPE)
        .and_then(|t| t.to_str().ok())
        .map(str::to_owned);
    let etag = headers
        .get(header::ETAG)
        .and_then(|t| t.to_str().ok())
        .map(str::to_owned);
    // A stream does not end: stop at its head.
    let body = if ty.as_deref() == Some("text/event-stream") {
        Vec::new()
    } else {
        axum::body::to_bytes(response.into_body(), 1 << 26)
            .await
            .map(|b| b.to_vec())
            .unwrap_or_default()
    };
    let at = format!("{} {} → {status}", req.method, req.uri);
    assert!(
        status < 500,
        "{at}: {}",
        String::from_utf8_lossy(&body[..body.len().min(500)])
    );

    // The document's view of the exchange.
    let matched = check::operation(&req.method, &path);
    let sent_json = content_type == Some("application/json")
        && matched.is_some_and(|(_, op)| {
            op.pointer("/requestBody/content/application~1json")
                .is_some()
        });
    if let Err(e) = check::exchange(
        &req.method,
        &path,
        sent_json.then_some(bytes.as_slice()),
        status,
        ty.as_deref(),
        &body,
    ) {
        panic!("{at}: the OpenAPI document disagrees: {e}");
    }

    // A success only with an accepted credential.
    if (200..300).contains(&status)
        && let Some((template, op)) = matched
    {
        let alternatives: Vec<Vec<&str>> = op
            .get("security")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .map(|a| a.keys().map(String::as_str).collect())
            .collect();
        let query_key = |t: usize| req.uri.contains(&format!("key={}", world.t[t].publishable));
        let met = |scheme: &str| match scheme {
            "secretKey" => matches!(req.cred, Cred::Secret(_)),
            "publishableKey" => matches!(req.cred, Cred::Publishable(_)),
            "publishableKeyQuery" => query_key(0) || query_key(1),
            "adminKey" => req.cred == Cred::Admin,
            "consoleSession" => false,
            // Not keys: what they guard is the handlers' to check.
            _ => true,
        };
        let allowed =
            alternatives.is_empty() || alternatives.iter().any(|a| a.iter().all(|s| met(s)));
        assert!(allowed, "{at}: {template} succeeded with {:?}", req.cred);
        // A session token is verified whenever sent, and required by an
        // event that sets `require_session` (08 §11).
        let takes_session = alternatives.iter().flatten().any(|s| *s == "sessionToken");
        if let (true, Some(t), Some(_)) = (takes_session, acting(req.cred), req.op) {
            let target = req.event.as_deref();
            let ok = match &req.session {
                Session::None => {
                    target.is_none_or(|e| !world.t[t].requires_session.iter().any(|r| r == e))
                }
                Session::For(e) => target.is_none_or(|target| target == e),
                Session::Invalid => false,
                Session::Unknown => true,
            };
            assert!(
                ok,
                "{at}: {template} succeeded with session {:?} for event {target:?}",
                req.session
            );
        }
    }

    // Nothing of the other workspace.
    if let Some(t) = acting(req.cred)
        && matched.is_some_and(|(_, op)| {
            op.get("security")
                .and_then(Value::as_array)
                .is_some_and(|s| !s.is_empty())
        })
    {
        let answer = String::from_utf8_lossy(&body);
        let asked = format!(
            "{} {:?} {}",
            req.uri,
            req.headers,
            String::from_utf8_lossy(&bytes)
        );
        for secret in world.t[1 - t].secrets() {
            assert!(
                secret.is_empty() || !answer.contains(&secret) || asked.contains(&secret),
                "{at}: workspace {t}'s answer holds the other's {secret:?}"
            );
        }
    }

    let json = serde_json::from_slice(&body).unwrap_or(Value::Null);
    Answer {
        status,
        body,
        json,
        etag,
    }
}

/// Sends a request the world needs and expects `status`.
async fn must(h: &Harness, world: &World, req: Built, status: u16) -> Value {
    let at = format!("{} {}", req.method, req.uri);
    let answer = send(h, world, req).await;
    assert_eq!(
        answer.status,
        status,
        "seeding {at}: {}",
        String::from_utf8_lossy(&answer.body)
    );
    answer.json
}

fn text_of(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// The seeded chart: two sections of a row of four seats each (small, so
/// that publishing it, brotli included, is quick).
const SEED_CHART: &str = r##"{"schema":1,"name":"seed","nextOid":300,
  "categories":[{"key":"a","label":"A","color":"#2563EB"}],
  "nodes":[
    {"id":10,"type":"section","label":"L","category":"a","children":[{"id":11,"type":"rowBlock","rows":[{"rid":12,"seats":4,"oids":[[100,4]]}]}]},
    {"id":20,"type":"section","label":"R","category":"a","origin":[600,0],"children":[{"id":21,"type":"rowBlock","rows":[{"rid":22,"seats":4,"oids":[[200,4]]}]}]}]}"##;

/// How many worlds are made at once: their database round trips overlap.
const BATCH: usize = 8;

/// A world no input has used.
async fn fresh(h: &'static Harness) -> World {
    let ready = h.ready.lock().ok().and_then(|mut r| r.pop());
    if let Some(world) = ready {
        return world;
    }
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..BATCH {
        set.spawn(world(h));
    }
    let mut made = set.join_all().await;
    let world = made.pop().expect("a batch makes worlds");
    if let Ok(mut r) = h.ready.lock() {
        r.extend(made);
    }
    world
}

/// Two workspaces with everything a request may name (module docs), made
/// at once.
async fn world(h: &Harness) -> World {
    let n = h.next.fetch_add(1, Ordering::Relaxed);
    let (a, b) = tokio::join!(tenant(h, n, 0), tenant(h, n, 1));
    World {
        tag: n,
        sessions: [a.1, b.1],
        t: [a.0, b.0],
    }
}

/// Workspace `t` of world `n`, and its session tokens.
async fn tenant(h: &Harness, n: u32, t: usize) -> (Tenant, Vec<(String, Session)>) {
    // Seeding is checked like any request, but not for isolation: the other
    // workspace is being made at the same time.
    let none = World {
        tag: n,
        t: [Tenant::default(), Tenant::default()],
        sessions: [Vec::new(), Vec::new()],
    };
    let world = &none;
    let mark = MARKS[t];
    let name = |text: &str| format!("{text}{mark}");
    let mut admin = Built::new("POST", "/v1/workspaces".to_owned(), Cred::Admin);
    admin
        .headers
        .push(("authorization".into(), format!("Bearer {ADMIN}")));
    let ws = must(
        h,
        world,
        admin.json(&json!({ "key": format!("fz{n}-{t}"), "name": name("ws") })),
        201,
    )
    .await;
    let secret = text_of(&ws, "/secret_key/key");
    let call = |method: &str, uri: &str| {
        let mut b = Built::new(method, uri.to_owned(), Cred::Secret(t));
        b.headers
            .push(("authorization".into(), format!("Bearer {secret}")));
        b
    };
    let own_chart = format!("chart-{t}");
    let own_event = format!("event-{t}");
    let pk = must(
        h,
        world,
        call("POST", "/v1/keys").json(&json!({ "kind": "publishable", "name": name("web") })),
        201,
    )
    .await;
    let revoked = must(
        h,
        world,
        call("POST", "/v1/keys").json(&json!({ "kind": "secret", "name": name("old") })),
        201,
    )
    .await;
    let revoked_id = text_of(&revoked, "/id");
    must(
        h,
        world,
        call("DELETE", &format!("/v1/keys/{revoked_id}")),
        200,
    )
    .await;
    let mut document: Value = serde_json::from_str(SEED_CHART).expect("the chart parses");
    document["name"] = json!(name("chart"));
    let chart = must(
        h,
        world,
        call("POST", "/v1/charts")
            .json(&json!({ "key": "c1", "name": name("c1"), "document": document })),
        201,
    )
    .await;
    let version = must(h, world, call("POST", "/v1/charts/c1/publish"), 201).await;
    for key in [own_chart.as_str(), "c0"] {
        must(
            h,
            world,
            call("POST", "/v1/charts").json(&json!({ "key": key, "name": name("draft") })),
            201,
        )
        .await;
    }
    must(h, world, call("DELETE", "/v1/charts/c0"), 200).await;
    let events = [
        json!({ "key": "e1", "chart": "c1", "name": name("e1") }),
        json!({ "key": "e2", "chart": "c1", "settings": { "require_session": true, "max_objects": 3 } }),
        json!({ "key": own_event, "chart": "c1" }),
        json!({ "key": "e0", "chart": "c1" }),
    ];
    for body in &events {
        must(h, world, call("POST", "/v1/events").json(body), 201).await;
    }
    must(h, world, call("DELETE", "/v1/events/e0"), 200).await;
    let objects = must(h, world, call("GET", "/v1/events/e1/objects"), 200).await;
    let labels: Vec<String> = objects
        .pointer("/data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|o| o.get("label").and_then(Value::as_str).map(str::to_owned))
        .collect();
    let token = must(
        h,
        world,
        call("POST", "/v1/hold-tokens").json(&json!({ "event": "e1" })),
        201,
    )
    .await;
    let token = text_of(&token, "/hold_token");
    let first = labels.first().cloned().unwrap_or_default();
    must(
        h,
        world,
        call("POST", "/v1/events/e1/hold")
            .json(&json!({ "hold_token": token, "objects": [first] })),
        200,
    )
    .await;
    let tenant = Tenant {
        mark,
        ws: text_of(&ws, "/id"),
        secret_id: text_of(&ws, "/secret_key/id"),
        publishable: text_of(&pk, "/key"),
        publishable_id: text_of(&pk, "/id"),
        revoked: text_of(&revoked, "/key"),
        revoked_id,
        hash: text_of(&version, "/hash"),
        etag: text_of(&chart, "/draft_etag"),
        labels,
        tokens: vec![token],
        own_chart,
        own_event,
        requires_session: vec!["e2".to_owned()],
        secret: secret.clone(),
    };
    let exp = now() + 600;
    let valid = |event: &str| Session::For(event.to_owned());
    let sessions = vec![
        (
            session(&tenant.secret, &tenant.secret_id, "e2", exp),
            valid("e2"),
        ),
        (
            session(&tenant.secret, &tenant.secret_id, "e1", exp),
            valid("e1"),
        ),
        (
            session(&tenant.secret, &tenant.secret_id, "e2", now() - 3_600),
            Session::Invalid,
        ),
        (
            session(&tenant.secret, &tenant.secret_id, "nope", exp),
            valid("nope"),
        ),
        (
            session(&tenant.revoked, &tenant.revoked_id, "e2", exp),
            Session::Invalid,
        ),
    ];
    (tenant, sessions)
}

/// The value of path parameter `name` for workspace `t`.
fn path_value(c: &mut Choices<'_>, world: &World, t: usize, name: &str) -> String {
    let me = &world.t[t];
    let other = &world.t[1 - t];
    let pick = c.pick(8);
    if pick == 7 {
        return c.text();
    }
    let options: Vec<String> = match name {
        "event" => vec![
            "e1".into(),
            "e2".into(),
            "e0".into(),
            me.own_event.clone(),
            other.own_event.clone(),
            "none".into(),
            String::new(),
        ],
        "chart" => vec![
            "c1".into(),
            me.own_chart.clone(),
            "c0".into(),
            other.own_chart.clone(),
            "none".into(),
            "x".repeat(129),
        ],
        "token" => {
            let mut v = me.tokens.clone();
            v.extend(other.tokens.first().cloned());
            v.push(format!("ht_{}", "0".repeat(64)));
            v.push("ht_short".into());
            v
        }
        "id" => vec![
            me.publishable_id.clone(),
            me.secret_id.clone(),
            me.revoked_id.clone(),
            other.publishable_id.clone(),
            format!("key_{}", "0".repeat(32)),
            "key_x".into(),
        ],
        "workspace" => vec![
            me.ws.clone(),
            other.ws.clone(),
            format!("ws_{}", "0".repeat(32)),
            "ws_".into(),
        ],
        "hash" => vec![
            me.hash.clone(),
            other.hash.clone(),
            "0".repeat(32),
            me.hash.to_uppercase(),
        ],
        "file" => vec![
            format!("{}.ccf", me.hash),
            format!("{}.ccf", other.hash),
            me.hash.clone(),
            format!("{}.full.ccf", me.hash),
        ],
        _ => vec![
            format!("usr_{}", "0".repeat(32)),
            format!("inv_{}", "0".repeat(32)),
        ],
    };
    options[pick % options.len()].clone()
}

/// A query value for parameter `name`.
fn query_values(c: &mut Choices<'_>, world: &World, t: usize, name: &str) -> Vec<String> {
    let me = &world.t[t];
    let label = |c: &mut Choices<'_>| {
        let i = c.pick(me.labels.len() + 1);
        me.labels
            .get(i)
            .cloned()
            .unwrap_or_else(|| "no-such-label".into())
    };
    match (name, c.pick(6)) {
        (_, 5) => vec![c.text()],
        ("limit", p) => vec![["2", "1", "100", "0", "101"][p].into()],
        ("label", p) => (0..=p.min(3)).map(|_| label(c)).collect(),
        ("status", p) => vec![["booked", "free", "held", "blocked", "4"][p].into()],
        ("after", 0) => vec![label(c)],
        ("after", p) => vec![["e1", "c1", "event-0", "zzz"][p - 1].into()],
        ("chart", p) => vec![["c1", "c0", "none", "chart-0", "chart-1"][p].into()],
        ("key", 0) => vec![me.publishable.clone()],
        ("key", p) => vec![
            [
                world.t[1 - t].publishable.clone(),
                me.secret.clone(),
                me.revoked.clone(),
                String::new(),
            ][p - 1]
                .clone(),
        ],
        ("after_seq", p) => vec![["1", "0", "-1", "99999999999999999999", "x"][p].into()],
        (_, _) => vec![c.text()],
    }
}

/// Text for a field: mostly the workspace's own name for it; often the
/// bounds of the API's text rule (1 to 128 characters, 08 §6.1); sometimes
/// the input's bytes.
fn string(c: &mut Choices<'_>, me: &Tenant, field: &str) -> String {
    match c.pick(8) {
        0..=3 => me.name(field),
        4 => String::new(),
        5 => "x".repeat(129),
        6 => "x".repeat(128),
        _ => c.text(),
    }
}

/// A JSON value for `schema`, a field named `field` drawn from the world.
fn generate(
    c: &mut Choices<'_>,
    doc: &Value,
    world: &World,
    t: usize,
    field: &str,
    schema: &Value,
    depth: u8,
) -> Value {
    let me = &world.t[t];
    let schema = match schema.get("$ref").and_then(Value::as_str) {
        Some(r) => r
            .strip_prefix('#')
            .and_then(|p| doc.pointer(p))
            .unwrap_or(schema),
        None => schema,
    };
    // Sometimes the wrong kind of value altogether.
    if c.rare(23) {
        let wrong = [
            json!(null),
            json!(1),
            json!("x"),
            json!({}),
            json!(true),
            json!([]),
        ];
        return wrong[c.pick(wrong.len())].clone();
    }
    let label = |c: &mut Choices<'_>| {
        let i = c.pick(me.labels.len() + 1);
        json!(
            me.labels
                .get(i)
                .cloned()
                .unwrap_or_else(|| "no-such-label".into())
        )
    };
    match field {
        "event" => {
            return json!(
                [
                    "e1",
                    "e2",
                    "e0",
                    me.own_event.as_str(),
                    world.t[1 - t].own_event.as_str(),
                    "none"
                ][c.pick(6)]
            );
        }
        "chart" => {
            return json!(
                [
                    "c1",
                    "c0",
                    me.own_chart.as_str(),
                    world.t[1 - t].own_chart.as_str(),
                    "none"
                ][c.pick(5)]
            );
        }
        "hold_token" => {
            let mut v = me.tokens.clone();
            v.extend(world.t[1 - t].tokens.first().cloned());
            v.push("ht_bad".into());
            return json!(v[c.pick(v.len())]);
        }
        "objects" => {
            return match c.pick(5) {
                3 => json!({ "version": me.hash, "idx": [0, 1, c.pick(200)] }),
                4 => json!([]),
                n => json!((0..=n).map(|_| label(c)).collect::<Vec<_>>()),
            };
        }
        "key" if depth == 1 => return json!(["n1", "n2", "c1", "e1", "", "x/y"][c.pick(6)]),
        "name" | "order_ref" => return json!(string(c, me, field)),
        "email" => return json!(format!("owner{}@example.org", c.pick(3))),
        "document" => {
            let mut document: Value = serde_json::from_str(cavea_fuzz::CHART).unwrap_or_default();
            if c.rare(3) {
                document["nextOid"] = json!(c.number());
            }
            return document;
        }
        "ticket_types" | "extra" => {
            let mut map = Map::new();
            for _ in 0..c.pick(3) {
                let value = if field == "extra" {
                    json!({ "seat": c.pick(9) })
                } else {
                    json!("adult")
                };
                if let Value::String(l) = label(c) {
                    map.insert(l, value);
                }
            }
            return Value::Object(map);
        }
        _ => {}
    }
    for union in ["anyOf", "oneOf"] {
        if let Some(members) = schema.get(union).and_then(Value::as_array) {
            let member = &members[c.pick(members.len())];
            return generate(c, doc, world, t, field, member, depth);
        }
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return values[c.pick(values.len())].clone();
    }
    let kind = match schema.get("type") {
        Some(Value::String(k)) => k.clone(),
        Some(Value::Array(ks)) => ks[c.pick(ks.len())].as_str().unwrap_or("null").to_owned(),
        _ => "object".to_owned(),
    };
    match kind.as_str() {
        "string" => json!(string(c, me, "s")),
        "integer" | "number" => json!(c.number()),
        "boolean" => json!(c.pick(2) == 1),
        "null" => Value::Null,
        "array" => {
            let items = schema.get("items").cloned().unwrap_or_default();
            json!(
                (0..c.pick(3) + 1)
                    .map(|_| generate(c, doc, world, t, field, &items, depth + 1))
                    .collect::<Vec<_>>()
            )
        }
        _ => {
            let mut map = Map::new();
            let required: Vec<&str> = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|r| r.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if let Some(props) = schema.get("properties").and_then(Value::as_object)
                && depth < 4
            {
                for (name, prop) in props {
                    // A required field is left out now and then; an optional
                    // one is in half the time.
                    let keep = if required.contains(&name.as_str()) {
                        !c.rare(17)
                    } else {
                        c.pick(2) == 1
                    };
                    if keep {
                        map.insert(
                            name.clone(),
                            generate(c, doc, world, t, name, prop, depth + 1),
                        );
                    }
                }
            }
            if c.rare(19) {
                map.insert("unknown_field".into(), json!(1));
            }
            Value::Object(map)
        }
    }
}

/// The request record `bytes` describes.
fn build(h: &Harness, doc: &Value, world: &World, bytes: &[u8]) -> Option<Built> {
    let (&pick, rest) = bytes.split_first()?;
    let index = usize::from(pick) % h.ops.len();
    let op = &h.ops[index];
    let mut c = Choices { bytes: rest };

    // The credential.
    let cred = match c.pick(12) {
        0..=4 => Cred::Right(0),
        5 => Cred::Right(1),
        6 => Cred::None,
        7 => Cred::Malformed(c.byte() % 3),
        8 => Cred::Unknown,
        9 => Cred::Revoked(0),
        10 => Cred::Admin,
        _ => [Cred::Secret(0), Cred::Publishable(0), Cred::Publishable(1)][c.pick(3)],
    };
    let schemes: Vec<&str> = op.security.iter().flatten().map(String::as_str).collect();
    let cred = match cred {
        Cred::Right(_) if schemes.contains(&"adminKey") => Cred::Admin,
        Cred::Right(t) if schemes.contains(&"secretKey") => Cred::Secret(t),
        Cred::Right(t) if schemes.iter().any(|s| s.starts_with("publishableKey")) => {
            Cred::Publishable(t)
        }
        Cred::Right(_) => Cred::None,
        other => other,
    };
    let t = match cred {
        Cred::Secret(t) | Cred::Publishable(t) | Cred::Revoked(t) => t,
        _ => 0,
    };
    let key = match cred {
        Cred::Secret(t) => Some(world.t[t].secret.clone()),
        Cred::Publishable(t) => Some(world.t[t].publishable.clone()),
        Cred::Revoked(t) => Some(world.t[t].revoked.clone()),
        Cred::Admin => Some(ADMIN.to_owned()),
        Cred::Unknown => Some(format!("sk_{}", "7".repeat(64))),
        _ => None,
    };
    let in_query = schemes.contains(&"publishableKeyQuery");

    // The path.
    let mut path = String::new();
    let mut event = None;
    for segment in op.template.split('/').skip(1) {
        path.push('/');
        match segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            Some(name) => {
                let value = path_value(&mut c, world, t, name);
                path.push_str(&encode(&value));
                if name == "event" {
                    event = Some(value);
                }
            }
            None => path.push_str(segment),
        }
    }
    let mut method = op.method.clone();
    let mut changed = false;
    if c.rare(29) {
        changed = true;
        match c.pick(3) {
            0 => {
                method = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"][c.pick(7)]
                    .to_owned()
            }
            1 => path.push_str(&format!("/{}", encode(&c.text()))),
            _ => path = format!("/v1/{}", encode(&c.text())),
        }
    }

    // The query.
    let mut query: Vec<(String, String)> = Vec::new();
    for param in op.params.iter().filter(|p| p.place == Place::Query) {
        if param.name == "key" && in_query {
            if let Some(key) = &key {
                query.push(("key".into(), key.clone()));
            }
            continue;
        }
        if param.required || c.pick(3) != 0 {
            for value in query_values(&mut c, world, t, &param.name) {
                query.push((param.name.clone(), value));
            }
        }
    }
    if c.rare(31) {
        query.push((
            ["limit", "zz", "label", "after"][c.pick(4)].into(),
            c.text(),
        ));
    }
    let mut uri = path;
    if !query.is_empty() {
        uri.push('?');
        uri.push_str(
            &query
                .iter()
                .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
                .collect::<Vec<_>>()
                .join("&"),
        );
    }

    let mut req = Built::new(&method, uri, cred);
    req.op = (!changed).then_some(index);
    req.event = event;

    // Headers.
    match (cred, key) {
        (Cred::Malformed(m), _) => {
            let value = ["Basic abc", "Bearer", "Bearer a b"][usize::from(m)];
            req.headers.push(("authorization".into(), value.into()));
        }
        (_, Some(key)) if !in_query => req
            .headers
            .push(("authorization".into(), format!("Bearer {key}"))),
        _ => {}
    }
    // Header parameters, and the two credentials that are headers but not
    // parameters in the document.
    let mut names: Vec<&str> = op
        .params
        .iter()
        .filter(|p| p.place == Place::Header)
        .map(|p| p.name.as_str())
        .collect();
    if schemes.contains(&"sessionToken") {
        names.push("Cavea-Session");
    }
    if schemes.contains(&"holdToken") {
        names.push("Cavea-Hold-Token");
    }
    for name in names {
        let value = match name {
            "Idempotency-Key" => match c.pick(6) {
                0 => None,
                1..=3 => Some(format!("k{}", c.pick(3))),
                4 => Some(String::from_utf8_lossy(&c.raw()).into_owned()),
                _ => Some("x".repeat(256)),
            },
            "If-Match" => match c.pick(6) {
                0 => Some(world.t[t].etag.clone()),
                1 => Some("*".into()),
                2 => None,
                3 => Some("\"1\"".into()),
                4 => Some("W/\"2\"".into()),
                _ => Some(c.text()),
            },
            "Cavea-Session" => {
                let sessions = &world.sessions[t];
                let (value, session) = match c.pick(sessions.len() + 3) {
                    i if i < sessions.len() => match sessions.get(i) {
                        Some((token, session)) => (Some(token.clone()), session.clone()),
                        None => (None, Session::None),
                    },
                    i if i == sessions.len() => (None, Session::None),
                    // Signed by the other workspace's key.
                    i if i == sessions.len() + 1 => (
                        world.sessions[1 - t].first().map(|s| s.0.clone()),
                        Session::Invalid,
                    ),
                    _ => {
                        let text = c.text();
                        let session = if text.is_empty() {
                            Session::Unknown
                        } else {
                            Session::Invalid
                        };
                        (Some(text), session)
                    }
                };
                req.session = session;
                value
            }
            "Cavea-Hold-Token" => {
                let tokens = &world.t[t].tokens;
                match c.pick(tokens.len() + 2) {
                    i if i < tokens.len() => tokens.get(i).cloned(),
                    i if i == tokens.len() => world.t[1 - t].tokens.first().cloned(),
                    _ => None,
                }
            }
            "If-None-Match" => (c.pick(3) == 1).then(|| format!("\"{}\"", world.t[t].hash)),
            _ => c.rare(7).then(|| c.text()),
        };
        if let Some(value) = value {
            req.headers.push((name.to_owned(), value));
        }
    }
    if c.rare(13) {
        req.headers.push(("Cavea-Request-Id".into(), c.text()));
    }
    if c.rare(13) {
        req.headers.push((
            "Origin".into(),
            ["null", "https://evil.example", "http://localhost"][c.pick(3)].into(),
        ));
    }

    // The body.
    if op.bytes_body {
        req.body = Some(match c.pick(4) {
            0 | 1 => (PNG.to_vec(), "image/png"),
            2 => (PNG[..PNG.len() - 4].to_vec(), "image/png"),
            _ => (c.raw(), "application/octet-stream"),
        });
    } else if let Some(schema) = &op.json_body {
        req.body = Some(match c.pick(16) {
            15 => (c.raw(), "application/json"),
            14 => (b"{}".to_vec(), "text/plain"),
            13 => (
                b"[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]"
                    .to_vec(),
                "application/json",
            ),
            _ => {
                let mut body = generate(&mut c, doc, world, t, "", schema, 0);
                // Workspace keys are global: a workspace the admin key makes
                // is this world's own, so that an input does the same alone
                // as after others (two in one input still collide).
                if op.id == "createWorkspace"
                    && let Some(Value::String(key)) = body.get_mut("key")
                    && !key.is_empty()
                {
                    key.push_str(&format!("-w{}", world.tag));
                }
                if let Some(Value::String(e)) = body.get("event") {
                    req.event = Some(e.clone());
                }
                (body.to_string().into_bytes(), "application/json")
            }
        });
    } else if c.rare(37) {
        req.body = Some((c.raw(), "application/json"));
    }
    Some(req)
}

/// What the world learns from an answer: new hold tokens, the draft's etag.
fn learn(world: &mut World, cred: Cred, answer: &Answer) {
    let t = match cred {
        Cred::Secret(t) | Cred::Publishable(t) => t,
        _ => return,
    };
    if !(200..300).contains(&answer.status) {
        return;
    }
    if let Some(token) = answer.json.get("hold_token").and_then(Value::as_str)
        && token.starts_with("ht_")
        && !world.t[t].tokens.iter().any(|x| x == token)
    {
        world.t[t].tokens.push(token.to_owned());
    }
    // A draft's etag is a quoted number; a chart file's is its hash.
    let draft = |e: &str| {
        e.len() > 2
            && e.starts_with('"')
            && e.ends_with('"')
            && e[1..e.len() - 1].bytes().all(|b| b.is_ascii_digit())
    };
    // An event as its workspace's key sees it: whether it requires a session.
    if let (Cred::Secret(_), Some(key), Some(requires)) = (
        cred,
        answer.json.get("key").and_then(Value::as_str),
        answer
            .json
            .pointer("/settings/require_session")
            .and_then(Value::as_bool),
    ) {
        let list = &mut world.t[t].requires_session;
        list.retain(|e| e != key);
        if requires {
            list.push(key.to_owned());
        }
    }
    if let Some(etag) = answer.json.get("draft_etag").and_then(Value::as_str) {
        world.t[t].etag = etag.to_owned();
    } else if let Some(etag) = answer.etag.as_deref().filter(|e| draft(e)) {
        world.t[t].etag = etag.to_owned();
    }
}

fuzz_target!(|data: &[u8]| {
    let h = harness();
    let doc = &h.doc;
    h.runtime.block_on(async {
        let mut world = fresh(h).await;
        let mut rest = data;
        for _ in 0..MAX_REQUESTS {
            let Some((&n, tail)) = rest.split_first() else {
                break;
            };
            let take = usize::from(n).min(tail.len());
            let (record, tail) = tail.split_at(take);
            rest = tail;
            let Some(req) = build(h, doc, &world, record) else {
                continue;
            };
            let cred = req.cred;
            let answer = send(h, &world, req).await;
            learn(&mut world, cred, &answer);
        }
    });
});
