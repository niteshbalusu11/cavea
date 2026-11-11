//! Credentials, keys, workspaces and tenant isolation against PostgreSQL.
//!
//! `#[ignore]`d unless `CAVEA_TEST_DATABASE_URL` is set; each test creates and
//! drops its own database (AGENTS.md "Commands").

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::Method;
use sha2::{Digest as _, Sha256};
use sqlx::postgres::{PgConnectOptions, PgPool};
use sqlx::{AssertSqlSafe, ConnectOptions as _, Connection as _, Executor as _, PgConnection};

use super::*;
use crate::http::auth::PublishableKey;
use crate::http::{AdminKey, routes};
use crate::seats::{Seats, ServerClock};
use crate::store;

/// Obviously fake; at least 32 characters.
const ADMIN: &str = "test-admin-key-not-a-real-secret-000";

/// Run `body` with a pool on a new, uniquely named, migrated database, and drop
/// the database afterwards, also when `body` panics.
async fn with_db<F, Fut>(body: F)
where
    F: FnOnce(PgPool) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let url = std::env::var("CAVEA_TEST_DATABASE_URL")
        .expect("database tests need CAVEA_TEST_DATABASE_URL, a PostgreSQL server URL");
    let server = url
        .parse::<PgConnectOptions>()
        .unwrap()
        .disable_statement_logging();
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let name = format!(
        "cavea_test_api_{}_{}_{}",
        std::process::id(),
        nanos.subsec_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let mut conn = PgConnection::connect_with(&server).await.unwrap();
    // The name is generated above, never input.
    conn.execute(AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .await
        .unwrap();
    let options = server.clone().database(&name);
    store::migrate(&options).await.unwrap();
    let pool = store::connect(options, 4).await.unwrap();
    // The test's runtime is single-threaded, so the task logs to the test's
    // captured subscriber.
    let result = tokio::spawn(body(pool.clone())).await;
    pool.close().await;
    conn.execute(AssertSqlSafe(format!("DROP DATABASE {name} WITH (FORCE)")))
        .await
        .unwrap();
    if let Err(e) = result {
        std::panic::resume_unwind(e.into_panic());
    }
}

/// The real routes, plus `GET /test/public`: a bare publishable-key route for
/// the credential matrix.
fn api(pool: &PgPool, admin: Option<&str>) -> Router {
    api_with(pool, admin, ServerClock::System, "test").0
}

/// [`api`] on a given engine clock and node id, with its registry.
fn api_with(pool: &PgPool, admin: Option<&str>, clock: ServerClock, node: &str) -> (Router, Seats) {
    let mut state = state();
    state.db = pool.clone();
    let config = Arc::make_mut(&mut state.config);
    config.admin_key = admin.map(|a| a.parse::<AdminKey>().unwrap());
    config.node_id = node.to_owned();
    state.seats = Seats::new(pool.clone(), &state.config, clock);
    let seats = state.seats.clone();
    let public = get(|PublishableKey(auth): PublishableKey| async move {
        axum::Json(json!({ "workspace": auth.workspace().to_string() }))
    });
    (finish(routes().route("/test/public", public), state), seats)
}

async fn call(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (Response<Body>, Value) {
    call_from(app, method, uri, token, None, body).await
}

async fn call_from(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    origin: Option<&str>,
    body: Option<Value>,
) -> (Response<Body>, Value) {
    let headers: &[(header::HeaderName, &str)] = match origin {
        Some(origin) => &[(header::ORIGIN, origin)],
        None => &[],
    };
    call_with(app, method, uri, token, headers, body).await
}

async fn call_with(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    headers: &[(header::HeaderName, &str)],
    body: Option<Value>,
) -> (Response<Body>, Value) {
    let request = request(method, uri, token, headers, body.map(|b| b.to_string()));
    send(app.clone(), request).await
}

fn request(
    method: Method,
    uri: &str,
    token: Option<&str>,
    headers: &[(header::HeaderName, &str)],
    body: Option<String>,
) -> Request<Body> {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    for (name, value) in headers {
        request = request.header(name, *value);
    }
    match body {
        Some(body) => request
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body)),
        None => request.body(Body::empty()),
    }
    .unwrap()
}

/// A workspace made through the API, with one credential of each kind.
struct Tenant {
    id: String,
    secret: String,
    secret_id: String,
    publishable: String,
    publishable_id: String,
    /// A revoked secret key.
    revoked: String,
    /// A publishable key for routes to act on: `{key}` in a route table.
    spare_id: String,
    /// A chart with one published version: `{chart}` in a route table.
    chart: String,
    /// The draft of `chart`, as published: `{document}` in a route body.
    document: String,
    /// The public hash of that version: `{hash}` in a route table.
    hash: String,
    /// An event on `chart`: `{event}` in a route table.
    event: String,
    /// A hold token on `event`: `{token}` in a route table.
    token: String,
}

impl Tenant {
    async fn create(app: &Router, key: &str) -> Self {
        let (response, ws) = call(
            app,
            Method::POST,
            "/v1/workspaces",
            Some(ADMIN),
            Some(json!({ "key": key, "name": key })),
        )
        .await;
        assert_eq!(response.status(), 201, "{ws}");
        let secret = ws["secret_key"]["key"].as_str().unwrap().to_owned();
        let mint = async |kind: &str| {
            let body = json!({ "kind": kind, "name": kind });
            let (response, key) =
                call(app, Method::POST, "/v1/keys", Some(&secret), Some(body)).await;
            assert_eq!(response.status(), 201, "{key}");
            (
                key["key"].as_str().unwrap().to_owned(),
                key["id"].as_str().unwrap().to_owned(),
            )
        };
        let (publishable, publishable_id) = mint("publishable").await;
        let (_, spare_id) = mint("publishable").await;
        let (revoked, revoked_id) = mint("secret").await;
        let uri = format!("/v1/keys/{revoked_id}");
        let (response, _) = call(app, Method::DELETE, &uri, Some(&secret), None).await;
        assert_eq!(response.status(), 200);
        let chart = format!("chart-{key}");
        let document = tiny_document(key);
        let body = json!({ "key": chart, "name": key, "document": serde_json::from_str::<Value>(&document).unwrap() });
        let (response, created) =
            call(app, Method::POST, "/v1/charts", Some(&secret), Some(body)).await;
        assert_eq!(response.status(), 201, "{created}");
        let uri = format!("/v1/charts/{chart}/publish");
        let (response, version) = call(app, Method::POST, &uri, Some(&secret), None).await;
        assert_eq!(response.status(), 201, "{version}");
        let event = format!("event-{key}");
        let body = json!({ "key": event, "chart": chart });
        let (response, created) =
            call(app, Method::POST, "/v1/events", Some(&secret), Some(body)).await;
        assert_eq!(response.status(), 201, "{created}");
        let body = json!({ "event": event });
        let (response, token) = call(
            app,
            Method::POST,
            "/v1/hold-tokens",
            Some(&secret),
            Some(body),
        )
        .await;
        assert_eq!(response.status(), 201, "{token}");
        Self {
            event,
            token: token["hold_token"].as_str().unwrap().to_owned(),
            chart,
            document,
            hash: version["hash"].as_str().unwrap().to_owned(),
            id: ws["id"].as_str().unwrap().to_owned(),
            secret_id: ws["secret_key"]["id"].as_str().unwrap().to_owned(),
            secret,
            publishable,
            publishable_id,
            revoked,
            spare_id,
        }
    }

    /// Every credential this tenant holds: none may appear in any log.
    fn secrets(&self) -> [&str; 4] {
        [&self.secret, &self.publishable, &self.revoked, &self.token]
    }

    /// Every identifier of this tenant: none may appear in a response to
    /// another workspace's key.
    fn identifiers(&self) -> Vec<&str> {
        vec![
            &self.id,
            &self.secret_id,
            &self.publishable_id,
            &self.spare_id,
            &self.secret[..11],
            &self.publishable[..11],
            &self.chart,
            &self.hash,
            &self.event,
            &self.token,
        ]
    }

    /// Fill a route template's placeholders with this tenant's identifiers.
    fn path(&self, template: &str) -> String {
        template
            .replace("{key}", &self.spare_id)
            .replace("{chart}", &self.chart)
            .replace("{hash}", &self.hash)
            .replace("{event}", &self.event)
            .replace("{token}", &self.token)
            .replace("{document}", &self.document)
    }
}

/// A small valid chart document named `name`: a 2,000-seat synthetic
/// stadium, so that each name compiles to its own content hash.
fn tiny_document(name: &str) -> String {
    let mut document = cavea_testkit::synthetic::stadium(2_000, 1);
    document.name = name.to_owned();
    String::from_utf8(document.to_json().unwrap()).unwrap()
}

fn assert_no_secrets(logs: &Captured, secrets: &[&str]) {
    let text = logs.text();
    assert!(!text.is_empty());
    for secret in secrets.iter().chain(&[ADMIN, "Bearer"]) {
        assert!(!text.contains(secret), "{secret:.11}… logged");
    }
}

fn sha256(s: &str) -> Vec<u8> {
    Sha256::digest(s.as_bytes()).to_vec()
}

/// Create a workspace and its first key, rotate to a second key, revoke the
/// first; the key is shown once and only its hash and prefix are stored.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn key_lifecycle() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    let logs_in = logs.clone();
    with_db(async move |pool| {
        let logs = logs_in;
        let app = api(&pool, Some(ADMIN));
        let body = json!({ "key": "acme", "name": "Acme Tickets" });
        let (response, ws) = call(
            &app,
            Method::POST,
            "/v1/workspaces",
            Some(ADMIN),
            Some(body.clone()),
        )
        .await;
        assert_eq!(response.status(), 201, "{ws}");
        let created = &ws["secret_key"];
        let sk1 = created["key"].as_str().unwrap().to_owned();
        assert!(sk1.starts_with("sk_") && sk1.len() == 67, "{sk1:.11}");
        assert_eq!(created["prefix"], sk1[..11]);
        assert_eq!(created["kind"], "secret");
        assert_eq!(created["last_used_at"], Value::Null);
        assert!(ws["id"].as_str().unwrap().starts_with("ws_"));
        assert_eq!(logs.completed(request_id(&response))["workspace"], ws["id"]);

        // A second workspace with the same key is refused.
        let (response, problem) = call(
            &app,
            Method::POST,
            "/v1/workspaces",
            Some(ADMIN),
            Some(body),
        )
        .await;
        assert_problem(&response, &problem, Code::AlreadyExists);

        // Only the SHA-256 and the prefix are stored; no column holds the key.
        let (prefix, hash): (String, Vec<u8>) = sqlx::query_as("SELECT prefix, hash FROM api_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!((prefix.as_str(), hash), (&sk1[..11], sha256(&sk1)));
        let rows_with_key: i64 =
            sqlx::query_scalar("SELECT count(*) FROM api_keys k WHERE strpos(k::text, $1) > 0")
                .bind(&sk1[3..])
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(rows_with_key, 0);

        // The key authenticates; the list never shows it.
        let (response, list) = call(&app, Method::GET, "/v1/keys", Some(&sk1), None).await;
        assert_eq!(response.status(), 200, "{list}");
        assert_eq!(list["data"].as_array().unwrap().len(), 1);
        assert!(list["data"][0].get("key").is_none() && list["data"][0].get("hash").is_none());
        assert!(!list.to_string().contains(&sk1[11..]));
        assert_eq!(list["next"], Value::Null);
        assert_eq!(logs.completed(request_id(&response))["workspace"], ws["id"]);

        // last_used_at: set on first use, then at most once an hour.
        let last_used = async || -> String {
            sqlx::query_scalar("SELECT last_used_at::text FROM api_keys WHERE prefix = $1")
                .bind(&sk1[..11])
                .fetch_one(&pool)
                .await
                .unwrap()
        };
        let first = last_used().await;
        call(&app, Method::GET, "/v1/keys", Some(&sk1), None).await;
        assert_eq!(last_used().await, first);
        sqlx::query("UPDATE api_keys SET last_used_at = now() - interval '61 minutes'")
            .execute(&pool)
            .await
            .unwrap();
        let stale = last_used().await;
        call(&app, Method::GET, "/v1/keys", Some(&sk1), None).await;
        assert!(last_used().await > stale);

        // Rotation: a second secret key; both work during the overlap.
        let body = json!({ "kind": "secret", "name": "rotated" });
        let (response, created) =
            call(&app, Method::POST, "/v1/keys", Some(&sk1), Some(body)).await;
        assert_eq!(response.status(), 201, "{created}");
        let sk2 = created["key"].as_str().unwrap().to_owned();
        for key in [&sk1, &sk2] {
            let (response, _) = call(&app, Method::GET, "/v1/keys", Some(key), None).await;
            assert_eq!(response.status(), 200);
        }
        let sk1_id = ws["secret_key"]["id"].as_str().unwrap();
        let sk2_id = created["id"].as_str().unwrap();

        // A key cannot revoke itself; another revokes it, after which it is 401.
        let (response, problem) = call(
            &app,
            Method::DELETE,
            &format!("/v1/keys/{sk2_id}"),
            Some(&sk2),
            None,
        )
        .await;
        assert_problem(&response, &problem, Code::Forbidden);
        assert_eq!(problem["details"]["reason"], "self_revocation");
        let uri = format!("/v1/keys/{sk1_id}");
        let (response, revoked) = call(&app, Method::DELETE, &uri, Some(&sk2), None).await;
        assert_eq!(response.status(), 200, "{revoked}");
        assert!(revoked["revoked_at"].is_string() && revoked.get("key").is_none());
        let (response, again) = call(&app, Method::DELETE, &uri, Some(&sk2), None).await;
        assert_eq!(response.status(), 200);
        assert_eq!(again["revoked_at"], revoked["revoked_at"]);
        let (response, problem) = call(&app, Method::GET, "/v1/keys", Some(&sk1), None).await;
        assert_problem(&response, &problem, Code::Unauthorized);
        let (response, list) = call(&app, Method::GET, "/v1/keys", Some(&sk2), None).await;
        assert_eq!(response.status(), 200);
        assert_eq!(list["data"][0]["revoked_at"], revoked["revoked_at"]);
        assert_eq!(list["data"][1]["revoked_at"], Value::Null);

        // Pages of one.
        let (_, page) = call(&app, Method::GET, "/v1/keys?limit=1", Some(&sk2), None).await;
        assert_eq!(page["next"], sk1_id);
        let uri = format!("/v1/keys?limit=1&after={sk1_id}");
        let (_, page) = call(&app, Method::GET, &uri, Some(&sk2), None).await;
        assert_eq!(
            (&page["data"][0]["id"], &page["next"]),
            (&json!(sk2_id), &Value::Null)
        );
        let (response, problem) =
            call(&app, Method::GET, "/v1/keys?limit=0", Some(&sk2), None).await;
        assert_problem(&response, &problem, Code::InvalidRequest);
        assert_eq!(problem["details"]["reason"], "query");

        assert_no_secrets(&logs, &[&sk1, &sk2]);
    })
    .await;
}

/// Every kind of credential against a route of each kind; the origin
/// allow-list; and the admin endpoints with no admin key configured.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn credential_matrix() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    let logs_in = logs.clone();
    with_db(async move |pool| {
        let logs = logs_in;
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let unknown = format!("sk_{}", "0".repeat(64));

        // (header value, expected per route: admin, secret, publishable).
        // `Ok` is 200; `Err(code, reason)` is the problem.
        type Expect = Result<(), (Code, &'static str)>;
        const MISSING: Expect = Err((Code::Unauthorized, "missing"));
        const MALFORMED: Expect = Err((Code::Unauthorized, "malformed"));
        const INVALID: Expect = Err((Code::Unauthorized, "invalid"));
        const KIND: Expect = Err((Code::Forbidden, "key_kind"));
        let bearer = |t: &str| Some(format!("Bearer {t}"));
        let cases: [(Option<String>, [Expect; 3]); 10] = [
            (None, [MISSING; 3]),
            (Some(format!("Basic {}", a.secret)), [MALFORMED; 3]),
            (Some("Bearer".to_owned()), [MALFORMED; 3]),
            (Some(format!("Bearer {} x", a.secret)), [MALFORMED; 3]),
            (bearer(&unknown), [INVALID; 3]),
            (bearer("sk_short"), [INVALID; 3]),
            (bearer(&a.revoked), [INVALID; 3]),
            (bearer(ADMIN), [Ok(()), KIND, KIND]),
            (bearer(&a.secret), [KIND, Ok(()), KIND]),
            (bearer(&a.publishable), [KIND, KIND, Ok(())]),
        ];
        for (authorization, expected) in cases {
            for (uri, expect) in ["/v1/workspaces", "/v1/keys", "/test/public"]
                .iter()
                .zip(expected)
            {
                let mut request = Request::get(*uri);
                if let Some(value) = &authorization {
                    request = request.header(header::AUTHORIZATION, value);
                }
                let (response, body) =
                    send(app.clone(), request.body(Body::empty()).unwrap()).await;
                let label = authorization.as_deref().map(|a| &a[..a.len().min(10)]);
                match expect {
                    Ok(()) => assert_eq!(response.status(), 200, "{uri} {label:?}: {body}"),
                    Err((code, reason)) => {
                        assert_problem(&response, &body, code);
                        assert_eq!(body["details"]["reason"], reason, "{uri} {label:?}");
                    }
                }
            }
        }

        // Origins: a key with an allow-list, and one without.
        let body = json!({
            "kind": "publishable", "name": "web",
            "allowed_origins": ["https://Tickets.Example", "http://localhost:5173"],
        });
        let (response, created) =
            call(&app, Method::POST, "/v1/keys", Some(&a.secret), Some(body)).await;
        assert_eq!(response.status(), 201, "{created}");
        assert_eq!(
            created["allowed_origins"],
            json!(["https://tickets.example", "http://localhost:5173"])
        );
        let listed = created["key"].as_str().unwrap();
        let origins: [(&str, Option<&str>, bool); 7] = [
            (listed, None, true),
            (listed, Some("https://tickets.example"), true),
            (listed, Some("https://TICKETS.example"), true),
            (listed, Some("http://localhost:5173"), true),
            (listed, Some("https://evil.example"), false),
            (listed, Some("null"), false),
            (&a.publishable, Some("https://evil.example"), true),
        ];
        for (key, origin, allowed) in origins {
            let (response, body) =
                call_from(&app, Method::GET, "/test/public", Some(key), origin, None).await;
            if allowed {
                assert_eq!(response.status(), 200, "{origin:?}");
                assert_eq!(body["workspace"], a.id.as_str());
            } else {
                assert_problem(&response, &body, Code::Forbidden);
                assert_eq!(body["details"]["reason"], "origin", "{origin:?}");
            }
        }
        for (kind, origins) in [
            ("secret", json!(["https://tickets.example"])),
            ("publishable", json!([])),
            ("publishable", json!(["https://tickets.example/path"])),
            ("publishable", json!(["tickets.example"])),
        ] {
            let body = json!({ "kind": kind, "name": "x", "allowed_origins": origins });
            let (response, problem) =
                call(&app, Method::POST, "/v1/keys", Some(&a.secret), Some(body)).await;
            assert_problem(&response, &problem, Code::InvalidRequest);
            assert_eq!(problem["details"]["field"], "allowed_origins", "{origins}");
        }

        // No admin key configured: the admin endpoints refuse everything.
        let unset = api(&pool, None);
        let (response, body) = call(&unset, Method::GET, "/v1/workspaces", Some(ADMIN), None).await;
        assert_problem(&response, &body, Code::Unauthorized);

        let mut secrets = a.secrets().to_vec();
        secrets.push(listed);
        assert_no_secrets(&logs, &secrets);
    })
    .await;
}

/// What a route needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Needs {
    Admin,
    Secret,
    Publishable,
    /// A publishable key in the `key` query parameter: the realtime routes.
    QueryKey,
    /// No credential: a public file whose URL names it (08 §6.2).
    Nothing,
}

fn credential(tenant: &Tenant, needs: Needs) -> Option<&str> {
    match needs {
        Needs::Admin => Some(ADMIN),
        Needs::Secret => Some(&tenant.secret),
        Needs::Publishable | Needs::QueryKey => Some(&tenant.publishable),
        Needs::Nothing => None,
    }
}

/// One row per endpoint. A path's `{key}` is filled with an identifier of the
/// tenant under test (see [`Tenant::path`]); a later endpoint adds a row here,
/// and a placeholder and fixture to `Tenant` if it needs one.
const ROUTES: &[(Method, &str, Needs, u16, Option<&str>)] = &[
    (Method::GET, "/v1/workspaces", Needs::Admin, 200, None),
    (
        Method::POST,
        "/v1/workspaces",
        Needs::Admin,
        201,
        Some(r#"{"key":"c","name":"c"}"#),
    ),
    (Method::GET, "/v1/keys", Needs::Secret, 200, None),
    (
        Method::POST,
        "/v1/keys",
        Needs::Secret,
        201,
        Some(r#"{"kind":"secret","name":"n"}"#),
    ),
    (Method::DELETE, "/v1/keys/{key}", Needs::Secret, 200, None),
    (Method::GET, "/v1/charts", Needs::Secret, 200, None),
    (
        Method::POST,
        "/v1/charts",
        Needs::Secret,
        201,
        Some(r#"{"key":"new","name":"n"}"#),
    ),
    (Method::GET, "/v1/charts/{chart}", Needs::Secret, 200, None),
    (
        Method::GET,
        "/v1/charts/{chart}/draft",
        Needs::Secret,
        200,
        None,
    ),
    (
        Method::PUT,
        "/v1/charts/{chart}/draft",
        Needs::Secret,
        200,
        Some("{document}"),
    ),
    (
        Method::POST,
        "/v1/charts/{chart}/validate",
        Needs::Secret,
        200,
        None,
    ),
    // The draft is the published one: publishing it again creates nothing.
    (
        Method::POST,
        "/v1/charts/{chart}/publish",
        Needs::Secret,
        200,
        None,
    ),
    (
        Method::GET,
        "/v1/charts/{chart}/versions",
        Needs::Secret,
        200,
        None,
    ),
    (
        Method::GET,
        "/v1/public/charts/{hash}.ccf",
        Needs::Nothing,
        200,
        None,
    ),
    (Method::GET, "/v1/events", Needs::Secret, 200, None),
    (
        Method::POST,
        "/v1/events",
        Needs::Secret,
        201,
        Some(r#"{"key":"new-event","chart":"{chart}"}"#),
    ),
    (Method::GET, "/v1/events/{event}", Needs::Secret, 200, None),
    (
        Method::PATCH,
        "/v1/events/{event}",
        Needs::Secret,
        200,
        Some(r#"{"name":"renamed"}"#),
    ),
    (
        Method::POST,
        "/v1/hold-tokens",
        Needs::Secret,
        201,
        Some(r#"{"event":"{event}"}"#),
    ),
    (
        Method::GET,
        "/v1/hold-tokens/{token}",
        Needs::Secret,
        200,
        None,
    ),
    (
        Method::POST,
        "/v1/hold-tokens/{token}/extend",
        Needs::Secret,
        200,
        Some("{}"),
    ),
    (
        Method::POST,
        "/v1/events/{event}/hold",
        Needs::Secret,
        200,
        Some(r#"{"hold_token":"{token}","objects":{"version":"{hash}","idx":[0]}}"#),
    ),
    (
        Method::POST,
        "/v1/events/{event}/release",
        Needs::Secret,
        200,
        Some(r#"{"hold_token":"{token}"}"#),
    ),
    (
        Method::POST,
        "/v1/events/{event}/book",
        Needs::Secret,
        200,
        Some(r#"{"objects":{"version":"{hash}","idx":[1]},"order_ref":"o-1"}"#),
    ),
    (
        Method::POST,
        "/v1/events/{event}/release",
        Needs::Secret,
        200,
        Some(r#"{"objects":{"version":"{hash}","idx":[1]}}"#),
    ),
    (
        Method::POST,
        "/v1/events/{event}/change-status",
        Needs::Secret,
        200,
        Some(r#"{"objects":{"version":"{hash}","idx":[2]},"status":"blocked"}"#),
    ),
    (
        Method::GET,
        "/v1/public/events/{event}",
        Needs::Publishable,
        200,
        None,
    ),
    (
        Method::POST,
        "/v1/public/hold-tokens",
        Needs::Publishable,
        201,
        Some(r#"{"event":"{event}"}"#),
    ),
    // The token goes in `Cavea-Hold-Token` (see `HEADER_TOKEN`).
    (
        Method::GET,
        "/v1/public/hold-tokens/current",
        Needs::Publishable,
        200,
        None,
    ),
    (
        Method::POST,
        "/v1/public/events/{event}/hold",
        Needs::Publishable,
        200,
        Some(r#"{"hold_token":"{token}","objects":{"version":"{hash}","idx":[3]}}"#),
    ),
    (
        Method::POST,
        "/v1/public/events/{event}/release",
        Needs::Publishable,
        200,
        Some(r#"{"hold_token":"{token}"}"#),
    ),
    (
        Method::GET,
        "/v1/public/events/{event}/state",
        Needs::QueryKey,
        200,
        None,
    ),
    // Without an upgrade; a WebSocket naming another workspace's event is in
    // `realtime::websocket_follows_the_event`.
    (Method::GET, "/v1/public/rt", Needs::QueryKey, 400, None),
    (
        Method::DELETE,
        "/v1/hold-tokens/{token}",
        Needs::Secret,
        200,
        None,
    ),
    (
        Method::DELETE,
        "/v1/events/{event}",
        Needs::Secret,
        200,
        None,
    ),
    (
        Method::DELETE,
        "/v1/charts/{chart}",
        Needs::Secret,
        200,
        None,
    ),
];

/// `path` with `key` in its query string.
fn with_key(path: &str, key: &str) -> String {
    let join = if path.contains('?') { '&' } else { '?' };
    format!("{path}{join}key={key}")
}

/// Placeholders that name an identifier of the tenant under test.
const IDENTIFIERS: [&str; 5] = ["{key}", "{chart}", "{hash}", "{event}", "{token}"];

/// The route that takes the tenant's hold token in `Cavea-Hold-Token`.
const HEADER_TOKEN: &str = "/v1/public/hold-tokens/current";

const HOLD_TOKEN: header::HeaderName = header::HeaderName::from_static("cavea-hold-token");

/// T5: every route, called with each wrong kind of credential, is refused;
/// called with workspace B's key of the right kind and workspace A's
/// identifiers, it is `404` if it names one, and never reveals anything of A.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn tenant_isolation() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    let logs_in = logs.clone();
    with_db(async move |pool| {
        let logs = logs_in;
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let b = Tenant::create(&app, "b").await;
        for (method, template, needs, ok, body) in ROUTES {
            let names_identifier = *template == HEADER_TOKEN
                || IDENTIFIERS
                    .iter()
                    .any(|p| template.contains(p) || body.is_some_and(|b| b.contains(p)));
            let body = body.map(|b| serde_json::from_str::<Value>(&a.path(b)).unwrap());
            let path = a.path(template);
            // `If-Match: *` and A's hold token for the routes that need them;
            // the others ignore them.
            let headers = [(header::IF_MATCH, "*"), (HOLD_TOKEN, a.token.as_str())];
            let call = async |token: Option<&str>| {
                // A realtime route takes whatever key it is given in its URL.
                let (path, token) = match (needs, token) {
                    (Needs::QueryKey, Some(key)) => (with_key(&path, key), None),
                    _ => (path.clone(), token),
                };
                call_with(&app, method.clone(), &path, token, &headers, body.clone()).await
            };
            if *needs == Needs::Nothing {
                // Any credential, of any workspace, or none: the same answer.
                let tokens = [ADMIN, &a.secret, &a.publishable, &b.secret, &b.publishable];
                for token in tokens.map(Some).into_iter().chain([None]) {
                    let (response, _) = call(token).await;
                    assert_eq!(response.status().as_u16(), *ok, "{method} {template}");
                }
                continue;
            }
            let (response, problem) = call(None).await;
            assert_problem(&response, &problem, Code::Unauthorized);
            for wrong in [Needs::Admin, Needs::Secret, Needs::Publishable] {
                if wrong != *needs && !(wrong == Needs::Publishable && *needs == Needs::QueryKey) {
                    let (response, problem) = call(credential(&a, wrong)).await;
                    assert_problem(&response, &problem, Code::Forbidden);
                }
            }
            if *needs != Needs::Admin {
                let (response, foreign) = call(credential(&b, *needs)).await;
                if names_identifier {
                    assert_problem(&response, &foreign, Code::NotFound);
                } else {
                    assert_eq!(
                        response.status().as_u16(),
                        *ok,
                        "{method} {template}: {foreign}"
                    );
                }
                let text = foreign.to_string();
                for id in a.identifiers() {
                    assert!(!text.contains(id), "{method} {template} revealed {id:.11}");
                }
            }
            let (response, own) = call(credential(&a, *needs)).await;
            assert_eq!(
                response.status().as_u16(),
                *ok,
                "{method} {template}: {own}"
            );
        }
        let secrets: Vec<&str> = a.secrets().into_iter().chain(b.secrets()).collect();
        assert_no_secrets(&logs, &secrets);
    })
    .await;
}

mod charts;
mod realtime;
mod seats;
