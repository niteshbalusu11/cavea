//! The console's people against PostgreSQL (09 §2, §10; 12 §3 T9): setup,
//! sign-in, sessions and their cookie, the back-off, and console users as
//! actors on `/v1`. The role table on every route is the matrix's
//! (`authorisation_matrix`).

use std::net::SocketAddr;

use axum::extract::ConnectInfo;
use cavea_engine::{ManualClock, Timestamp};

use super::*;
use crate::console::guard::PAIR_FREE;
use crate::limiter::Limiter;

/// A console request: JSON `body`, the session's cookie and CSRF token,
/// `workspace` named, from the socket `peer`.
async fn console_call(
    app: &Router,
    method: Method,
    uri: &str,
    session: Option<&SessionId>,
    workspace: Option<&str>,
    body: Option<Value>,
) -> (Response<Body>, Value) {
    let headers = session.map(|s| console(s, workspace)).unwrap_or_default();
    call_with(app, method, uri, None, &refs(&headers), body).await
}

fn sign_in_body(email: &str, password: &str) -> Value {
    json!({ "email": email, "password": password })
}

/// The session a reply's `Set-Cookie` starts.
fn cookie_session(response: &Response<Body>) -> SessionId {
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    let value = cookie
        .strip_prefix("__Host-cavea_console=")
        .and_then(|c| c.split(';').next())
        .unwrap();
    SessionId::parse(value).unwrap()
}

async fn setup_token(pool: &PgPool) -> String {
    let link = crate::console::setup::prepare(pool, Some("https://seats.example.com"))
        .await
        .unwrap()
        .unwrap();
    link.strip_prefix("https://seats.example.com/console/setup#token=")
        .unwrap()
        .to_owned()
}

/// CON-002: while no user exists, start-up stores a token and logs one link;
/// of two setups at once with it, exactly one creates the owner, who owns
/// every workspace and is signed in; the token is burnt, and once an owner
/// exists start-up stores and logs nothing.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn setup_creates_one_owner_once() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    let logs_in = logs.clone();
    with_db(async move |pool| {
        let logs = logs_in;
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let b = Tenant::create(&app, "b").await;
        // Without CAVEA_PUBLIC_URL, the path; a second start replaces it.
        let link = crate::console::setup::prepare(&pool, None).await.unwrap().unwrap();
        assert!(link.starts_with("/console/setup#token="), "{link}");
        crate::console::setup::announce(&link, false);
        let token = setup_token(&pool).await;
        let stale = link.strip_prefix("/console/setup#token=").unwrap();
        let setup = |token: &str, email: &str| {
            json!({ "token": token, "email": email, "password": "not-a-real-password-0000" })
        };

        let (response, problem) = call(
            &app,
            Method::POST,
            "/console/api/setup",
            None,
            Some(setup(stale, "Owner@Example.org")),
        )
        .await;
        assert_problem(&response, &problem, Code::Unauthorized);
        assert_eq!(problem["details"]["reason"], "setup_token");

        let first = call(
            &app,
            Method::POST,
            "/console/api/setup",
            None,
            Some(setup(&token, "Owner@Example.org")),
        );
        let second = call(
            &app,
            Method::POST,
            "/console/api/setup",
            None,
            Some(setup(&token, "other@example.org")),
        );
        let ((r1, me1), (r2, me2)) = tokio::join!(first, second);
        let (won, me, lost) = if r1.status() == 201 {
            (r1, me1, me2)
        } else {
            (r2, me2, me1)
        };
        assert_eq!(won.status(), 201, "{me}");
        assert_eq!(lost["details"]["reason"], "setup_token", "{lost}");
        let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(users, 1);
        let roles: Vec<(&Value, &Value)> = me["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| (&w["id"], &w["role"]))
            .collect();
        assert_eq!(
            roles,
            [(&json!(a.id), &json!("owner")), (&json!(b.id), &json!("owner"))]
        );
        // Signed in: the cookie's session reads `me`.
        let id = cookie_session(&won);
        let (response, again) =
            console_call(&app, Method::GET, "/console/api/me", Some(&id), None, None).await;
        assert_eq!(response.status(), 200);
        assert_eq!(again["user"], me["user"]);

        // Burnt, and nothing more once an owner exists.
        let (response, _) = call(
            &app,
            Method::POST,
            "/console/api/setup",
            None,
            Some(setup(&token, "late@example.org")),
        )
        .await;
        assert_eq!(response.status(), 401);
        assert_eq!(crate::console::setup::prepare(&pool, None).await.unwrap(), None);

        // The link was logged once, as one record, and the token nowhere else.
        let records = logs.records();
        let with_link: Vec<_> = records
            .iter()
            .filter(|r| r.to_string().contains(stale))
            .collect();
        assert_eq!(with_link.len(), 1);
        assert_eq!(with_link[0]["setup_link"], link);
        assert_eq!(with_link[0]["level"], "INFO");
        assert!(!logs.text().contains(&token));
        assert_no_secrets(&logs, &["not-a-real-password", "example.org", id.as_str(), &id.csrf()]);
    })
    .await;
}

/// With no workspace, setup creates the first one, named by the request.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn setup_creates_the_first_workspace() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, None);
        let token = setup_token(&pool).await;
        let body = json!({
            "token": token, "email": "owner@example.org",
            "password": "not-a-real-password-0000",
            "workspace": { "key": "acme", "name": "Acme Tickets" },
        });
        let (response, me) = call(&app, Method::POST, "/console/api/setup", None, Some(body)).await;
        assert_eq!(response.status(), 201, "{me}");
        assert_eq!(me["workspaces"][0]["key"], "acme");
        assert_eq!(me["workspaces"][0]["role"], "owner");
        // The owner acts on `/v1` with the session alone.
        let id = cookie_session(&response);
        let ws = me["workspaces"][0]["id"].as_str().unwrap();
        let body = json!({ "kind": "secret", "name": "backend" });
        let (response, key) = console_call(
            &app,
            Method::POST,
            "/v1/keys",
            Some(&id),
            Some(ws),
            Some(body),
        )
        .await;
        assert_eq!(response.status(), 201, "{key}");
    })
    .await;
}

/// CON-003: a wrong password and an unknown email answer alike; sign-in sets
/// the cookie with its attributes, rotates a presented session, and
/// sign-out ends the session on the server.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn sign_in_is_uniform_rotates_and_signs_out() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let (_, old) = member(&pool, &a.id, Role::Editor, "ada@example.org").await;

        let mut refusals = Vec::new();
        for (email, password) in [
            ("ada@example.org", "not-the-password-at-all"),
            ("nobody@example.org", PASSWORD),
            ("not an email", PASSWORD),
            ("ada@example.org", &"x".repeat(200)),
        ] {
            let (response, mut problem) = call(
                &app,
                Method::POST,
                "/console/api/session",
                None,
                Some(sign_in_body(email, password)),
            )
            .await;
            assert_problem(&response, &problem, Code::Unauthorized);
            assert!(response.headers().get(header::SET_COOKIE).is_none());
            problem.as_object_mut().unwrap().remove("request_id");
            refusals.push(problem);
        }
        assert!(refusals.windows(2).all(|w| w[0] == w[1]), "{refusals:?}");
        assert_eq!(refusals[0]["details"]["reason"], "credentials");

        // Case and spaces in the email do not matter. The old session sent
        // along ends; a new one starts.
        let headers = console(&old, None);
        let (response, me) = call_with(
            &app,
            Method::POST,
            "/console/api/session",
            None,
            &refs(&headers),
            Some(sign_in_body(" ADA@example.org ", PASSWORD)),
        )
        .await;
        assert_eq!(response.status(), 200, "{me}");
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        let id = cookie_session(&response);
        assert_eq!(
            cookie,
            format!(
                "__Host-cavea_console={}; Path=/; Max-Age=604800; HttpOnly; SameSite=Lax; Secure",
                id.as_str()
            )
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(me["csrf_token"], id.csrf());
        assert_eq!(
            me["workspaces"][0]["permissions"],
            json!(["charts.read", "charts.write", "events.read"])
        );
        let (response, _) =
            console_call(&app, Method::GET, "/console/api/me", Some(&old), None, None).await;
        assert_eq!(response.status(), 401);

        let (response, _) = console_call(
            &app,
            Method::DELETE,
            "/console/api/session",
            Some(&id),
            None,
            None,
        )
        .await;
        assert_eq!(response.status(), 204);
        assert_eq!(
            response.headers()[header::SET_COOKIE],
            "__Host-cavea_console=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax; Secure"
        );
        let (response, problem) =
            console_call(&app, Method::GET, "/console/api/me", Some(&id), None, None).await;
        assert_problem(&response, &problem, Code::Unauthorized);
        assert_eq!(problem["details"]["reason"], "console_session");
    })
    .await;
}

/// Over plain HTTP in development (`CAVEA_CONSOLE_COOKIE_SECURE=false`): no
/// `__Host-` prefix and no `Secure`, the rest alike.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn an_insecure_cookie_only_when_configured() {
    let _serial = serial();
    with_db(async move |pool| {
        let mut state = state();
        state.db = pool.clone();
        let config = Arc::make_mut(&mut state.config);
        config.admin_key = Some(ADMIN.parse::<AdminKey>().unwrap());
        config.console_cookie_secure = false;
        state.seats = Seats::new(pool.clone(), &state.config, ServerClock::System, None);
        let app = finish(routes().into_router(), state);
        let a = Tenant::create(&app, "a").await;
        member(&pool, &a.id, Role::Viewer, "ada@example.org").await;
        let (response, _) = call(
            &app,
            Method::POST,
            "/console/api/session",
            None,
            Some(sign_in_body("ada@example.org", PASSWORD)),
        )
        .await;
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        let value = cookie.strip_prefix("cavea_console=").unwrap();
        let id = value.split(';').next().unwrap();
        assert_eq!(
            cookie,
            format!("cavea_console={id}; Path=/; Max-Age=604800; HttpOnly; SameSite=Lax")
        );
    })
    .await;
}

/// A session ends 12 hours after its last use and 7 days after sign-in
/// however used; use within the idle window renews it, at most every five
/// minutes.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn sessions_slide_and_end() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let (_, id) = member(&pool, &a.id, Role::Viewer, "ada@example.org").await;
        let set = async |sql: &'static str| {
            sqlx::query(sql)
                .bind(id.hash().as_slice())
                .execute(&pool)
                .await
                .unwrap();
        };
        let left = async || -> f64 {
            sqlx::query_scalar(
                "SELECT extract(epoch FROM expires_at - now())::float8 FROM console_sessions WHERE id_hash = $1",
            )
            .bind(id.hash().as_slice())
            .fetch_one(&pool)
            .await
            .unwrap()
        };
        let me = async || {
            console_call(&app, Method::GET, "/console/api/me", Some(&id), None, None)
                .await
                .0
                .status()
        };
        // Used an hour before its end: renewed to 12 hours.
        set("UPDATE console_sessions SET expires_at = now() + interval '1 hour' WHERE id_hash = $1")
            .await;
        assert_eq!(me().await, 200);
        assert!(left().await > 11.9 * 3600.0);
        // Within the first five minutes: not written again.
        set("UPDATE console_sessions SET expires_at = now() + interval '11 hours 58 minutes' WHERE id_hash = $1")
            .await;
        assert_eq!(me().await, 200);
        assert!(left().await < 11.97 * 3600.0);
        // Never past seven days from sign-in.
        set("UPDATE console_sessions SET created_at = now() - interval '6 days 23 hours', expires_at = now() + interval '1 minute' WHERE id_hash = $1")
            .await;
        assert_eq!(me().await, 200);
        let rest = left().await;
        assert!(rest > 3500.0 && rest < 3600.0, "{rest}");
        set("UPDATE console_sessions SET created_at = now() - interval '7 days', expires_at = now() + interval '1 hour' WHERE id_hash = $1")
            .await;
        assert_eq!(me().await, 401);
        // Idle past its end.
        let id2 = session_of(&pool, &a_user(&pool).await).await;
        sqlx::query("UPDATE console_sessions SET expires_at = now() - interval '1 second' WHERE id_hash = $1")
            .bind(id2.hash().as_slice())
            .execute(&pool)
            .await
            .unwrap();
        let (response, _) =
            console_call(&app, Method::GET, "/console/api/me", Some(&id2), None, None).await;
        assert_eq!(response.status(), 401);
    })
    .await;
}

async fn a_user(pool: &PgPool) -> String {
    sqlx::query_scalar("SELECT replace(id::text, '-', '') FROM users LIMIT 1")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// SEC-003: a password change ends every session of the user and starts a
/// new one; a wrong current password changes nothing.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn a_password_change_ends_every_session() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let (user, here) = member(&pool, &a.id, Role::Admin, "ada@example.org").await;
        let elsewhere = session_of(&pool, &user).await;
        let change = |current: &str| {
            json!({ "current_password": current, "new_password": "a-new-fake-password-1111" })
        };
        let (response, problem) = console_call(
            &app,
            Method::POST,
            "/console/api/password",
            Some(&here),
            None,
            Some(change("not-the-password-at-all")),
        )
        .await;
        assert_problem(&response, &problem, Code::Forbidden);
        assert_eq!(problem["details"]["reason"], "credentials");
        let (response, _) = console_call(
            &app,
            Method::POST,
            "/console/api/password",
            Some(&here),
            None,
            Some(change(PASSWORD)),
        )
        .await;
        assert_eq!(response.status(), 200);
        let new = cookie_session(&response);
        for (id, status) in [(&here, 401), (&elsewhere, 401), (&new, 200)] {
            let (response, _) =
                console_call(&app, Method::GET, "/console/api/me", Some(id), None, None).await;
            assert_eq!(response.status(), status);
        }
        let (response, _) = call(
            &app,
            Method::POST,
            "/console/api/session",
            None,
            Some(sign_in_body("ada@example.org", "a-new-fake-password-1111")),
        )
        .await;
        assert_eq!(response.status(), 200);
    })
    .await;
}

/// R1-2: a sign-in's check that the password it verified is still the
/// user's holds the user's row until the session is in, so a password change
/// waits for it (and then ends that session); once a change is committed, the
/// old hash no longer passes.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn a_sign_in_and_a_password_change_serialise() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let (user, _) = member(&pool, &a.id, Role::Viewer, "ada@example.org").await;
        let mut sign_in = pool.begin().await.unwrap();
        let current =
            crate::http::console::still_current(&mut sign_in, &user, Some(password_hash()));
        assert!(current.await.unwrap());
        let change = sqlx::query("UPDATE users SET password_hash = 'changed' WHERE id = $1::uuid")
            .bind(&user)
            .execute(&pool);
        let mut change = std::pin::pin!(change);
        let waited = tokio::time::timeout(Duration::from_millis(300), &mut change).await;
        assert!(waited.is_err(), "the change did not wait for the sign-in");
        sign_in.commit().await.unwrap();
        change.await.unwrap();
        let mut late = pool.begin().await.unwrap();
        let current = crate::http::console::still_current(&mut late, &user, Some(password_hash()));
        assert!(!current.await.unwrap());
    })
    .await;
}

/// SEC-003: failures from one address make its next attempts wait, doubling,
/// and a refused attempt checks no password; the same for an email nobody
/// has; another address, the member's own, is untouched; the per-address
/// budget refuses the eleventh attempt in a minute; a wait ends by itself.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn sign_in_backs_off_without_locking_out() {
    let _serial = serial();
    with_db(async move |pool| {
        let clock = ManualClock::new(Timestamp(1_800_000_000_000));
        let mut state = state();
        state.db = pool.clone();
        let config = Arc::make_mut(&mut state.config);
        config.admin_key = Some(ADMIN.parse::<AdminKey>().unwrap());
        state.seats = Seats::new(
            pool.clone(),
            &state.config,
            ServerClock::Manual(clock.clone()),
            None,
        );
        state.limiter = Arc::new(Limiter::new(120, 3_000).with_sign_in(10));
        let guard = state.sign_in.clone();
        let app = finish(routes().into_router(), state);
        let a = Tenant::create(&app, "a").await;
        member(&pool, &a.id, Role::Viewer, "ada@example.org").await;
        let attempt = async |peer: &str, email: &str, password: &str| {
            let body = sign_in_body(email, password).to_string();
            let mut request = request(Method::POST, "/console/api/session", None, &[], Some(body));
            let peer = SocketAddr::new(peer.parse().unwrap(), 40_000);
            request.extensions_mut().insert(ConnectInfo(peer));
            send(app.clone(), request).await
        };
        let attacker = "203.0.113.9";
        for (attacker, email) in [
            (attacker, "ada@example.org"),
            ("203.0.113.10", "nobody@example.org"),
        ] {
            for _ in 0..PAIR_FREE {
                let (response, _) = attempt(attacker, email, "not-the-password-at-all").await;
                assert_eq!(response.status(), 401);
            }
            // Even the right password waits now: nothing is checked.
            let (response, problem) = attempt(attacker, email, PASSWORD).await;
            assert_problem(&response, &problem, Code::RateLimited);
            assert_eq!(problem["details"]["reason"], "backoff");
            assert_eq!(response.headers()[header::RETRY_AFTER], "1");
        }
        // The member signs in from their own address meanwhile.
        let (response, _) = attempt("198.51.100.7", "ada@example.org", PASSWORD).await;
        assert_eq!(response.status(), 200);
        // A second later the attacker's address may try once more, then waits
        // twice as long.
        clock.advance_ms(1_000);
        let (response, _) = attempt(attacker, "ada@example.org", "still-not-the-password").await;
        assert_eq!(response.status(), 401);
        let (response, _) = attempt(attacker, "ada@example.org", PASSWORD).await;
        assert_eq!(response.headers()[header::RETRY_AFTER], "2");
        clock.advance_ms(2_000);
        let (response, _) = attempt(attacker, "ada@example.org", PASSWORD).await;
        assert_eq!(response.status(), 200);
        use crate::console::guard::Outcome;
        assert_eq!(guard.counted(Outcome::Backoff), 3);
        assert_eq!(guard.counted(Outcome::Ok), 2);

        // Ten attempts a minute from one address, whatever the email.
        clock.advance_ms(60_000);
        let busy = "192.0.2.44";
        for i in 0..10 {
            let (response, _) = attempt(busy, &format!("u{i}@example.org"), "x").await;
            assert_eq!(response.status(), 401);
        }
        let (response, problem) = attempt(busy, "ada@example.org", PASSWORD).await;
        assert_problem(&response, &problem, Code::RateLimited);
        assert_eq!(problem["details"]["reason"], "sign_in");
    })
    .await;
}

/// CON-004 and the audit trail: a console user's seat operations are written
/// to the event log as `usr_…`, and drafts and versions record who.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn a_console_user_is_the_actor() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let (user, id) = member(&pool, &a.id, Role::Operator, "ada@example.org").await;
        let uri = format!("/v1/events/{}/change-status", a.event);
        let body = json!({ "objects": { "version": a.hash, "idx": [5] }, "status": "blocked" });
        let (response, reply) =
            console_call(&app, Method::POST, &uri, Some(&id), Some(&a.id), Some(body)).await;
        assert_eq!(response.status(), 200, "{reply}");
        let actor: String = sqlx::query_scalar(
            "SELECT actor FROM event_log WHERE seq = $1 ORDER BY at DESC LIMIT 1",
        )
        .bind(reply["seq"].as_i64().unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(actor, format!("usr_{user}"));

        // An editor saves and publishes: both record the user; a key, nobody.
        let (editor, id) = member(&pool, &a.id, Role::Editor, "bob@example.org").await;
        let uri = format!("/v1/charts/{}/draft", a.chart);
        let mut headers = console(&id, Some(&a.id));
        headers.push((header::IF_MATCH, "*".to_owned()));
        let document = tiny_document("by bob");
        let (response, _) = call_with(
            &app,
            Method::PUT,
            &uri,
            None,
            &refs(&headers),
            Some(serde_json::from_str(&document).unwrap()),
        )
        .await;
        assert_eq!(response.status(), 200);
        let uri = format!("/v1/charts/{}/publish", a.chart);
        let (response, _) =
            console_call(&app, Method::POST, &uri, Some(&id), Some(&a.id), None).await;
        assert_eq!(response.status(), 201);
        let who: (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT replace(c.draft_updated_by::text, '-', ''), replace(v.created_by::text, '-', '')
               FROM charts c JOIN chart_versions v ON v.id = c.published_version_id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(who, (Some(editor.clone()), Some(editor.clone())));
        // A chart created with its document records its author too.
        let body = json!({ "key": "by-bob", "name": "n", "document": serde_json::from_str::<Value>(&document).unwrap() });
        let (response, _) =
            console_call(&app, Method::POST, "/v1/charts", Some(&id), Some(&a.id), Some(body)).await;
        assert_eq!(response.status(), 201);
        let author: Option<String> = sqlx::query_scalar(
            "SELECT replace(draft_updated_by::text, '-', '') FROM charts WHERE key = 'by-bob'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(author, Some(editor));
    })
    .await;
}

/// What a console session costs a `/v1` request against a secret key: the
/// same read, 2,000 times each, through the router (no network). Run with
/// `--release -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "a measurement: needs CAVEA_TEST_DATABASE_URL and --release"]
async fn measure_session_lookup() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let (_, id) = member(&pool, &a.id, Role::Viewer, "ada@example.org").await;
        let uri = format!("/v1/events/{}", a.event);
        let key = [(header::AUTHORIZATION, format!("Bearer {}", a.secret))];
        let cookie = console(&id, Some(&a.id));
        for (label, headers) in [("secret key", &key[..]), ("console session", &cookie[..])] {
            let mut times = Vec::new();
            for i in 0..2_200 {
                let request = request(Method::GET, &uri, None, &refs(headers), None);
                let started = std::time::Instant::now();
                let response = app.clone().oneshot(request).await.unwrap();
                let elapsed = started.elapsed();
                assert_eq!(response.status(), 200);
                if i >= 200 {
                    times.push(elapsed.as_secs_f64() * 1e3);
                }
            }
            times.sort_by(f64::total_cmp);
            let at = |q: f64| times[((times.len() - 1) as f64 * q) as usize];
            println!("{label}: p50 {:.3} ms, p99 {:.3} ms", at(0.5), at(0.99));
        }
    })
    .await;
}

/// The hold path with a secret key: 1,800 holds of one seat each on the
/// tenant's token, through the router (no network); the first 200 warm up.
/// Run with `--release -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "a measurement: needs CAVEA_TEST_DATABASE_URL and --release"]
async fn measure_hold_with_a_key() {
    let _serial = serial();
    with_db(async move |pool| {
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let uri = format!("/v1/events/{}/hold", a.event);
        let key = [(header::AUTHORIZATION, format!("Bearer {}", a.secret))];
        let mut times = Vec::new();
        for i in 0..1_800 {
            let body =
                json!({ "hold_token": a.token, "objects": { "version": a.hash, "idx": [i] } });
            let request = request(
                Method::POST,
                &uri,
                None,
                &refs(&key),
                Some(body.to_string()),
            );
            let started = std::time::Instant::now();
            let response = app.clone().oneshot(request).await.unwrap();
            let elapsed = started.elapsed();
            assert_eq!(response.status(), 200);
            if i >= 200 {
                times.push(elapsed.as_secs_f64() * 1e3);
            }
        }
        times.sort_by(f64::total_cmp);
        let at = |q: f64| times[((times.len() - 1) as f64 * q) as usize];
        println!(
            "hold with a secret key: p50 {:.3} ms, p99 {:.3} ms",
            at(0.5),
            at(0.99)
        );
    })
    .await;
}
