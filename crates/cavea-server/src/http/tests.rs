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
use axum::body::Body;
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
use crate::seats::{Seats, ServerClock};

/// Tests in this module run one at a time. `tracing` caches per callsite,
/// process-wide, whether any subscriber wants it; a test thread with no
/// subscriber can race another test's capture into caching "never".
fn serial() -> MutexGuard<'static, ()> {
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
    };
    // Never connects: no route under test reaches the database.
    let db = sqlx::postgres::PgPoolOptions::new().connect_lazy_with(Default::default());
    AppState {
        seats: Seats::new(db.clone(), &config, ServerClock::System),
        config: Arc::new(config),
        db,
    }
}

fn app(routes: Router<AppState>) -> Router {
    finish(routes, state())
}

async fn send(app: Router, request: Request<Body>) -> (Response<Body>, Value) {
    let response = app.oneshot(request).await.unwrap();
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (Response::from_parts(parts, Body::empty()), json)
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

#[tokio::test]
async fn request_span_logs_id_and_route_template_not_path() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    let app = app(Router::new().route("/v1/events/{event_key}", get(|| async { "ok" })));

    let (response, _) = send(app.clone(), get_req("/v1/events/customer-key-1")).await;
    assert_eq!(response.status(), 200);
    let id = request_id(&response);
    assert!(id.starts_with("req_") && id.len() == 36, "{id}");
    let span = logs.completed(id);
    assert_eq!(span["name"], span::REQUEST);
    assert_eq!(span["method"], "GET");
    assert_eq!(span["route"], "/v1/events/{event_key}");
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

mod api;
