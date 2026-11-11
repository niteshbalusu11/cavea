//! Session tokens on the public routes against PostgreSQL (08 §11): the key
//! lookup, `require_session`, the `max_objects` cap, the `sub` binding and the
//! per-`sub` token cap.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, KeyInit as _, Mac as _};

use super::*;

const SESSION: header::HeaderName = header::HeaderName::from_static("cavea-session");

/// A session token as a customer's backend signs it: HS256 with the secret
/// key itself as the secret, which is HMAC with its SHA-256 (RFC 2104 §2).
pub(super) fn signed(secret: &str, kid: &str, claims: &Value) -> String {
    let b64 = |v: &Value| URL_SAFE_NO_PAD.encode(v.to_string());
    let header = json!({ "alg": "HS256", "typ": "JWT", "kid": kid });
    let signing_input = format!("{}.{}", b64(&header), b64(claims));
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(signing_input.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("{signing_input}.{signature}")
}

fn in_ten_minutes() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 600
}

/// A key's id by its display prefix.
async fn key_id(pool: &PgPool, key: &str) -> String {
    let id: String = sqlx::query_scalar(
        "SELECT 'key_' || replace(id::text, '-', '') FROM api_keys WHERE prefix = $1",
    )
    .bind(&key[..11])
    .fetch_one(pool)
    .await
    .unwrap();
    id
}

/// An event that requires sessions: anonymous public calls are refused and
/// secret-key calls are not; a token caps the hold token's limit; a hold token
/// is bound to its `sub`; each `sub` has at most `max_tokens_per_sub` live
/// tokens, also when created at once; every way a token can fail; and an
/// event without the requirement still honours a session.
#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn session_tokens() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = logs.install();
    let logs_in = logs.clone();
    with_db(async move |pool| {
        let logs = logs_in;
        let (app, seats) = api_with(&pool, Some(ADMIN), ServerClock::System, "test");
        let a = Tenant::create(&app, "a").await;
        let b = Tenant::create(&app, "b").await;
        let (sk, pk) = (a.secret.as_str(), a.publishable.as_str());
        let uri = format!("/v1/events/{}", a.event);
        let body = json!({ "settings": { "require_session": true, "max_objects": 6 } });
        let (response, event) = call(&app, Method::PATCH, &uri, Some(sk), Some(body)).await;
        assert_eq!(response.status(), 200, "{event}");
        assert_eq!(event["settings"], json!({ "hold_ttl_seconds": 900, "max_objects": 6,
            "require_session": true, "max_tokens_per_sub": 2 }));

        let claims = |sub: &str| json!({ "sub": sub, "event": a.event, "exp": in_ten_minutes(), "max_objects": 3 });
        let session = |sub: &str| signed(sk, &a.secret_id, &claims(sub));
        let buyer = async |method: Method, uri: &str, session: Option<&str>, body: Option<Value>| {
            let headers: Vec<_> = session.map(|s| (SESSION, s)).into_iter().collect();
            call_with(&app, method, uri, Some(pk), &headers, body).await
        };
        let ev = |op: &str| format!("/v1/public/events/{}/{op}", a.event);
        let create = json!({ "event": a.event });
        let create_with = async |session: Option<&str>| {
            buyer(Method::POST, "/v1/public/hold-tokens", session, Some(create.clone())).await
        };

        // The renderer learns that it needs a session before the first tap.
        let (_, info) = buyer(Method::GET, &format!("/v1/public/events/{}", a.event), None, None).await;
        assert_eq!((&info["require_session"], &info["max_objects"]), (&json!(true), &json!(6)));

        // Anonymous: refused. With a session: the token's limit is the lower one.
        let (response, p) = create_with(None).await;
        assert_problem(&response, &p, Code::Unauthorized);
        assert_eq!((&p["details"]["reason"], &p["details"]["session"]), (&json!("session_token"), &json!("missing")));
        let one = session("buyer-1");
        let (response, created) = create_with(Some(&one)).await;
        assert_eq!(response.status(), 201, "{created}");
        assert_eq!(created["max_objects"], 3);
        let mine = created["hold_token"].as_str().unwrap().to_owned();
        let body = json!({ "event": a.event, "max_objects": 4 });
        let (response, p) = buyer(Method::POST, "/v1/public/hold-tokens", Some(&one), Some(body)).await;
        assert_problem(&response, &p, Code::InvalidRequest);
        assert_eq!(p["details"]["field"], "max_objects");
        // Only the SHA-256 of `sub` is stored, on the hold.
        let stored: Option<String> = sqlx::query_scalar("SELECT session_sub FROM holds WHERE token_hash = $1")
            .bind(sha256(&mine))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored.as_deref(), Some(super::seats::hex(&sha256("buyer-1")).as_str()));
        for table in ["holds", "event_log", "object_state"] {
            let sql = format!("SELECT count(*) FROM {table} t WHERE strpos(t::text, 'buyer-1') > 0");
            let n: i64 = sqlx::query_scalar(AssertSqlSafe(sql)).fetch_one(&pool).await.unwrap();
            assert_eq!(n, 0, "{table}");
        }

        // The hold token works only with its own `sub`.
        let hold = json!({ "hold_token": mine, "objects": { "version": a.hash, "idx": [5, 6] } });
        let (response, held) = buyer(Method::POST, &ev("hold"), Some(&one), Some(hold.clone())).await;
        assert_eq!(response.status(), 200, "{held}");
        let two = session("buyer-2");
        for op in ["hold", "release"] {
            let (response, p) = buyer(Method::POST, &ev(op), Some(&two), Some(hold.clone())).await;
            assert_problem(&response, &p, Code::Forbidden);
            assert_eq!(p["details"]["reason"], "session", "{op}");
            let (response, p) = buyer(Method::POST, &ev(op), None, Some(hold.clone())).await;
            assert_problem(&response, &p, Code::Unauthorized);
            assert_eq!(p["details"]["session"], "missing", "{op}");
        }
        // A token the backend created is not bound: any valid session may use it.
        let body = json!({ "hold_token": a.token, "objects": { "version": a.hash, "idx": [7] } });
        let (response, held) = buyer(Method::POST, &ev("hold"), Some(&two), Some(body)).await;
        assert_eq!(response.status(), 200, "{held}");
        // Secret-key routes are untouched by the requirement.
        let (response, created) = call(&app, Method::POST, "/v1/hold-tokens", Some(sk), Some(create.clone())).await;
        assert_eq!(response.status(), 201, "{created}");
        let body = json!({ "hold_token": created["hold_token"], "objects": { "version": a.hash, "idx": [8] } });
        let (response, held) = call(&app, Method::POST, &format!("{uri}/hold"), Some(sk), Some(body)).await;
        assert_eq!(response.status(), 200, "{held}");

        // Two live tokens per `sub` (the default); a third is refused.
        let (response, _) = create_with(Some(&one)).await;
        assert_eq!(response.status(), 201);
        let (response, p) = create_with(Some(&one)).await;
        assert_problem(&response, &p, Code::HoldLimitExceeded);
        assert_eq!((&p["details"]["reason"], &p["details"]["limit"]), (&json!("tokens_per_sub"), &json!(2)));
        // Created at once: exactly the cap is bound, the rest refused.
        let three = session("buyer-3");
        let tasks: Vec<_> = (0..6)
            .map(|_| {
                let (app, three, body) = (app.clone(), three.clone(), create.clone());
                let pk = pk.to_owned();
                tokio::spawn(async move {
                    let headers = [(SESSION, three.as_str())];
                    let (response, _) = call_with(&app, Method::POST, "/v1/public/hold-tokens", Some(&pk), &headers, Some(body)).await;
                    response.status().as_u16()
                })
            })
            .collect();
        let mut statuses = Vec::new();
        for task in tasks {
            statuses.push(task.await.unwrap());
        }
        assert_eq!(statuses.iter().filter(|&&s| s == 201).count(), 2, "{statuses:?}");
        assert!(statuses.iter().all(|&s| s == 201 || s == 422), "{statuses:?}");
        let bound: i64 = sqlx::query_scalar("SELECT count(*) FROM holds WHERE session_sub = $1")
            .bind(super::seats::hex(&sha256("buyer-3")))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(bound, 2);

        // Every failure is a 401; only a correctly signed token learns why.
        let revoked_id = key_id(&pool, &a.revoked).await;
        let past = in_ten_minutes() - 3_600;
        let cases = [
            ("another workspace's key", signed(&b.secret, &b.secret_id, &claims("x")), "invalid"),
            ("our kid, their key", signed(&b.secret, &a.secret_id, &claims("x")), "invalid"),
            ("their kid, our key", signed(sk, &b.secret_id, &claims("x")), "invalid"),
            ("a publishable key", signed(pk, &a.publishable_id, &claims("x")), "invalid"),
            ("a revoked key", signed(&a.revoked, &revoked_id, &claims("x")), "invalid"),
            ("another event", signed(sk, &a.secret_id, &json!({ "sub": "x", "event": b.event, "exp": in_ten_minutes() })), "event"),
            ("expired", signed(sk, &a.secret_id, &json!({ "sub": "x", "event": a.event, "exp": past })), "expired"),
            ("not a token", "not-a-token".to_owned(), "invalid"),
        ];
        for (case, token, session) in cases {
            let (response, p) = create_with(Some(&token)).await;
            assert_problem(&response, &p, Code::Unauthorized);
            assert_eq!((&p["details"]["reason"], &p["details"]["session"]), (&json!("session_token"), &json!(session)), "{case}");
            assert_eq!(response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*", "{case}");
        }

        // Without the requirement: anonymous holding works, a session sent is
        // still verified and still binds its token.
        let body = json!({ "key": "open", "chart": a.chart });
        let (response, _) = call(&app, Method::POST, "/v1/events", Some(sk), Some(body)).await;
        assert_eq!(response.status(), 201);
        let open = json!({ "event": "open" });
        let (response, anon) = buyer(Method::POST, "/v1/public/hold-tokens", None, Some(open.clone())).await;
        assert_eq!(response.status(), 201, "{anon}");
        let bad = signed(sk, &a.secret_id, &json!({ "sub": "x", "event": "open", "exp": past }));
        let (response, p) = buyer(Method::POST, "/v1/public/hold-tokens", Some(&bad), Some(open.clone())).await;
        assert_problem(&response, &p, Code::Unauthorized);
        let good = signed(sk, &a.secret_id, &json!({ "sub": "buyer-1", "event": "open", "exp": in_ten_minutes() }));
        let (response, created) = buyer(Method::POST, "/v1/public/hold-tokens", Some(&good), Some(open)).await;
        assert_eq!((response.status().as_u16(), &created["max_objects"]), (201, &json!(10)), "{created}");
        let body = json!({ "hold_token": created["hold_token"], "objects": { "version": a.hash, "idx": [1] } });
        let (response, p) = buyer(Method::POST, "/v1/public/events/open/hold", None, Some(body.clone())).await;
        assert_problem(&response, &p, Code::Forbidden);
        let (response, held) = buyer(Method::POST, "/v1/public/events/open/hold", Some(&good), Some(body)).await;
        assert_eq!(response.status(), 200, "{held}");
        let body = json!({ "hold_token": anon["hold_token"], "objects": { "version": a.hash, "idx": [2] } });
        let (response, held) = buyer(Method::POST, "/v1/public/events/open/hold", None, Some(body)).await;
        assert_eq!(response.status(), 200, "{held}");

        // Settings are validated like the others.
        for (settings, field) in [
            (json!({ "max_tokens_per_sub": 0 }), "settings.max_tokens_per_sub"),
            (json!({ "max_tokens_per_sub": 11 }), "settings.max_tokens_per_sub"),
        ] {
            let (response, p) = call(&app, Method::PATCH, &uri, Some(sk), Some(json!({ "settings": settings }))).await;
            assert_problem(&response, &p, Code::InvalidRequest);
            assert_eq!(p["details"]["field"], field);
        }
        let (response, p) = call(&app, Method::PATCH, &uri, Some(sk), Some(json!({ "settings": { "require_session": "yes" } }))).await;
        assert_problem(&response, &p, Code::InvalidRequest);

        seats.shutdown().await;
        let mut secrets: Vec<&str> = a.secrets().into_iter().chain(b.secrets()).collect();
        let hash = super::seats::hex(&sha256("buyer-1"));
        secrets.extend([one.as_str(), two.as_str(), three.as_str(), good.as_str(), &mine, "buyer-1", "buyer-2", "buyer-3", &hash]);
        assert_no_secrets(&logs, &secrets);
    })
    .await;
}
