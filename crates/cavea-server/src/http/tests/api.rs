//! Credentials, keys, workspaces and tenant isolation against PostgreSQL.
//!
//! `#[ignore]`d unless `CAVEA_TEST_DATABASE_URL` is set; each test creates and
//! drops its own database (AGENTS.md "Commands").

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::http::Method;
use sha2::{Digest as _, Sha256};
use sqlx::postgres::{PgConnectOptions, PgPool};
use sqlx::{AssertSqlSafe, ConnectOptions as _, Connection as _, Executor as _, PgConnection};

use super::*;
use crate::http::auth::PublishableKey;
use crate::http::openapi::Needs;
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
    state.seats = Seats::new(pool.clone(), &state.config, clock, None);
    let seats = state.seats.clone();
    let public = get(|PublishableKey(auth): PublishableKey| async move {
        axum::Json(json!({ "workspace": auth.workspace().to_string() }))
    });
    (
        finish(routes().into_router().route("/test/public", public), state),
        seats,
    )
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
        Self::create_as(app, key, key).await
    }

    /// A workspace `key` whose chart and event keys are `chart-{name}` and
    /// `event-{name}`: two workspaces may share them. The chart's content,
    /// and so its hash, is the workspace's own.
    async fn create_as(app: &Router, key: &str, name: &str) -> Self {
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
        let chart = format!("chart-{name}");
        let document = tiny_document(key);
        let body = json!({ "key": chart, "name": key, "document": serde_json::from_str::<Value>(&document).unwrap() });
        let (response, created) =
            call(app, Method::POST, "/v1/charts", Some(&secret), Some(body)).await;
        assert_eq!(response.status(), 201, "{created}");
        let uri = format!("/v1/charts/{chart}/publish");
        let (response, version) = call(app, Method::POST, &uri, Some(&secret), None).await;
        assert_eq!(response.status(), 201, "{version}");
        let event = format!("event-{name}");
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
            .replace("{workspace}", &self.id)
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
/// Readiness follows the database: refusing connections and ending the
/// pool's, `/readyz` turns `503` within a few probes, and `200` again once it
/// accepts them; `/healthz` stays `ok` throughout.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn readiness_follows_the_database() {
    with_db(|pool| async move {
        let mut state = state();
        state.db = pool.clone();
        let probe = tokio::spawn(state.health.clone().watch(pool.clone()));
        let app = finish(routes().into_router(), state);
        let status = async |path: &str| call(&app, Method::GET, path, None, None).await.0.status();
        let becomes = async |want: u16| {
            for _ in 0..100 {
                if status("/readyz").await == want {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            panic!("/readyz never answered {want}");
        };
        becomes(200).await;
        let name: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(&pool)
            .await
            .unwrap();
        let url = std::env::var("CAVEA_TEST_DATABASE_URL").unwrap();
        let server = url.parse::<PgConnectOptions>().unwrap();
        let mut admin = PgConnection::connect_with(&server).await.unwrap();
        // The name is generated by `with_db`, never input.
        admin
            .execute(AssertSqlSafe(format!(
                "ALTER DATABASE {name} ALLOW_CONNECTIONS false"
            )))
            .await
            .unwrap();
        sqlx::query("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = $1")
            .bind(&name)
            .execute(&mut admin)
            .await
            .unwrap();
        becomes(503).await;
        assert_eq!(status("/healthz").await, 200);
        admin
            .execute(AssertSqlSafe(format!(
                "ALTER DATABASE {name} ALLOW_CONNECTIONS true"
            )))
            .await
            .unwrap();
        becomes(200).await;
        probe.abort();
    })
    .await;
}

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

/// The operator mints a secret key for an existing workspace, shown once;
/// an unknown or malformed workspace is `404`.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn admin_mints_a_secret_key() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    let logs_in = logs.clone();
    with_db(async move |pool| {
        let logs = logs_in;
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let uri = format!("/v1/workspaces/{}/keys", a.id);
        let body = json!({ "name": "recovery" });
        let (response, created) =
            call(&app, Method::POST, &uri, Some(ADMIN), Some(body.clone())).await;
        assert_eq!(response.status(), 201, "{created}");
        assert_eq!(
            (&created["kind"], &created["name"]),
            (&json!("secret"), &json!("recovery"))
        );
        let key = created["key"].as_str().unwrap().to_owned();
        let (response, list) = call(&app, Method::GET, "/v1/keys", Some(&key), None).await;
        assert_eq!(response.status(), 200, "{list}");
        assert!(
            list["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|k| k["id"] == created["id"] && k.get("key").is_none())
        );
        let unknown = format!("/v1/workspaces/ws_{}/keys", "0".repeat(32));
        for uri in [unknown.as_str(), "/v1/workspaces/acme/keys"] {
            let (response, p) =
                call(&app, Method::POST, uri, Some(ADMIN), Some(body.clone())).await;
            assert_problem(&response, &p, Code::NotFound);
        }
        let (response, p) = call(
            &app,
            Method::POST,
            &uri,
            Some(ADMIN),
            Some(json!({ "name": "" })),
        )
        .await;
        assert_problem(&response, &p, Code::InvalidRequest);
        let mut secrets = a.secrets().to_vec();
        secrets.push(&key);
        assert_no_secrets(&logs, &secrets);
    })
    .await;
}

/// The origin allow-list of publishable keys, and the admin endpoints with no
/// admin key configured. Every other credential is in [`authorisation_matrix`].
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn origins_and_unset_admin_key() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    let logs_in = logs.clone();
    with_db(async move |pool| {
        let logs = logs_in;
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;

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

/// A row's route as the router has it: its template without the query, and
/// the fixtures' placeholders named as the route's parameters.
fn route_of(template: &str) -> String {
    template
        .split('?')
        .next()
        .unwrap()
        .replace("{hash}.ccf", "{file}")
        .replace("{key}", "{id}")
}

/// The credential a row's operation takes, and whether it takes a session
/// token: from the route table itself.
fn op_of(method: &Method, template: &str) -> (Needs, bool) {
    let path = route_of(template);
    let ops = routes();
    let op = ops
        .ops()
        .iter()
        .find(|op| op.method == *method && op.path == path)
        .unwrap_or_else(|| panic!("{method} {path} is not a registered route"));
    (op.needs, op.takes_session())
}

/// One row per operation, with what a successful call sends and answers. A
/// path's `{key}` is filled with an identifier of the tenant under test (see
/// [`Tenant::path`]). [`every_route_has_a_row`]
/// fails until a new route has its row.
const ROUTES: &[(Method, &str, u16, Option<&str>)] = &[
    (Method::GET, "/healthz", 200, None),
    (Method::GET, "/readyz", 200, None),
    (Method::GET, "/v1/openapi.json", 200, None),
    (Method::GET, "/metrics", 200, None),
    (Method::GET, "/v1/workspaces", 200, None),
    (
        Method::POST,
        "/v1/workspaces",
        201,
        Some(r#"{"key":"c","name":"c"}"#),
    ),
    (
        Method::POST,
        "/v1/workspaces/{workspace}/keys",
        201,
        Some(r#"{"name":"recovery"}"#),
    ),
    (Method::GET, "/v1/keys", 200, None),
    (
        Method::POST,
        "/v1/keys",
        201,
        Some(r#"{"kind":"secret","name":"n"}"#),
    ),
    (Method::DELETE, "/v1/keys/{key}", 200, None),
    (Method::GET, "/v1/charts", 200, None),
    (
        Method::POST,
        "/v1/charts",
        201,
        Some(r#"{"key":"new","name":"n"}"#),
    ),
    (Method::GET, "/v1/charts/{chart}", 200, None),
    (Method::GET, "/v1/charts/{chart}/draft", 200, None),
    (
        Method::PUT,
        "/v1/charts/{chart}/draft",
        200,
        Some("{document}"),
    ),
    (Method::POST, "/v1/charts/{chart}/validate", 200, None),
    // The draft is the published one: publishing it again creates nothing.
    (Method::POST, "/v1/charts/{chart}/publish", 200, None),
    (Method::GET, "/v1/charts/{chart}/versions", 200, None),
    (Method::GET, "/v1/public/charts/{hash}.ccf", 200, None),
    (Method::GET, "/v1/events", 200, None),
    (
        Method::POST,
        "/v1/events",
        201,
        Some(r#"{"key":"new-event","chart":"{chart}"}"#),
    ),
    (Method::GET, "/v1/events/{event}", 200, None),
    (
        Method::PATCH,
        "/v1/events/{event}",
        200,
        Some(r#"{"name":"renamed"}"#),
    ),
    (
        Method::POST,
        "/v1/hold-tokens",
        201,
        Some(r#"{"event":"{event}"}"#),
    ),
    (Method::GET, "/v1/hold-tokens/{token}", 200, None),
    (
        Method::POST,
        "/v1/hold-tokens/{token}/extend",
        200,
        Some("{}"),
    ),
    (
        Method::POST,
        "/v1/events/{event}/hold",
        200,
        Some(r#"{"hold_token":"{token}","objects":{"version":"{hash}","idx":[0]}}"#),
    ),
    (
        Method::POST,
        "/v1/events/{event}/release",
        200,
        Some(r#"{"hold_token":"{token}"}"#),
    ),
    (
        Method::POST,
        "/v1/events/{event}/book",
        200,
        Some(r#"{"objects":{"version":"{hash}","idx":[1]},"order_ref":"o-1"}"#),
    ),
    (
        Method::POST,
        "/v1/events/{event}/release",
        200,
        Some(r#"{"objects":{"version":"{hash}","idx":[1]}}"#),
    ),
    (
        Method::POST,
        "/v1/events/{event}/change-status",
        200,
        Some(r#"{"objects":{"version":"{hash}","idx":[2]},"status":"blocked"}"#),
    ),
    (
        Method::GET,
        "/v1/events/{event}/objects?label=Nope",
        200,
        None,
    ),
    (
        Method::GET,
        "/v1/events/{event}/objects?status=held&limit=5",
        200,
        None,
    ),
    (Method::GET, "/v1/public/events/{event}", 200, None),
    (
        Method::POST,
        "/v1/public/hold-tokens",
        201,
        Some(r#"{"event":"{event}"}"#),
    ),
    // The token goes in `Cavea-Hold-Token` (see `HEADER_TOKEN`).
    (Method::GET, "/v1/public/hold-tokens/current", 200, None),
    (
        Method::POST,
        "/v1/public/events/{event}/hold",
        200,
        Some(r#"{"hold_token":"{token}","objects":{"version":"{hash}","idx":[3]}}"#),
    ),
    (
        Method::POST,
        "/v1/public/events/{event}/release",
        200,
        Some(r#"{"hold_token":"{token}"}"#),
    ),
    (Method::GET, "/v1/public/events/{event}/state", 200, None),
    (
        Method::GET,
        "/v1/public/events/{event}/state?after_seq=1",
        200,
        None,
    ),
    // Read up to the response head: the body is a stream.
    (Method::GET, "/v1/public/events/{event}/stream", 200, None),
    // Without an upgrade; a WebSocket naming another workspace's event is in
    // `realtime::websocket_follows_the_event`.
    (Method::GET, "/v1/public/rt", 400, None),
    (Method::DELETE, "/v1/hold-tokens/{token}", 200, None),
    (Method::DELETE, "/v1/events/{event}", 200, None),
    (Method::DELETE, "/v1/charts/{chart}", 200, None),
];

/// 13 §10 "tenant isolation": two workspaces with identical chart and event
/// keys. Every read of a route naming only those keys answers each workspace
/// with its own, revealing none of the other's identifiers; a hold in one is
/// not in the other's counts or objects; archiving one's chart leaves the
/// other's.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn identical_keys_never_cross() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create_as(&app, "a", "same").await;
        let b = Tenant::create_as(&app, "b", "same").await;
        assert_eq!((&a.chart, &a.event), (&b.chart, &b.event));
        assert_ne!(a.hash, b.hash);
        // B holds a seat in its event of the same key.
        let uri = format!("/v1/events/{}/hold", b.event);
        let body = json!({ "hold_token": b.token, "objects": { "version": b.hash, "idx": [0] } });
        let (response, held) = call(&app, Method::POST, &uri, Some(&b.secret), Some(body)).await;
        assert_eq!(response.status(), 200, "{held}");

        let reads = ROUTES.iter().filter(|(method, template, ..)| {
            *method == Method::GET
                && (template.contains("{chart}") || template.contains("{event}"))
                && !["{key}", "{hash}", "{token}"]
                    .iter()
                    .any(|p| template.contains(p))
        });
        let mut checked = 0;
        for (method, template, ..) in reads {
            let (needs, _) = op_of(method, template);
            let path = a.path(template);
            for (mine, theirs) in [(&a, &b), (&b, &a)] {
                let key = if needs == Needs::Secret {
                    &mine.secret
                } else {
                    &mine.publishable
                };
                let (path, token) = match needs {
                    Needs::QueryKey => (with_key(&path, key), None),
                    _ => (path.clone(), Some(key.as_str())),
                };
                let (response, answer) = call(&app, method.clone(), &path, token, None).await;
                // Polling after seq 1 is `204` for A, which published nothing
                // since, and a delta for B, which held a seat.
                assert!(response.status().is_success(), "{template}: {answer}");
                let text = answer.to_string();
                for id in theirs
                    .identifiers()
                    .into_iter()
                    .filter(|id| *id != a.chart && *id != a.event)
                {
                    assert!(!text.contains(id), "{template} crossed to {id:.11}");
                }
                if text.contains("\"hash\"") {
                    assert!(text.contains(&mine.hash), "{template}: {text}");
                }
                checked += 1;
            }
        }
        assert!(checked >= 10, "{checked}");

        let held_in = async |t: &Tenant| {
            let uri = format!("/v1/events/{}", t.event);
            let (_, event) = call(&app, Method::GET, &uri, Some(&t.secret), None).await;
            let uri = format!("/v1/events/{}/objects?status=held", t.event);
            let (_, objects) = call(&app, Method::GET, &uri, Some(&t.secret), None).await;
            (
                event["counts"]["held"].clone(),
                objects["data"].as_array().unwrap().len(),
            )
        };
        assert_eq!(held_in(&a).await, (json!(0), 0));
        assert_eq!(held_in(&b).await, (json!(1), 1));

        let uri = format!("/v1/charts/{}", b.chart);
        let (response, archived) = call(&app, Method::DELETE, &uri, Some(&b.secret), None).await;
        assert!(response.status() == 200 && archived["archived_at"].is_string());
        let (_, chart) = call(&app, Method::GET, &uri, Some(&a.secret), None).await;
        assert_eq!(chart["archived_at"], Value::Null);
    })
    .await;
}

/// TST-026: a new route cannot be left out of the matrix, nor a row outlive
/// its route.
#[test]
fn every_route_has_a_row() {
    use std::collections::BTreeSet;
    let registered: BTreeSet<(String, String)> = routes()
        .ops()
        .iter()
        .map(|op| (op.method.to_string(), op.path.to_owned()))
        .collect();
    let rows: BTreeSet<(String, String)> = ROUTES
        .iter()
        .map(|(method, template, ..)| (method.to_string(), route_of(template)))
        .collect();
    assert_eq!(rows, registered);
}

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
const SESSION: header::HeaderName = header::HeaderName::from_static("cavea-session");

/// A credential, as one cell's column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cred {
    None,
    /// `Basic …`, `Bearer` alone, `Bearer a b`; in a URL, a percent-encoded
    /// key, an empty one, two.
    Malformed(usize),
    /// A key that was never issued, one too short to be one, a revoked one.
    Invalid(usize),
    Admin,
    Secret,
    Publishable,
    ForeignSecret,
    ForeignPublishable,
}

/// In the order they are tried: refusals change nothing, so the tenant's own
/// keys, of which one succeeds and may change state (a key revoked, an event
/// archived), come last; a refusal after it is decided before the handler.
const CREDS: [Cred; 12] = [
    Cred::None,
    Cred::Malformed(0),
    Cred::Malformed(1),
    Cred::Malformed(2),
    Cred::Invalid(0),
    Cred::Invalid(1),
    Cred::Invalid(2),
    Cred::ForeignSecret,
    Cred::ForeignPublishable,
    Cred::Admin,
    Cred::Secret,
    Cred::Publishable,
];

/// The declared table (13 §10): what each credential gets on a route that
/// needs `needs`, as the refusal's code and `reason`. `None` is allowed: the
/// route's success, or for another workspace's key `404` when the route names
/// one of the tenant's identifiers.
fn expect(needs: Needs, cred: Cred) -> Option<(Code, &'static str)> {
    use Cred as C;
    match (needs, cred) {
        (Needs::Nothing, _)
        | (Needs::Admin, C::Admin)
        | (Needs::Secret, C::Secret | C::ForeignSecret)
        | (Needs::Publishable | Needs::QueryKey, C::Publishable | C::ForeignPublishable) => None,
        (_, C::None) => Some((Code::Unauthorized, "missing")),
        (_, C::Malformed(_)) => Some((Code::Unauthorized, "malformed")),
        (_, C::Invalid(_)) => Some((Code::Unauthorized, "invalid")),
        _ => Some((Code::Forbidden, "key_kind")),
    }
}

/// 13 §10 and T5: every route × every credential, compared with [`expect`];
/// with workspace B's key of the right kind and workspace A's identifiers,
/// `404` if the route names one, never revealing anything of A; an expired or
/// another workspace's session token on the routes that take one; and
/// another event's hold token where a body names a token and an event.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn authorisation_matrix() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    let logs_in = logs.clone();
    with_db(async move |pool| {
        let logs = logs_in;
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let b = Tenant::create(&app, "b").await;
        // A second event of A's, with a token: another event's hold token.
        let (response, _) = call(
            &app,
            Method::POST,
            "/v1/events",
            Some(&a.secret),
            Some(json!({ "key": "other", "chart": a.chart })),
        )
        .await;
        assert_eq!(response.status(), 201);
        let (_, other) = call(
            &app,
            Method::POST,
            "/v1/hold-tokens",
            Some(&a.secret),
            Some(json!({ "event": "other" })),
        )
        .await;
        let other_token = other["hold_token"].as_str().unwrap().to_owned();
        let unknown = format!("sk_{}", "0".repeat(64));
        let malformed = [
            format!("Basic {}", a.secret),
            "Bearer".to_owned(),
            format!("Bearer {} x", a.secret),
        ];
        let invalid = [unknown.as_str(), "sk_short", a.revoked.as_str()];
        let exp = |secs: u64| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                - secs
        };
        let expired = session::signed(
            &a.secret,
            &a.secret_id,
            &json!({ "sub": "s", "event": a.event, "exp": exp(3_600) }),
        );
        let foreign_session = session::signed(
            &b.secret,
            &b.secret_id,
            &json!({ "sub": "s", "event": a.event, "exp": exp(0) + 600 }),
        );
        let mut cells = 0;
        for (method, template, ok, body) in ROUTES {
            let (needs, takes_session) = op_of(method, template);
            let names_identifier = *template == HEADER_TOKEN
                || IDENTIFIERS
                    .iter()
                    .any(|p| template.contains(p) || body.is_some_and(|b| b.contains(p)));
            let path = a.path(template);
            let send_with = async |authorization: Option<&str>,
                                   extra: &[(header::HeaderName, &str)],
                                   body: Option<&str>| {
                // A realtime route takes whatever key it is given in its URL.
                let (path, authorization) = match (needs, authorization) {
                    (Needs::QueryKey, Some(value)) => {
                        let key = value.strip_prefix("Bearer ").unwrap_or(value);
                        (with_key(&path, key), None)
                    }
                    _ => (path.clone(), authorization),
                };
                let mut request = Request::builder().method(method.clone()).uri(&path);
                if let Some(value) = authorization {
                    request = request.header(header::AUTHORIZATION, value);
                }
                // `If-Match: *` and A's hold token for the routes that need
                // them; the others ignore them.
                request = request
                    .header(header::IF_MATCH, "*")
                    .header(&HOLD_TOKEN, a.token.as_str());
                for (name, value) in extra {
                    request = request.header(name, *value);
                }
                let request = match body {
                    Some(body) => request
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(a.path(body))),
                    None => request.body(Body::empty()),
                };
                send(app.clone(), request.unwrap()).await
            };
            for cred in CREDS {
                let value = match cred {
                    Cred::None => None,
                    Cred::Malformed(i) if needs == Needs::QueryKey => {
                        Some(["pk_%30", "", "pk_0&key=pk_1"][i].to_owned())
                    }
                    Cred::Malformed(i) => Some(malformed[i].clone()),
                    Cred::Invalid(i) => Some(format!("Bearer {}", invalid[i])),
                    Cred::Admin => Some(format!("Bearer {ADMIN}")),
                    Cred::Secret => Some(format!("Bearer {}", a.secret)),
                    Cred::Publishable => Some(format!("Bearer {}", a.publishable)),
                    Cred::ForeignSecret => Some(format!("Bearer {}", b.secret)),
                    Cred::ForeignPublishable => Some(format!("Bearer {}", b.publishable)),
                };
                let foreign = matches!(cred, Cred::ForeignSecret | Cred::ForeignPublishable);
                let expected = expect(needs, cred);
                let (response, answer) = send_with(value.as_deref(), &[], *body).await;
                cells += 1;
                let at = format!("{method} {template} with {cred:?}");
                match expected {
                    Some((code, reason)) => {
                        assert_problem(&response, &answer, code);
                        assert_eq!(answer["details"]["reason"], reason, "{at}");
                    }
                    None if foreign && names_identifier && needs != Needs::Nothing => {
                        assert_problem(&response, &answer, Code::NotFound);
                    }
                    None => assert_eq!(response.status().as_u16(), *ok, "{at}: {answer}"),
                }
                if foreign {
                    let text = answer.to_string();
                    for id in a.identifiers() {
                        assert!(!text.contains(id), "{at} revealed {id:.11}");
                    }
                }
            }
            // An expired session token, and one another workspace signed.
            if takes_session {
                for (session, reason) in [(&expired, "expired"), (&foreign_session, "invalid")] {
                    let bearer = format!("Bearer {}", a.publishable);
                    let (response, answer) =
                        send_with(Some(&bearer), &[(SESSION, session)], *body).await;
                    cells += 1;
                    assert_problem(&response, &answer, Code::Unauthorized);
                    assert_eq!(answer["details"]["reason"], "session_token");
                    assert_eq!(answer["details"]["session"], reason, "{method} {template}");
                }
            }
            // Another event's hold token, where the body names a token for
            // the event in the path.
            if template.contains("{event}") && body.is_some_and(|b| b.contains("{token}")) {
                let key = if needs == Needs::Secret {
                    &a.secret
                } else {
                    &a.publishable
                };
                let bearer = format!("Bearer {key}");
                let body = body.map(|b| b.replace("{token}", &other_token));
                let (response, answer) = send_with(Some(&bearer), &[], body.as_deref()).await;
                cells += 1;
                assert_problem(&response, &answer, Code::NotFound);
                assert_eq!(
                    answer["details"]["missing"], "hold_token",
                    "{method} {template}"
                );
            }
        }
        assert!(cells > 500, "{cells} cells");
        let mut secrets: Vec<&str> = a.secrets().into_iter().chain(b.secrets()).collect();
        secrets.extend([
            other_token.as_str(),
            expired.as_str(),
            foreign_session.as_str(),
        ]);
        assert_no_secrets(&logs, &secrets);
    })
    .await;
}

mod charts;
mod limits;
mod objects;
mod realtime;
mod seats;
mod session;
mod verify;
