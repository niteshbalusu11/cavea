#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test helpers outside #[test] functions; a panic is a test failure"
)]
#![allow(
    clippy::await_holding_lock,
    reason = "the serial lock is held across a whole test; each test has its own runtime"
)]

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderValue, Request, Response, StatusCode, header};
use axum::routing::{get, post};
use cavea_engine::{
    Acquired, ActorConfig, Authority, Batch, ChartVersionId, ChartView, Command, EventId,
    EventState, Failure, InvalidReason, LoadedEvent, ManualClock, MemoryStore, Missing,
    ObjectFailure, ObjectReason, ObjectRef, Outcome, Request as EngineRequest, SeatStore,
    StoreError, Timestamp, TokenHash,
};
use cavea_telemetry::{LogFormat, REQUEST_ID_HEADER, span};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use tracing::subscriber::DefaultGuard;

use super::{ApiError, AppState, Code, Json, finish, router};
use crate::config::Config;
use crate::limiter::Limiter;
use crate::realtime::{Realtime, Timing};
use crate::seats::{Seats, ServerClock};

/// Tests in this module, and others that log through the engine, run one at a
/// time. `tracing` caches per callsite,
/// process-wide, whether any subscriber wants it; a test thread with no
/// subscriber can race another test's capture into caching "never".
pub(crate) fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn state() -> AppState {
    let config = Config {
        listen: ([127, 0, 0, 1], 0).into(),
        log: "info".to_owned(),
        log_format: LogFormat::Json,
        database_url: "postgres://localhost/unused".parse().unwrap(),
        db_pool: std::num::NonZeroU32::MIN,
        node_id: "test".to_owned(),
        lease_secs: std::num::NonZeroU32::MIN.saturating_add(14),
        heartbeat_secs: std::num::NonZeroU32::MIN.saturating_add(4),
        admin_key: None,
        cors_origins: crate::config::CorsOrigins::Any,
        snapshot_max_age: 1,
        rt_max_connections: std::num::NonZeroU32::MIN.saturating_add(99),
        rt_max_per_ip: std::num::NonZeroU32::MIN.saturating_add(9),
        trusted_proxies: crate::net::TrustedProxies::default(),
        rate_limit_holds: 120,
        rate_limit_reads: 3_000,
        asset_max_bytes: std::num::NonZeroU32::MIN.saturating_add((10 << 20) - 1),
    };
    // Never connects: no route under test reaches the database.
    let db = sqlx::postgres::PgPoolOptions::new().connect_lazy_with(Default::default());
    AppState {
        seats: Seats::new(db.clone(), &config, ServerClock::System, None),
        realtime: Realtime::new(100, 10, Timing::default()),
        limiter: Arc::new(Limiter::new(
            config.rate_limit_holds,
            config.rate_limit_reads,
        )),
        config: Arc::new(config),
        db,
        health: super::Health::default(),
    }
}

fn app(routes: Router<AppState>) -> Router {
    finish(routes, state())
}

/// Send `request` and read the response, checking both against the OpenAPI
/// document (`openapi::check`): the response against its route and status,
/// and a JSON body the server accepted against the route's request schema.
async fn send(app: Router, request: Request<Body>) -> (Response<Body>, Value) {
    send_raw(app, request).await.0
}

/// [`send`], also returning the response body's bytes.
async fn send_raw(app: Router, request: Request<Body>) -> ((Response<Body>, Value), Bytes) {
    let method = request.method().to_string();
    let path = request.uri().path().to_owned();
    let json_body = request
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|t| t.as_bytes().starts_with(b"application/json"));
    let (request, sent) = if json_body {
        let (parts, body) = request.into_parts();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        (
            Request::from_parts(parts, Body::from(bytes.clone())),
            Some(bytes),
        )
    } else {
        (request, None)
    };
    let response = app.oneshot(request).await.unwrap();
    let (parts, body) = response.into_parts();
    let content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .map(|t| t.to_str().unwrap().to_owned());
    // A stream does not end: stop at its head.
    let bytes = if content_type.as_deref() == Some("text/event-stream") {
        Bytes::new()
    } else {
        axum::body::to_bytes(body, 1 << 26).await.unwrap()
    };
    let checked = super::openapi::check::exchange(
        &method,
        &path,
        sent.as_deref(),
        parts.status.as_u16(),
        content_type.as_deref(),
        &bytes,
    );
    if let Err(e) = checked {
        panic!("the OpenAPI document disagrees: {e}");
    }
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    ((Response::from_parts(parts, Body::empty()), json), bytes)
}

fn get_req(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

fn request_id(response: &Response<Body>) -> &str {
    response
        .headers()
        .get(REQUEST_ID_HEADER)
        .expect("Cavea-Request-Id on every response")
        .to_str()
        .unwrap()
}

/// Checks the problem+json shape and returns the body.
fn assert_problem(response: &Response<Body>, body: &Value, code: Code) {
    assert_eq!(response.status(), code.status(), "{body}");
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    let object = body.as_object().unwrap();
    let mut keys: Vec<_> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["code", "details", "message", "request_id", "status"]);
    assert_eq!(body["code"], code.as_str());
    assert_eq!(body["status"], code.status().as_u16());
    assert!(body["message"].as_str().is_some_and(|m| !m.is_empty()));
    assert!(body["details"].is_object());
    assert_eq!(body["request_id"], request_id(response));
}

/// Log output captured through the production JSON formatter.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Captured {
    fn install(&self) -> DefaultGuard {
        let writer = self.clone();
        let subscriber =
            cavea_telemetry::subscriber("debug", LogFormat::Json, move || writer.clone()).unwrap();
        tracing::subscriber::set_default(subscriber)
    }

    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }

    fn records(&self) -> Vec<Value> {
        self.text()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// The request span of the completion record for `id`.
    fn completed(&self, id: &str) -> Value {
        self.records()
            .into_iter()
            .filter(|r| r["message"] == "request completed")
            .map(|r| r["spans"][0].clone())
            .find(|s| s["request_id"] == id)
            .unwrap_or_else(|| panic!("no completion record for {id} in {}", self.text()))
    }
}

#[tokio::test]
async fn healthz_responds_ok() {
    let _serial = serial();
    let response = router(state()).oneshot(get_req("/healthz")).await.unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers().contains_key(REQUEST_ID_HEADER));
    let body = axum::body::to_bytes(response.into_body(), 64)
        .await
        .unwrap();
    assert_eq!(&body[..], b"ok");
}

/// `GET /metrics` answers the admin key alone, with Prometheus text: each
/// family declared before its samples; each sample a name, labels from the
/// bounded set and a number; each histogram's buckets cumulative up to `+Inf`,
/// which is its count. A path no route matched is `<unmatched>`, never itself.
#[tokio::test]
async fn metrics_are_prometheus_text_with_bounded_labels() {
    let _serial = serial();
    const ADMIN: &str = "test-admin-key-not-a-real-secret-000";
    let mut state = state();
    Arc::make_mut(&mut state.config).admin_key = Some(ADMIN.parse().unwrap());
    let app = router(state);
    for path in ["/healthz", "/readyz", "/v1/no-such-route-ev_9f3a"] {
        app.clone().oneshot(get_req(path)).await.unwrap();
    }
    let refused = app.clone().oneshot(get_req("/metrics")).await.unwrap();
    assert_eq!(refused.status(), 401);
    let request = Request::get("/metrics")
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN}"))
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/plain; version=0.0.4; charset=utf-8"
    );
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .unwrap();
    let text = std::str::from_utf8(&body).unwrap();

    const LABELS: [&str; 7] = [
        "op",
        "result",
        "route",
        "status",
        "transport",
        "budget",
        "le",
    ];
    let mut kinds = std::collections::HashMap::new();
    // Per histogram series: the last cumulative bucket count, and `+Inf`'s.
    let mut buckets: std::collections::HashMap<String, (f64, Option<f64>)> = Default::default();
    let mut counts = Vec::new();
    let mut routes = std::collections::HashSet::new();
    for line in text.lines() {
        if line.starts_with("# HELP ") {
            continue;
        }
        if let Some(declared) = line.strip_prefix("# TYPE ") {
            let (name, kind) = declared.split_once(' ').unwrap();
            assert!(["counter", "gauge", "histogram"].contains(&kind), "{line}");
            assert!(
                kinds.insert(name.to_owned(), kind).is_none(),
                "declared twice: {line}"
            );
            continue;
        }
        let (series, value) = line.rsplit_once(' ').unwrap();
        let value: f64 = value.parse().unwrap_or_else(|_| panic!("{line}"));
        assert!(value.is_finite() && value >= 0.0, "{line}");
        let (name, labels) = match series.split_once('{') {
            Some((name, labels)) => (name, labels.strip_suffix('}').unwrap()),
            None => (series, ""),
        };
        assert!(
            name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && name.starts_with("cavea_"),
            "{line}"
        );
        let mut le = None;
        let mut rest = labels;
        let mut others = String::new();
        while !rest.is_empty() {
            let (key, after) = rest.split_once("=\"").unwrap();
            assert!(LABELS.contains(&key), "unbounded label {key}: {line}");
            let end = after.find('"').unwrap();
            let value = &after[..end];
            match key {
                "le" => le = Some(value),
                "route" => {
                    routes.insert(value.to_owned());
                }
                _ => {}
            }
            if key != "le" {
                others.push_str(&rest[..key.len() + 2 + end + 1]);
            }
            rest = after[end + 1..].trim_start_matches(',');
        }
        let family = [
            name.strip_suffix("_bucket"),
            name.strip_suffix("_sum"),
            name.strip_suffix("_count"),
        ]
        .into_iter()
        .flatten()
        .find(|f| kinds.get(*f) == Some(&"histogram"))
        .unwrap_or(name);
        assert!(
            kinds.contains_key(family),
            "sample before its # TYPE: {line}"
        );
        let key = format!("{family}{{{others}}}");
        if name.ends_with("_bucket") && family != name {
            let entry = buckets.entry(key).or_insert((0.0, None));
            assert!(value >= entry.0, "buckets not cumulative: {line}");
            entry.0 = value;
            if le.unwrap() == "+Inf" {
                entry.1 = Some(value);
            }
        } else if name.ends_with("_count") && family != name {
            counts.push((key, value));
        }
    }
    assert!(!counts.is_empty());
    for (key, count) in counts {
        assert_eq!(buckets[&key].1, Some(count), "{key}: +Inf is not the count");
    }
    for name in [
        "cavea_commands_total",
        "cavea_http_requests_total",
        "cavea_rt_subscribers",
        "cavea_db_pool_in_use",
        "cavea_log_records_dropped_total",
        "cavea_rt_fanout_seconds",
    ] {
        assert!(kinds.contains_key(name), "{name} missing");
    }
    assert!(
        routes.contains("<unmatched>") && routes.contains("/healthz"),
        "{routes:?}"
    );
    assert!(routes.iter().all(|r| !r.contains("ev_9f3a")), "{routes:?}");
}

#[tokio::test]
async fn request_span_logs_id_and_route_template_not_path() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    // Not a documented path, so that `send` does not hold it to the document.
    let app = app(Router::new().route("/test/events/{event_key}", get(|| async { "ok" })));

    let (response, _) = send(app.clone(), get_req("/test/events/customer-key-1")).await;
    assert_eq!(response.status(), 200);
    let id = request_id(&response);
    assert!(id.starts_with("req_") && id.len() == 36, "{id}");
    let span = logs.completed(id);
    assert_eq!(span["name"], span::REQUEST);
    assert_eq!(span["method"], "GET");
    assert_eq!(span["route"], "/test/events/{event_key}");
    assert_eq!(span["status"], 200);
    assert!(span["duration_ms"].is_f64());

    let (response, _) = send(app, get_req("/v2/customer-key-2")).await;
    let span = logs.completed(request_id(&response));
    assert_eq!(span["route"], cavea_telemetry::UNMATCHED_ROUTE);
    assert_eq!(span["status"], 404);

    let text = logs.text();
    assert!(!text.contains("customer-key"), "raw path logged: {text}");
}

#[tokio::test]
async fn inbound_request_id_is_adopted_only_when_well_formed() {
    let _serial = serial();
    let app = app(Router::new().route("/r", get(|| async { "ok" })));
    let long = "a".repeat(65);
    let cases: [(&[u8], bool); 7] = [
        (b"upstream-1.2_X", true),
        (&[b'b'; 64], true),
        (long.as_bytes(), false),
        (b"", false),
        (b"a b", false),
        (b"x\",\"level\":\"ERROR", false),
        (b"caf\xc3\xa9", false),
    ];
    for (inbound, adopted) in cases {
        let request = Request::get("/r")
            .header(REQUEST_ID_HEADER, HeaderValue::from_bytes(inbound).unwrap())
            .body(Body::empty())
            .unwrap();
        let (response, _) = send(app.clone(), request).await;
        let id = request_id(&response);
        if adopted {
            assert_eq!(id.as_bytes(), inbound);
        } else {
            assert!(
                id.starts_with("req_") && id.len() == 36,
                "{inbound:?} -> {id}"
            );
        }
    }
}

#[tokio::test]
async fn every_code_renders_as_problem_json() {
    let _serial = serial();
    for code in Code::ALL {
        let app = app(Router::new().route("/e", get(move || async move { ApiError::new(code) })));
        let (response, body) = send(app, get_req("/e")).await;
        assert_problem(&response, &body, code);
        assert_eq!(body["details"], json!({}));
        assert_eq!(
            response.headers().get(header::RETRY_AFTER).is_some(),
            code == Code::Overloaded,
            "{code:?}"
        );
    }
}

#[tokio::test]
async fn internal_error_logs_cause_and_returns_generic_body() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    let app = app(Router::new().route(
        "/e",
        get(|| async { ApiError::internal("pool closed while committing") }),
    ));
    let (response, body) = send(app, get_req("/e")).await;
    assert_problem(&response, &body, Code::InternalError);
    assert!(!body.to_string().contains("pool closed"));
    let id = request_id(&response);
    let record = logs
        .records()
        .into_iter()
        .find(|r| r["message"] == "internal error")
        .unwrap();
    assert_eq!(record["error"], "pool closed while committing");
    assert_eq!(record["code"], "internal_error");
    assert_eq!(record["spans"][0]["request_id"], id);
}

#[tokio::test]
async fn engine_failures_convert_without_loss() {
    let _serial = serial();
    let failures = [
        (
            Failure::InvalidRequest(InvalidReason::TtlOutOfRange),
            json!({"reason": "ttl_out_of_range"}),
        ),
        (
            Failure::NotFound(Missing::HoldToken),
            json!({"missing": "hold_token"}),
        ),
        (
            Failure::NotFound(Missing::Objects(vec![
                ObjectRef::Label("A-1".into()),
                ObjectRef::Idx(7),
            ])),
            json!({"missing": "objects", "objects": [{"label": "A-1"}, {"idx": 7}]}),
        ),
        (
            Failure::ObjectNotFree(vec![ObjectFailure {
                idx: 3,
                reason: ObjectReason::NotFree,
            }]),
            json!({"objects": [{"idx": 3, "reason": "not_free"}]}),
        ),
        (
            Failure::ObjectNotHeldByToken(vec![
                ObjectFailure {
                    idx: 4,
                    reason: ObjectReason::NotHeldByToken,
                },
                ObjectFailure {
                    idx: 5,
                    reason: ObjectReason::NotFree,
                },
            ]),
            json!({"objects": [
                {"idx": 4, "reason": "not_held_by_token"},
                {"idx": 5, "reason": "not_free"},
            ]}),
        ),
        (Failure::HoldTokenExpired, json!({})),
        (
            Failure::HoldLimitExceeded {
                limit: 4,
                requested: 6,
            },
            json!({"limit": 4, "requested": 6}),
        ),
        (Failure::Overloaded, json!({})),
        (Failure::Unavailable, json!({})),
    ];
    for (failure, details) in failures {
        let error = ApiError::from(failure.clone());
        let code = error.code();
        assert_eq!(code.as_str(), failure.code());
        assert_eq!(code.status().as_u16(), failure.http_status());
        let app = app(Router::new().route(
            "/e",
            get(move || {
                let failure = failure.clone();
                async move { ApiError::from(failure) }
            }),
        ));
        let (response, body) = send(app, get_req("/e")).await;
        assert_problem(&response, &body, code);
        assert_eq!(body["details"], details);
    }
}

#[tokio::test]
async fn axum_rejections_are_problem_json() {
    let _serial = serial();
    let app = app(Router::new()
        .route(
            "/json",
            post(|Json(v): Json<Value>| async move { axum::Json(v) }),
        )
        .layer(DefaultBodyLimit::max(64)));
    let post_json = |body: &str| {
        Request::post("/json")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))
            .unwrap()
    };

    let (response, body) = send(app.clone(), get_req("/missing")).await;
    assert_problem(&response, &body, Code::NotFound);
    let (response, body) = send(app.clone(), get_req("/json")).await;
    assert_problem(&response, &body, Code::NotFound);

    let cases = [
        (post_json("{\"a\":"), "json_syntax"),
        (
            post_json(&format!("\"{}\"", "x".repeat(100))),
            "body_too_large",
        ),
        (
            Request::post("/json").body(Body::from("{}")).unwrap(),
            "content_type",
        ),
    ];
    for (request, reason) in cases {
        let (response, body) = send(app.clone(), request).await;
        assert_problem(&response, &body, Code::InvalidRequest);
        assert_eq!(body["details"]["reason"], reason);
    }
    let (response, body) = send(app, post_json("{\"a\":1}")).await;
    assert_eq!(response.status(), 200);
    assert_eq!(body, json!({"a": 1}));
}

/// The in-memory store, logging once inside each commit as a database store
/// would.
struct LoggingStore(MemoryStore);

impl SeatStore for LoggingStore {
    async fn acquire(&self, event: EventId) -> Result<Acquired, StoreError> {
        self.0.acquire(event).await
    }

    async fn load_event(&self, event: EventId) -> Result<LoadedEvent, StoreError> {
        self.0.load_event(event).await
    }

    async fn commit_batch(&self, batch: Batch) -> Result<(), StoreError> {
        tracing::info!("committing");
        self.0.commit_batch(batch).await
    }

    async fn heartbeat(&self, event: EventId, epoch: u64) -> Result<(), StoreError> {
        self.0.heartbeat(event, epoch).await
    }

    async fn release(&self, event: EventId, epoch: u64) -> Result<(), StoreError> {
        self.0.release(event, epoch).await
    }
}

/// An actor for a one-seat event, and a route that creates hold token 1 on it.
fn hold_route(
    event: EventId,
) -> (
    axum::routing::MethodRouter<AppState>,
    tokio::task::JoinHandle<Result<(), cavea_engine::ActorError>>,
) {
    let chart = Arc::new(ChartView::new([(100, "S-0")]).unwrap());
    let loaded = LoadedEvent {
        epoch: 1,
        seq: 0,
        holds: vec![],
        objects: vec![],
        idempotency: vec![],
    };
    let store = MemoryStore::new();
    store.create_event(event, 1, ChartVersionId(1));
    let (handle, run) = cavea_engine::actor(
        EventState::restore(event, chart, loaded).unwrap(),
        Arc::new(LoggingStore(store)),
        ManualClock::new(Timestamp(1_000_000)),
        ActorConfig::default(),
    );
    let route = post(move || {
        let handle = handle.clone();
        async move {
            let request = EngineRequest {
                actor: "sk_test".into(),
                authority: Authority::Secret,
                command: Command::CreateHold {
                    token: TokenHash([1; 32]),
                    ttl_secs: 900,
                    max_objects: None,
                },
                idempotency: None,
            };
            match handle.send(request).await {
                Ok(Outcome::HoldCreated { .. }) => Ok(StatusCode::CREATED),
                Ok(_) => Err(ApiError::internal("unexpected outcome")),
                Err(failure) => Err(ApiError::from(failure)),
            }
        }
    });
    (route, tokio::spawn(run))
}

/// Commands submitted in request handlers are applied and committed on the
/// actors' tasks. In the production JSON logs a record inside `apply` carries
/// the submitting request's id, and a record inside `commit` shares its
/// `event_id` and `seq` with exactly the requests whose commands it made
/// durable, even when two events commit the same `seq`.
#[tokio::test]
async fn actor_work_is_tied_to_its_request_in_logs() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();

    let (route_a, actor_a) = hold_route(EventId(0xa));
    let (route_b, actor_b) = hold_route(EventId(0xb));
    let app = app(Router::new().route("/a", route_a).route("/b", route_b));
    let hold = |uri| Request::post(uri).body(Body::empty()).unwrap();

    let mut created = Vec::new();
    for uri in ["/a", "/b"] {
        let (response, _) = send(app.clone(), hold(uri)).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        created.push(request_id(&response).to_owned());
    }
    // The same token again: refused in `apply`, so nothing to commit.
    let (refused, _) = send(app.clone(), hold("/a")).await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    drop(app);
    actor_a.await.unwrap().unwrap();
    actor_b.await.unwrap().unwrap();

    let commits: Vec<Value> = logs
        .records()
        .into_iter()
        .filter(|r| r["message"] == "committing")
        .map(|r| r["spans"][0].clone())
        .collect();
    assert_eq!(commits.len(), 2);
    for (id, commit) in created.iter().zip(&commits) {
        assert_eq!(commit["name"], span::COMMIT);
        assert_eq!(commit["seq"], 1);
        let request = logs.completed(id);
        assert_eq!(request["commit_seq"], commit["seq"]);
        assert_eq!(request["event_id"], commit["event_id"]);
    }
    assert_ne!(commits[0]["event_id"], commits[1]["event_id"]);

    let refused_id = request_id(&refused);
    assert!(logs.completed(refused_id).get("commit_seq").is_none());
    let record = logs
        .records()
        .into_iter()
        .find(|r| r["message"] == "command refused")
        .unwrap();
    assert_eq!(record["spans"][0]["name"], span::REQUEST);
    assert_eq!(record["spans"][0]["request_id"], refused_id);
    assert_eq!(record["spans"][1]["name"], span::APPLY);
}

/// Bodies over the route's limit, and JSON nested deeper than the depth
/// limit, are refused before anything is parsed. A megabyte of `[` under a
/// route whose limit admits it is refused by the depth scan, which allocates
/// nothing.
#[tokio::test]
async fn body_size_and_json_depth_limits() {
    use super::extract::{DEFAULT_BODY_LIMIT, MAX_JSON_DEPTH};
    let _serial = serial();
    let echo = |Json(v): Json<Value>| async move { axum::Json(v) };
    let app = app(Router::new()
        .route("/default", post(echo))
        .route("/large", post(echo).layer(DefaultBodyLimit::max(2 << 20))));
    let post_json = |uri: &str, body: String| {
        Request::post(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap()
    };
    let string_of = |len: usize| format!("\"{}\"", "x".repeat(len - 2));
    let nested = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));

    let cases = [
        (
            "/default",
            string_of(DEFAULT_BODY_LIMIT + 1),
            Some("body_too_large"),
        ),
        ("/default", string_of(DEFAULT_BODY_LIMIT), None),
        ("/large", string_of(2 * DEFAULT_BODY_LIMIT), None),
        ("/default", nested(MAX_JSON_DEPTH), None),
        ("/default", nested(MAX_JSON_DEPTH + 1), Some("json_depth")),
        (
            "/default",
            format!("{{\"a\":{}}}", nested(MAX_JSON_DEPTH)),
            Some("json_depth"),
        ),
        // Brackets in strings, escaped quotes included, are not nesting.
        ("/default", format!("[\"\\\"{}\"]", "[".repeat(100)), None),
        ("/large", "[".repeat(1 << 20), Some("json_depth")),
    ];
    for (uri, body, refused) in cases {
        let len = body.len();
        let (response, json) = send(app.clone(), post_json(uri, body)).await;
        match refused {
            Some(reason) => {
                assert_problem(&response, &json, Code::InvalidRequest);
                assert_eq!(json["details"]["reason"], reason, "{uri} {len}");
            }
            None => assert_eq!(response.status(), 200, "{uri} {len}: {json}"),
        }
    }
}

/// CORS (08 §11) without a database: preflights for every public route,
/// requests refused before the database (no credential) from an allowed and a
/// disallowed origin under `*` and under a list, and none of it on a
/// secret-key route or the chart files' open policy.
#[tokio::test]
async fn cors_on_public_routes_only() {
    use crate::config::CorsOrigins;
    let _serial = serial();
    let allowed = "https://tickets.example";
    let public = [
        ("GET", "/v1/public/events/e"),
        ("POST", "/v1/public/hold-tokens"),
        ("GET", "/v1/public/hold-tokens/current"),
        ("POST", "/v1/public/events/e/hold"),
        ("POST", "/v1/public/events/e/release"),
    ];
    let send_from = async |app: &Router, method: &str, uri: &str, origin: &str, preflight| {
        let mut request = Request::builder().uri(uri).header(header::ORIGIN, origin);
        request = if preflight {
            request
                .method("OPTIONS")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, method)
                .header(
                    header::ACCESS_CONTROL_REQUEST_HEADERS,
                    "authorization,content-type",
                )
        } else {
            request.method(method)
        };
        send(app.clone(), request.body(Body::empty()).unwrap()).await
    };
    let cors_headers = |response: &Response<Body>| {
        response
            .headers()
            .keys()
            .filter(|k| k.as_str().starts_with("access-control-"))
            .count()
    };
    for policy in [
        CorsOrigins::Any,
        CorsOrigins::List(vec![allowed.to_owned(), "http://localhost:5173".to_owned()]),
    ] {
        let mut state = state();
        Arc::make_mut(&mut state.config).cors_origins = policy.clone();
        let app = finish(super::routes().into_router(), state);
        let listed = policy != CorsOrigins::Any;
        let expected_allow = if listed { allowed } else { "*" };
        for (method, uri) in public {
            for (origin, ok) in [
                (allowed, true),
                ("https://TICKETS.example", true),
                ("https://evil.example", !listed),
            ] {
                let label = format!("{policy:?} {method} {uri} {origin}");
                // The preflight: no credential, answered from the setting.
                let (response, _) = send_from(&app, method, uri, origin, true).await;
                assert_eq!(response.status(), 204, "{label}");
                let h = response.headers();
                assert_eq!(h.get(header::VARY).is_some(), listed, "{label}");
                assert!(h.get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS).is_none());
                if ok {
                    let allow = if listed { origin } else { "*" };
                    assert_eq!(h[header::ACCESS_CONTROL_ALLOW_ORIGIN], allow, "{label}");
                    assert!(
                        h[header::ACCESS_CONTROL_ALLOW_METHODS]
                            .to_str()
                            .unwrap()
                            .contains(method)
                    );
                    let headers = h[header::ACCESS_CONTROL_ALLOW_HEADERS].to_str().unwrap();
                    for name in ["Authorization", "Content-Type", "Cavea-Hold-Token"] {
                        assert!(headers.contains(name), "{label}: {headers}");
                    }
                    assert_eq!(h[header::ACCESS_CONTROL_MAX_AGE], "7200");
                } else {
                    assert_eq!(cors_headers(&response), 0, "{label}");
                }
                // The request itself, refused without a credential: the error
                // is readable by an allowed page.
                let (response, problem) = send_from(&app, method, uri, origin, false).await;
                assert_problem(&response, &problem, Code::Unauthorized);
                let h = response.headers();
                if ok {
                    let allow = if listed { origin } else { expected_allow };
                    assert_eq!(h[header::ACCESS_CONTROL_ALLOW_ORIGIN], allow, "{label}");
                    assert_eq!(
                        h[header::ACCESS_CONTROL_EXPOSE_HEADERS],
                        "Cavea-Request-Id, Retry-After"
                    );
                    assert!(h.get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS).is_none());
                } else {
                    assert_eq!(cors_headers(&response), 0, "{label}");
                }
            }
        }
        // Secret-key routes: no CORS at all; `OPTIONS` is an unknown method.
        for (method, uri) in [
            ("POST", "/v1/events/e/hold"),
            ("GET", "/v1/hold-tokens/x"),
            ("POST", "/v1/hold-tokens"),
        ] {
            let (response, problem) = send_from(&app, method, uri, allowed, true).await;
            assert_problem(&response, &problem, Code::NotFound);
            assert_eq!(cors_headers(&response), 0, "{uri}");
            let (response, problem) = send_from(&app, method, uri, allowed, false).await;
            assert_problem(&response, &problem, Code::Unauthorized);
            assert_eq!(cors_headers(&response), 0, "{uri}");
        }
        // The chart files stay open to every origin, with no `Vary: Origin`.
        let (response, _) = send_from(
            &app,
            "GET",
            "/v1/public/charts/x.ccf",
            "https://evil.example",
            false,
        )
        .await;
        assert_eq!(response.status(), 404);
        assert_eq!(response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert!(response.headers().get(header::VARY).is_none());
    }
}

/// The document is whole: unique operation ids, every `$ref` resolves, every
/// component is used, every error code is listed (08 §6.4), every operation
/// has a success response and, with a credential, its refusals; and the
/// schemas use only what `openapi::check` validates.
#[test]
fn openapi_document_is_complete() {
    use super::openapi::check::{doc, validate};
    let doc = doc();
    let text = doc.to_string();
    let mut refs = std::collections::BTreeSet::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find("\"$ref\":\"#") {
        rest = &rest[at + 9..];
        let end = rest.find('"').unwrap();
        let pointer = &rest[..end];
        assert!(doc.pointer(pointer).is_some(), "{pointer} does not resolve");
        refs.insert(pointer.to_owned());
    }
    for (kind, entries) in doc["components"].as_object().unwrap() {
        if kind == "securitySchemes" {
            continue;
        }
        for name in entries.as_object().unwrap().keys() {
            let pointer = format!("/components/{kind}/{name}");
            assert!(refs.contains(&pointer), "{pointer} is never used");
        }
    }
    let problem = &doc["components"]["schemas"]["Problem"]["properties"]["code"]["enum"];
    let codes: Vec<&str> = Code::ALL.iter().map(|c| c.as_str()).collect();
    assert_eq!(problem, &json!(codes));
    let mut ids = std::collections::HashSet::new();
    for (path, item) in doc["paths"].as_object().unwrap() {
        for (method, op) in item.as_object().unwrap() {
            let id = op["operationId"].as_str().unwrap();
            assert!(ids.insert(id), "operationId {id} twice");
            assert!(!op["summary"].as_str().unwrap().is_empty(), "{id}");
            let statuses: Vec<&String> = op["responses"].as_object().unwrap().keys().collect();
            assert!(
                statuses
                    .iter()
                    .any(|s| s.starts_with('1') || s.starts_with('2')),
                "{id}"
            );
            if op["security"] != json!([]) {
                assert!(statuses.contains(&&"401".to_owned()), "{method} {path}");
                assert!(statuses.contains(&&"403".to_owned()), "{method} {path}");
            }
        }
    }
    // Every schema is one `check` understands: validating anything against
    // it reports a refusal or a pass, never an unchecked keyword.
    for (name, _) in doc["components"]["schemas"].as_object().unwrap() {
        let schema = json!({ "$ref": format!("#/components/schemas/{name}") });
        for value in [json!(null), json!({}), json!([]), json!("x"), json!(1)] {
            if let Err(e) = validate(&schema, &value, name) {
                assert!(!e.contains("is not checked"), "{e}");
            }
        }
    }
}

/// The committed `openapi.json` is the document the code builds. When this
/// fails, run `cargo run -p cavea-server -- openapi > crates/cavea-server/openapi.json`.
#[test]
fn openapi_file_is_current() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/openapi.json");
    let committed = std::fs::read_to_string(path).unwrap_or_default();
    assert!(
        committed == super::openapi::text(),
        "{path} is stale: run `cargo run -p cavea-server -- openapi > crates/cavea-server/openapi.json`"
    );
}

/// `GET /v1/openapi.json` serves the document without a credential, cacheable,
/// with an etag that a revalidation matches.
#[tokio::test]
async fn openapi_is_served() {
    let _serial = serial();
    let app = router(state());
    let ((response, json), bytes) = send_raw(app.clone(), get_req("/v1/openapi.json")).await;
    assert_eq!(response.status(), 200);
    assert_eq!(&bytes[..], super::openapi::text().as_bytes());
    assert_eq!(json["openapi"], "3.1.0");
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "public, max-age=300"
    );
    let etag = response.headers()[header::ETAG].clone();
    let request = Request::get("/v1/openapi.json")
        .header(header::IF_NONE_MATCH, etag)
        .body(Body::empty())
        .unwrap();
    let (response, _) = send(app, request).await;
    assert_eq!(response.status(), 304);
}

/// A panic in a handler answers `500 internal_error` with the request id, is
/// logged once as an error in the request's span without the request's data,
/// is counted under its route, and leaves the connection and the server
/// serving: a second request on the same keep-alive connection is answered.
#[tokio::test]
async fn a_panicking_handler_answers_500() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let _serial = serial();
    install_panic_log();
    let logs = Captured::default();
    let _guard = logs.install();
    // With one capturing subscriber alive, `tracing` decides a callsite's
    // interest from the thread that reaches it first, or rebuilds it: a
    // failing test's thread, without a subscriber, would cache "never" for
    // the hook's record. Reach it here (a caught panic), then decide again
    // from this thread; no other thread rebuilds while `serial` is held.
    let _ = std::panic::catch_unwind(|| std::panic::panic_any("priming the panic log"));
    tracing::callsite::rebuild_interest_cache();
    const MARKER: &str = "request-data-1f0c";
    let mut state = state();
    const ADMIN: &str = "test-admin-key-not-a-real-secret-000";
    Arc::make_mut(&mut state.config).admin_key = Some(ADMIN.parse().unwrap());
    let routes = super::routes().into_router().route(
        "/test/panic",
        post(|body: String| async move {
            assert!(body.is_empty(), "handler failed on {body}");
        }),
    );
    let app = finish(routes, state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut answers = Vec::new();
    for request in [
        format!(
            "POST /test/panic HTTP/1.1\r\nhost: x\r\ncontent-length: {}\r\n\r\n{MARKER}",
            MARKER.len()
        ),
        "GET /healthz HTTP/1.1\r\nhost: x\r\n\r\n".to_owned(),
    ] {
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut buffer = Vec::new();
        // Read one response: its head, then `content-length` bytes.
        loop {
            let mut chunk = [0; 4096];
            let n = socket.read(&mut chunk).await.unwrap();
            assert!(n > 0, "the connection closed");
            buffer.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buffer).to_string();
            if let Some((head, body)) = text.split_once("\r\n\r\n") {
                let length: usize = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(str::to_owned)
                    })
                    .unwrap()
                    .parse()
                    .unwrap();
                if body.len() >= length {
                    answers.push((head.to_owned(), body.to_owned()));
                    break;
                }
            }
        }
    }
    let (head, body) = &answers[0];
    assert!(head.starts_with("HTTP/1.1 500"), "{head}");
    assert!(
        head.contains("content-type: application/problem+json"),
        "{head}"
    );
    let problem: Value = serde_json::from_str(body).unwrap();
    assert_eq!(problem["code"], "internal_error");
    let id = problem["request_id"].as_str().unwrap();
    assert!(head.contains(&format!("cavea-request-id: {id}")), "{head}");
    assert!(answers[1].0.starts_with("HTTP/1.1 200"), "{}", answers[1].0);
    assert_eq!(answers[1].1, "ok");

    let errors: Vec<Value> = logs
        .records()
        .into_iter()
        .filter(|r| r["level"] == "ERROR" && r["payload"] != "priming the panic log")
        .collect();
    assert_eq!(errors.len(), 1, "{}", logs.text());
    assert_eq!(errors[0]["message"], "panic");
    assert_eq!(errors[0]["spans"][0]["request_id"], id);
    assert_eq!(errors[0]["spans"][0]["route"], "/test/panic");
    assert!(errors[0]["location"].as_str().unwrap().contains("tests.rs"));
    assert!(!logs.text().contains(MARKER), "request data logged");
    assert_eq!(logs.completed(id)["status"], 500);
    let metrics = crate::metrics::render(&state);
    assert!(
        metrics.contains(r#"cavea_http_requests_total{route="/test/panic",status="500"} 1"#),
        "{metrics}"
    );
    server.abort();
}

/// The production panic hook (`cavea_telemetry::log_panic`), chained before
/// the default one so that a failing test still prints its message. Once per
/// test binary.
pub(crate) fn install_panic_log() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            cavea_telemetry::log_panic(info);
            default(info);
        }));
    });
}

mod api;
