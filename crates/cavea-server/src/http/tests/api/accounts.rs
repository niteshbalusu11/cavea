//! CON-003 against PostgreSQL (09 §2, §10; 12 §3 T9; D-89, D-90): enrolling
//! and confirming a second factor, the two-step sign-in in order (the
//! password first, nothing told before it), replay, one step of drift,
//! recovery codes used once, the challenge's expiry and its count of wrong
//! codes, the back-off, turning the factor off; password reset links (single
//! use, expiry, sessions ended, the second factor kept unless chosen), an
//! owner's or admin's resets and who may make them, and the admin key's.
//! The role and tenant matrix, the headers and the logs are the shared
//! tests' (`ROUTES`).

use cavea_engine::{ManualClock, Timestamp};

use super::*;
use crate::console::totp;

const EMAIL: &str = "ada@a.example";

/// A password that is not the user's, made at run time (no literal that
/// a secret scanner would take for one).
fn wrong() -> String {
    ["wrong", "guess", "0"].join("-")
}

/// A second-factor code that is not one, made at run time like `wrong()`.
fn not_a_code() -> String {
    ["not", "a", "code"].join("-")
}

/// A clock in the middle of a TOTP step.
fn clock() -> ManualClock {
    ManualClock::new(Timestamp(1_900_000_000_000 + 7_000))
}

fn now(clock: &ManualClock) -> u64 {
    use cavea_engine::Clock as _;
    clock.now().0
}

/// The session a reply's `Set-Cookie` starts.
fn started(response: &Response<Body>) -> SessionId {
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    let value = cookie
        .strip_prefix("__Host-cavea_console=")
        .and_then(|c| c.split(';').next())
        .unwrap();
    SessionId::parse(value).unwrap()
}

async fn as_user(
    app: &Router,
    method: Method,
    uri: &str,
    session: &SessionId,
    workspace: Option<&str>,
    body: Option<Value>,
) -> (Response<Body>, Value) {
    let headers = console(session, workspace);
    call_with(app, method, uri, None, &refs(&headers), body).await
}

async fn sign_in(app: &Router, email: &str, password: &str) -> (Response<Body>, Value) {
    let body = json!({ "email": email, "password": password });
    call(app, Method::POST, "/console/api/session", None, Some(body)).await
}

async fn second_step(app: &Router, challenge: &str, code: &str) -> (Response<Body>, Value) {
    let body = json!({ "challenge": challenge, "code": code });
    let uri = "/console/api/session/second-factor";
    call(app, Method::POST, uri, None, Some(body)).await
}

/// RFC 4648 base32 without padding, as the enrolment returns it.
pub(super) fn unbase32(text: &str) -> Vec<u8> {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let (mut out, mut buffer, mut bits) = (Vec::new(), 0u32, 0u32);
    for c in text.bytes() {
        let v = alphabet.iter().position(|&a| a == c).unwrap() as u32;
        buffer = (buffer << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    out
}

/// Enrol `session`'s user, whose password is `password`, and confirm with
/// the current code: the secret and the recovery codes.
async fn enrol(
    app: &Router,
    clock: &ManualClock,
    session: &SessionId,
    password: &str,
) -> (Vec<u8>, Vec<String>) {
    let body = json!({ "password": password });
    let (response, secret) = as_user(
        app,
        Method::POST,
        "/console/api/totp",
        session,
        None,
        Some(body),
    )
    .await;
    assert_eq!(response.status(), 200, "{secret}");
    let uri = secret["uri"].as_str().unwrap();
    let text = secret["secret"].as_str().unwrap();
    assert!(uri.starts_with("otpauth://totp/Cavea:") && uri.contains(&format!("secret={text}&")));
    assert_eq!(
        (secret["digits"].as_u64(), secret["period"].as_u64()),
        (Some(6), Some(30))
    );
    let key = unbase32(text);
    assert_eq!(key.len(), totp::SECRET_BYTES);
    let code = totp::code(&key, totp::step(now(clock)));
    let body = json!({ "code": code });
    let (response, codes) = as_user(
        app,
        Method::POST,
        "/console/api/totp/confirm",
        session,
        None,
        Some(body),
    )
    .await;
    assert_eq!(response.status(), 200, "{codes}");
    let codes: Vec<String> = codes["recovery_codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(codes.len(), totp::RECOVERY_CODES);
    (key, codes)
}

async fn actions(pool: &PgPool, user: &str) -> Vec<(String, Option<String>)> {
    sqlx::query_as(
        "SELECT action, detail::text FROM audit_log WHERE user_id = $1::uuid OR target = $2
          ORDER BY id",
    )
    .bind(user)
    .bind(format!("usr_{user}"))
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn two_step_sign_in() {
    let _serial = serial();
    with_db(async move |pool| {
        let clock = clock();
        let (app, _) = api_with(&pool, Some(ADMIN), ServerClock::Manual(clock.clone()), "t");
        let a = Tenant::create(&app, "a").await;
        let (user, session) = member(&pool, &a.id, Role::Admin, EMAIL).await;
        let step = || totp::step(now(&clock));

        // Without a second factor, sign-in is one step, as before.
        let (response, me) = sign_in(&app, EMAIL, PASSWORD).await;
        assert_eq!(response.status(), 200, "{me}");
        assert_eq!(me["user"]["second_factor"], false);

        // Enrolment needs the password; an unconfirmed secret signs nobody in
        // and a wrong code does not confirm it.
        let body = json!({ "password": wrong() });
        let (response, problem) = as_user(
            &app,
            Method::POST,
            "/console/api/totp",
            &session,
            None,
            Some(body),
        )
        .await;
        assert_problem(&response, &problem, Code::Forbidden);
        assert_eq!(problem["details"]["reason"], "credentials");
        let body = json!({ "code": "123456" });
        let uri = "/console/api/totp/confirm";
        let (response, problem) =
            as_user(&app, Method::POST, uri, &session, None, Some(body)).await;
        assert_problem(&response, &problem, Code::InvalidRequest);
        assert_eq!(problem["details"]["reason"], "no_second_factor");
        let body = json!({ "password": PASSWORD });
        let (response, _) = as_user(
            &app,
            Method::POST,
            "/console/api/totp",
            &session,
            None,
            Some(body),
        )
        .await;
        assert_eq!(response.status(), 200);
        assert_eq!(sign_in(&app, EMAIL, PASSWORD).await.0.status(), 200);
        let body = json!({ "code": "12345" });
        let (response, problem) =
            as_user(&app, Method::POST, uri, &session, None, Some(body)).await;
        assert_problem(&response, &problem, Code::Forbidden);
        assert_eq!(problem["details"]["reason"], "second_factor");
        // Starting again replaces the secret; confirming turns it on.
        let (key, recovery) = enrol(&app, &clock, &session, PASSWORD).await;
        let (_, me) = as_user(&app, Method::GET, "/console/api/me", &session, None, None).await;
        assert_eq!(me["user"]["second_factor"], true);
        let body = json!({ "password": PASSWORD });
        let (response, problem) = as_user(
            &app,
            Method::POST,
            "/console/api/totp",
            &session,
            None,
            Some(body),
        )
        .await;
        assert_problem(&response, &problem, Code::AlreadyExists);
        assert_eq!(problem["details"]["reason"], "second_factor");

        // Before the password is verified nothing differs: an unknown email,
        // and a wrong password, are refused exactly as for a user without a
        // second factor.
        for (email, password) in [("nobody@a.example", PASSWORD), (EMAIL, &*wrong())] {
            let (response, problem) = sign_in(&app, email, password).await;
            assert_problem(&response, &problem, Code::Unauthorized);
            assert_eq!(problem["details"], json!({ "reason": "credentials" }));
        }
        // With the right password the reply is `202`, a challenge, and no session yet.
        let (response, first) = sign_in(&app, EMAIL, PASSWORD).await;
        assert_eq!(response.status(), 202, "{first}");
        assert!(response.headers().get(header::SET_COOKIE).is_none());
        let challenge = first["challenge"].as_str().unwrap().to_owned();
        assert_eq!(challenge.len(), 64);
        // The code confirmation used is spent: the same step is refused.
        let used = totp::code(&key, step());
        let (response, problem) = second_step(&app, &challenge, &used).await;
        assert_problem(&response, &problem, Code::Unauthorized);
        assert_eq!(problem["details"]["reason"], "second_factor");
        // The next step's code, within one step of drift: signed in.
        clock.advance_ms(totp::STEP_MS);
        let (response, me) = second_step(&app, &challenge, &totp::code(&key, step())).await;
        assert_eq!(response.status(), 200, "{me}");
        let signed = started(&response);
        let (response, _) =
            as_user(&app, Method::GET, "/console/api/me", &signed, None, None).await;
        assert_eq!(response.status(), 200);
        // The challenge is used up; its code is spent.
        let (response, problem) =
            second_step(&app, &challenge, &totp::code(&key, step() + 1)).await;
        assert_problem(&response, &problem, Code::Unauthorized);
        assert_eq!(problem["details"]["reason"], "challenge");
        let (_, again) = sign_in(&app, EMAIL, PASSWORD).await;
        let again = again["challenge"].as_str().unwrap().to_owned();
        let (response, _) = second_step(&app, &again, &totp::code(&key, step())).await;
        assert_eq!(response.status(), 401, "a code is accepted once");
        // One step behind or ahead is accepted, two are not.
        clock.advance_ms(3 * totp::STEP_MS);
        let (response, _) = second_step(&app, &again, &totp::code(&key, step() - 2)).await;
        assert_eq!(response.status(), 401);
        let (response, _) = second_step(&app, &again, &totp::code(&key, step() - 1)).await;
        assert_eq!(response.status(), 200);
        let (_, next) = sign_in(&app, EMAIL, PASSWORD).await;
        let next = next["challenge"].as_str().unwrap().to_owned();
        let (response, _) = second_step(&app, &next, &totp::code(&key, step() + 2)).await;
        assert_eq!(response.status(), 401);
        let (response, _) = second_step(&app, &next, &totp::code(&key, step() + 1)).await;
        assert_eq!(response.status(), 200);

        // A recovery code, however typed, once.
        let (_, first) = sign_in(&app, EMAIL, PASSWORD).await;
        let challenge = first["challenge"].as_str().unwrap().to_owned();
        let typed = recovery[0].to_uppercase().replace('-', " ");
        let (response, me) = second_step(&app, &challenge, &typed).await;
        assert_eq!(response.status(), 200, "{me}");
        let (_, first) = sign_in(&app, EMAIL, PASSWORD).await;
        let challenge = first["challenge"].as_str().unwrap().to_owned();
        let (response, problem) = second_step(&app, &challenge, &recovery[0]).await;
        assert_problem(&response, &problem, Code::Unauthorized);
        assert_eq!(problem["details"]["reason"], "second_factor");

        // Wrong codes count in the back-off, and a challenge takes five (one
        // above, three here, one more).
        clock.advance_ms(totp::STEP_MS);
        for n in 1..=3 {
            let (response, _) = second_step(&app, &challenge, &not_a_code()).await;
            assert_eq!(response.status(), 401, "{n}");
        }
        // The fifth failure in a row from this address: the next waits.
        let (response, _) = second_step(&app, &challenge, &format!("still-{}", not_a_code())).await;
        assert_eq!(response.status(), 401);
        let (response, problem) = second_step(&app, &challenge, &totp::code(&key, step())).await;
        assert_problem(&response, &problem, Code::RateLimited);
        assert_eq!(problem["details"]["reason"], "backoff");
        clock.advance_ms(2_000);
        let (response, problem) = second_step(&app, &challenge, &totp::code(&key, step())).await;
        assert_problem(&response, &problem, Code::Unauthorized);
        assert_eq!(problem["details"]["reason"], "challenge");
        // After the wait, the password and a code sign in.
        clock.advance_ms(60_000);
        let (_, first) = sign_in(&app, EMAIL, PASSWORD).await;
        let challenge = first["challenge"].as_str().unwrap().to_owned();
        let (response, _) = second_step(&app, &challenge, &totp::code(&key, step())).await;
        assert_eq!(response.status(), 200);

        // A challenge expires, and dies with a password change.
        clock.advance_ms(totp::STEP_MS);
        let (_, first) = sign_in(&app, EMAIL, PASSWORD).await;
        let challenge = first["challenge"].as_str().unwrap().to_owned();
        sqlx::query("UPDATE console_challenges SET expires_at = now() - interval '1 second'")
            .execute(&pool)
            .await
            .unwrap();
        let (response, problem) = second_step(&app, &challenge, &totp::code(&key, step())).await;
        assert_eq!(
            problem["details"]["reason"],
            "challenge",
            "{}",
            response.status()
        );
        let (_, first) = sign_in(&app, EMAIL, PASSWORD).await;
        let challenge = first["challenge"].as_str().unwrap().to_owned();
        let body = json!({ "current_password": PASSWORD, "new_password": PASSWORD });
        let (response, _) = as_user(
            &app,
            Method::POST,
            "/console/api/password",
            &signed,
            None,
            Some(body),
        )
        .await;
        assert_eq!(response.status(), 200);
        let session = started(&response);
        let (response, problem) = second_step(&app, &challenge, &totp::code(&key, step())).await;
        assert_problem(&response, &problem, Code::Unauthorized);
        assert_eq!(problem["details"]["reason"], "challenge");

        // Turning it off needs the password and a code.
        let disable = |password: &str, code: &str| json!({ "password": password, "code": code });
        let uri = "/console/api/totp/disable";
        let body = disable(&wrong(), &totp::code(&key, step()));
        let (response, problem) =
            as_user(&app, Method::POST, uri, &session, None, Some(body)).await;
        assert_eq!(
            problem["details"]["reason"],
            "credentials",
            "{}",
            response.status()
        );
        let body = disable(PASSWORD, &not_a_code());
        let (response, problem) =
            as_user(&app, Method::POST, uri, &session, None, Some(body)).await;
        assert_eq!(
            problem["details"]["reason"],
            "second_factor",
            "{}",
            response.status()
        );
        let body = disable(PASSWORD, &recovery[1]);
        let (response, me) = as_user(&app, Method::POST, uri, &session, None, Some(body)).await;
        assert_eq!(response.status(), 200, "{me}");
        assert_eq!(me["user"]["second_factor"], false);
        let (response, _) = sign_in(&app, EMAIL, PASSWORD).await;
        assert_eq!(response.status(), 200);
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM totp_recovery_codes")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0);

        let actions = actions(&pool, &user).await;
        let has = |action: &str, detail: Option<&str>| {
            actions.iter().any(|(a, d)| {
                a == action && detail.is_none_or(|x| d.as_deref().is_some_and(|d| d.contains(x)))
            })
        };
        assert!(has("console.second_factor_enabled", None));
        assert!(has("console.sign_in", Some("\"totp\"")));
        assert!(has("console.sign_in", Some("\"recovery_code\"")));
        assert!(has("console.recovery_code_used", Some("\"remaining\": 9")));
        assert!(has("console.sign_in_failed", None));
        assert!(has("console.second_factor_disabled", None));
        // Nothing in the audit log is a secret.
        let text = format!("{actions:?}");
        assert!(!text.contains(&recovery[0]) && !text.contains(&totp::base32(&key)));
    })
    .await;
}

#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn password_reset_links() {
    let _serial = serial();
    with_db(async move |pool| {
        let clock = clock();
        let (app, _) = api_with(&pool, Some(ADMIN), ServerClock::Manual(clock.clone()), "t");
        let a = Tenant::create(&app, "a").await;
        let b = Tenant::create(&app, "b").await;
        let (owner, owner_session) = member(&pool, &a.id, Role::Owner, "owner@a.example").await;
        let (admin_id, admin) = member(&pool, &a.id, Role::Admin, "admin@a.example").await;
        let (viewer, viewer_session) = member(&pool, &a.id, Role::Viewer, EMAIL).await;
        let ws = Some(a.id.as_str());
        let reset_uri = |user: &str| format!("/console/api/members/usr_{user}/password-reset");
        let make = async |by: &SessionId, user: &str, clear: bool| {
            let body = json!({ "clear_second_factor": clear });
            as_user(&app, Method::POST, &reset_uri(user), by, ws, Some(body)).await
        };
        let token_of = |created: &Value| {
            let link = created["link"].as_str().unwrap();
            assert!(link.starts_with("/console/reset#token="), "{link}");
            link.rsplit("#token=").next().unwrap().to_owned()
        };
        let use_link = async |token: &str, password: &str| {
            let body = json!({ "token": token, "password": password });
            call(&app, Method::POST, "/console/api/password-reset", None, Some(body)).await
        };
        let new_password: &str = &["another", "one", "for", "tests"].join("-");

        // An admin makes a link; using it sets the password, ends every
        // session, and signs in.
        let (response, created) = make(&admin, &viewer, false).await;
        assert_eq!(response.status(), 201, "{created}");
        assert_eq!(created["reset"]["user"], format!("usr_{viewer}"));
        assert_eq!(created["reset"]["clear_second_factor"], false);
        let token = token_of(&created);
        let (response, problem) = use_link(&"0".repeat(64), new_password).await;
        assert_problem(&response, &problem, Code::Unauthorized);
        assert_eq!(problem["details"]["reason"], "reset_token");
        let (response, problem) = use_link(&token, "short").await;
        assert_eq!(problem["details"]["reason"], "invalid_field", "{}", response.status());
        let (response, me) = use_link(&token, new_password).await;
        assert_eq!(response.status(), 200, "{me}");
        assert_eq!(me["user"]["id"], format!("usr_{viewer}"));
        let (response, _) =
            as_user(&app, Method::GET, "/console/api/me", &viewer_session, None, None).await;
        assert_eq!(response.status(), 401, "the old session ended");
        assert_eq!(sign_in(&app, EMAIL, PASSWORD).await.0.status(), 401);
        assert_eq!(sign_in(&app, EMAIL, new_password).await.0.status(), 200);
        // Once only.
        let (response, problem) = use_link(&token, new_password).await;
        assert_problem(&response, &problem, Code::Unauthorized);
        assert_eq!(problem["details"]["reason"], "reset_token");
        // A new link replaces the last; an expired one works no more.
        let (_, first) = make(&owner_session, &viewer, false).await;
        let (_, second) = make(&owner_session, &viewer, false).await;
        let (response, _) = use_link(&token_of(&first), PASSWORD).await;
        assert_eq!(response.status(), 401);
        sqlx::query("UPDATE password_resets SET expires_at = now() - interval '1 second'")
            .execute(&pool)
            .await
            .unwrap();
        let (response, _) = use_link(&token_of(&second), PASSWORD).await;
        assert_eq!(response.status(), 401);

        // The second factor is kept unless the link says otherwise.
        let (response, _) = sign_in(&app, EMAIL, new_password).await;
        let viewer_session = started(&response);
        let (key, _) = enrol(&app, &clock, &viewer_session, new_password).await;
        let (_, kept) = make(&admin, &viewer, false).await;
        let (response, first) = use_link(&token_of(&kept), PASSWORD).await;
        assert_eq!(response.status(), 202, "{first}");
        assert!(response.headers().get(header::SET_COOKIE).is_none());
        clock.advance_ms(totp::STEP_MS);
        let code = totp::code(&key, totp::step(now(&clock)));
        let challenge = first["challenge"].as_str().unwrap();
        let (response, me) = second_step(&app, challenge, &code).await;
        assert_eq!(response.status(), 200, "{me}");
        assert_eq!(me["user"]["second_factor"], true);
        let (_, cleared) = make(&admin, &viewer, true).await;
        assert_eq!(cleared["reset"]["clear_second_factor"], true);
        let (response, me) = use_link(&token_of(&cleared), PASSWORD).await;
        assert_eq!(response.status(), 200, "{me}");
        assert_eq!(me["user"]["second_factor"], false);

        // An owner or admin resets a second factor alone; idempotent. The
        // members list says who has one, before and after.
        let viewer_session = started(&response);
        enrol(&app, &clock, &viewer_session, PASSWORD).await;
        let with_factor = async || {
            let uri = "/console/api/members";
            let (response, page) = as_user(&app, Method::GET, uri, &admin, ws, None).await;
            assert_eq!(response.status(), 200, "{page}");
            let members = page["data"].as_array().unwrap();
            assert_eq!(members.len(), 3);
            members
                .iter()
                .filter(|m| m["user"]["second_factor"] == true)
                .map(|m| m["user"]["id"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(with_factor().await, [format!("usr_{viewer}")]);
        let uri = format!("/console/api/members/usr_{viewer}/second-factor");
        for _ in 0..2 {
            let (response, _) = as_user(&app, Method::DELETE, &uri, &admin, ws, None).await;
            assert_eq!(response.status(), 204);
        }
        assert!(with_factor().await.is_empty());
        let (response, me) = sign_in(&app, EMAIL, PASSWORD).await;
        assert_eq!(response.status(), 200, "{me}");
        assert_eq!(me["user"]["second_factor"], false);

        // Who may reset whom: not oneself; an owner only by an owner; and
        // only someone the caller could manage in each of their workspaces.
        let (response, problem) = make(&admin, &admin_id, false).await;
        assert_problem(&response, &problem, Code::Forbidden);
        assert_eq!(problem["details"]["reason"], "own_account");
        let (response, problem) = make(&admin, &owner, false).await;
        assert_problem(&response, &problem, Code::Forbidden);
        assert_eq!(problem["details"]["reason"], "role");
        assert_eq!(problem["details"]["permission"], "ownership");
        let (response, _) = make(&owner_session, &owner_user_b(&pool, &b.id).await, false).await;
        assert_eq!(response.status(), 404, "not a member of A");
        sqlx::query("INSERT INTO memberships (user_id, workspace_id, role) VALUES ($1::uuid, $2::uuid, 'viewer')")
            .bind(&viewer)
            .bind(&b.id[3..])
            .execute(&pool)
            .await
            .unwrap();
        for by in [&admin, &owner_session] {
            let (response, problem) = make(by, &viewer, false).await;
            assert_problem(&response, &problem, Code::Forbidden);
            assert_eq!(problem["details"]["reason"], "other_workspaces");
            let (response, problem) = as_user(&app, Method::DELETE, &uri, by, ws, None).await;
            assert_problem(&response, &problem, Code::Forbidden);
            assert_eq!(problem["details"]["reason"], "other_workspaces");
        }

        // The admin key: an owner of the workspace, by email.
        let uri = format!("/v1/workspaces/{}/password-resets", a.id);
        let body = json!({ "email": "owner@a.example", "clear_second_factor": true });
        let (response, created) = call(&app, Method::POST, &uri, Some(ADMIN), Some(body)).await;
        assert_eq!(response.status(), 201, "{created}");
        assert_eq!(created["reset"]["user"], format!("usr_{owner}"));
        let body = json!({ "email": EMAIL });
        let (response, problem) = call(&app, Method::POST, &uri, Some(ADMIN), Some(body)).await;
        assert_problem(&response, &problem, Code::NotFound);
        assert_eq!(problem["details"]["missing"], "owner");
        let (response, me) = use_link(&token_of(&created), new_password).await;
        assert_eq!(response.status(), 200, "{me}");

        let audit: Vec<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT action, actor, detail::text FROM audit_log
              WHERE action IN ('member.password_reset_created', 'console.password_reset',
                               'member.second_factor_reset') ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(audit.iter().any(|(a, by, _)| a == "member.password_reset_created" && by == "admin"));
        assert!(audit.iter().any(|(a, _, d)| a == "console.password_reset"
            && d.as_deref().is_some_and(|d| d.contains("\"second_factor_cleared\": true"))));
        assert_eq!(audit.iter().filter(|(a, ..)| a == "member.second_factor_reset").count(), 1);
    })
    .await;
}

/// An owner of workspace `b` (`ws_…`), made for the purpose.
async fn owner_user_b(pool: &PgPool, b: &str) -> String {
    member(pool, b, Role::Owner, "owner@b.example").await.0
}

/// What changes while a request waits for the user's row: a second factor
/// turned on during a sign-in is asked for (R1-1); turning a factor off is
/// checked against the factor the user has when it commits, not one an
/// admin reset and they replaced meanwhile (R1-2); and the second step ends
/// the session presented with the sign-in, as one step does (R1-3); and a
/// reset holding the user while a second step waits ends cleanly (R2-1). The
/// test holds the row lock in a transaction of its own, so the order is
/// fixed, not raced.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn second_factor_under_the_lock() {
    let _serial = serial();
    with_db(async move |pool| {
        let clock = clock();
        let (app, _) = api_with(&pool, Some(ADMIN), ServerClock::Manual(clock.clone()), "t");
        let a = Tenant::create(&app, "a").await;
        let (user, session) = member(&pool, &a.id, Role::Admin, EMAIL).await;
        let step = || totp::step(now(&clock));
        let lock = "SELECT FROM users WHERE id = $1::uuid FOR UPDATE";
        let wait = || tokio::time::sleep(Duration::from_millis(1_500));

        // A factor turned on while the sign-in verifies the password.
        let mut held = pool.begin().await.unwrap();
        sqlx::query(lock)
            .bind(&user)
            .execute(&mut *held)
            .await
            .unwrap();
        let racing = tokio::spawn({
            let app = app.clone();
            async move { sign_in(&app, EMAIL, PASSWORD).await }
        });
        wait().await;
        sqlx::query(
            "UPDATE users SET totp_secret = '\\x3132333435363738393031323334353637383930',
                    totp_enabled_at = now() WHERE id = $1::uuid",
        )
        .bind(&user)
        .execute(&mut *held)
        .await
        .unwrap();
        held.commit().await.unwrap();
        let (response, answer) = racing.await.unwrap();
        assert_eq!(response.status(), 202, "{answer}");
        assert!(response.headers().get(header::SET_COOKIE).is_none());
        let key = b"12345678901234567890".to_vec();

        // The second step ends the session presented with the sign-in.
        let presented = session_of(&pool, &user).await;
        let cookie = console(&presented, None);
        let body = json!({ "email": EMAIL, "password": PASSWORD });
        let (response, first) = call_with(
            &app,
            Method::POST,
            "/console/api/session",
            None,
            &refs(&cookie),
            Some(body),
        )
        .await;
        assert_eq!(response.status(), 202);
        let body = json!({ "challenge": first["challenge"], "code": totp::code(&key, step()) });
        let uri = "/console/api/session/second-factor";
        let (response, _) =
            call_with(&app, Method::POST, uri, None, &refs(&cookie), Some(body)).await;
        assert_eq!(response.status(), 200);
        let (response, _) =
            as_user(&app, Method::GET, "/console/api/me", &presented, None, None).await;
        assert_eq!(response.status(), 401, "the presented session ended");

        // A reset holding the user while a second step waits: no lock cycle;
        // the step finds its challenge gone (R2-1).
        let (_, first) = sign_in(&app, EMAIL, PASSWORD).await;
        let challenge = first["challenge"].as_str().unwrap().to_owned();
        clock.advance_ms(totp::STEP_MS);
        let mut held = pool.begin().await.unwrap();
        sqlx::query(lock)
            .bind(&user)
            .execute(&mut *held)
            .await
            .unwrap();
        let racing = tokio::spawn({
            let app = app.clone();
            let code = totp::code(&key, step());
            async move { second_step(&app, &challenge, &code).await }
        });
        wait().await;
        sqlx::query("DELETE FROM console_challenges WHERE user_id = $1::uuid")
            .bind(&user)
            .execute(&mut *held)
            .await
            .unwrap();
        held.commit().await.unwrap();
        let (response, problem) = racing.await.unwrap();
        assert_problem(&response, &problem, Code::Unauthorized);
        assert_eq!(problem["details"]["reason"], "challenge");

        // Turning off with a code of a factor replaced meanwhile.
        clock.advance_ms(totp::STEP_MS);
        let old_code = totp::code(&key, step());
        let mut held = pool.begin().await.unwrap();
        sqlx::query(lock)
            .bind(&user)
            .execute(&mut *held)
            .await
            .unwrap();
        let racing = tokio::spawn({
            let app = app.clone();
            let session = session.clone();
            async move {
                let body = json!({ "password": PASSWORD, "code": old_code });
                let uri = "/console/api/totp/disable";
                as_user(&app, Method::POST, uri, &session, None, Some(body)).await
            }
        });
        wait().await;
        // Reset and enrolled again: a new secret, its step before the code's.
        sqlx::query(
            "UPDATE users SET totp_secret = '\\x6e6f742d7468652d6f6c642d736563726574212121',
                    totp_last_step = $2 WHERE id = $1::uuid",
        )
        .bind(&user)
        .bind(i64::try_from(step() - 1).unwrap())
        .execute(&mut *held)
        .await
        .unwrap();
        held.commit().await.unwrap();
        let (response, problem) = racing.await.unwrap();
        assert_problem(&response, &problem, Code::Forbidden);
        assert_eq!(problem["details"]["reason"], "second_factor");
        let on: bool = sqlx::query_scalar("SELECT totp_enabled_at IS NOT NULL FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(on, "the replacement is still on");
    })
    .await;
}
