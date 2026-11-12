//! SEC-009 (12 §4): no log record carries a secret. Every route of the table
//! is driven twice with everything captured at `trace`: once as it succeeds,
//! with recognisable values in the headers a client may add and in the order
//! reference, collecting every secret the routes hand out (keys, hold tokens,
//! CSRF tokens, invitation links); once with recognisable wrong secrets in
//! every place one can go (bearer, cookie, CSRF, session and hold-token
//! headers, the query string, identifier paths, passwords and tokens in
//! bodies), refused by the extractors; and, with the credential valid, with
//! each secret past it wrong (hold tokens in paths, bodies and the header,
//! passwords, setup and invitation tokens, session tokens), so that each is
//! refused by its own check inside the handler. Unknown paths carry one too. None of them may appear in the
//! output. The request span records the route template and never the path or
//! query (`cavea_telemetry::http`), so a path or query cannot leak by
//! construction; this test is the proof that nothing else does either. The
//! matrix (`authorisation_matrix`) checks the tenants' own credentials in
//! every refusal.

use tracing::subscriber::DefaultGuard;

use super::*;

/// A recognisable, obviously fake value: assembled at run time, letters,
/// digits and `-` only, so JSON output never escapes it.
fn canary(what: &str) -> String {
    ["logcanary", what, "7f3e9a"].join("-")
}

/// Capture everything, `trace` included, from every crate.
fn capture(logs: &Captured) -> DefaultGuard {
    let writer = logs.clone();
    let subscriber =
        cavea_telemetry::subscriber("trace", LogFormat::Json, move || writer.clone()).unwrap();
    tracing::subscriber::set_default(subscriber)
}

/// Every string in `value` that is a secret the server handed out: an API
/// key, a hold token, a CSRF token, a link with a token in its fragment.
fn handed_out(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (name, v) in map {
                if let Value::String(s) = v {
                    let secret = matches!(name.as_str(), "hold_token" | "csrf_token")
                        || (name == "key" && (s.starts_with("sk_") || s.starts_with("pk_")))
                        || (name == "link" && s.contains("#token="));
                    if secret {
                        let token = s.rsplit("#token=").next().unwrap_or(s);
                        found.push(token.to_owned());
                    }
                }
                handed_out(v, found);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| handed_out(v, found)),
        _ => {}
    }
}

#[tokio::test]
#[ignore = "needs CAVEA_TEST_DATABASE_URL"]
async fn secrets_never_reach_the_logs() {
    let _serial = serial();
    let logs = Captured::default();
    let _guard = capture(&logs);
    let logs_in = logs.clone();
    with_db(async move |pool| {
        let logs = logs_in;
        let app = api(&pool, Some(ADMIN));
        let a = Tenant::create(&app, "a").await;
        let b = Tenant::create(&app, "b").await;
        let mut members = Members::create(&pool, &a, &b).await;
        let owner = Cred::Member(Role::Owner);

        let header = |name: &'static str| header::HeaderName::from_static(name);
        let mut secrets: Vec<String> = Vec::new();
        // What a client may add to any request, sent with every success.
        let extra = [
            (header("x-canary"), canary("header")),
            (header::USER_AGENT, canary("agent")),
            (
                header::REFERER,
                format!("https://shop.example/?t={}", canary("referer")),
            ),
            (header("x-forwarded-for"), canary("forwarded")),
            (header::COOKIE, format!("other={}", canary("cookie"))),
        ];
        secrets.extend(extra.iter().map(|(_, v)| v.clone()));
        let order_ref = canary("order");
        secrets.push(order_ref.clone());
        // Wrong secrets, each in its own place.
        let password = canary("password");
        let bearer = canary("bearer");
        let session = canary("session");
        let csrf = canary("csrf");
        let session_token = canary("session-token");
        let hold_token = canary("hold-token");
        let query = canary("query");
        let token = canary("token");
        let identifier = canary("identifier");
        secrets.extend(
            [
                &password,
                &bearer,
                &session,
                &csrf,
                &session_token,
                &hold_token,
                &query,
                &token,
                &identifier,
            ]
            .map(|s| s.clone()),
        );

        // The image the upload row sends (see `png`), served by its hash.
        let upload = ROUTES
            .iter()
            .position(|(m, t, ..)| *m == Method::POST && *t == "/v1/assets")
            .unwrap();
        let asset: String = crate::assets::hash(&png(upload + 40))
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut downstream = 0;
        for (n, (method, template, ok, body)) in ROUTES.iter().enumerate() {
            let (needs, takes_session) = op_of(method, template);
            members.restore(&pool, &a).await;
            let path = members.path(&a.path(template)).replace("{asset}", &asset);
            let body = body.map(|b| {
                // One cell's objects, as in the matrix: the book row books
                // what the release row releases.
                a.path(
                    &b.replace("{n}", "30")
                        .replace("{m}", "130")
                        .replace("{k}", "230")
                        .replace("{p}", "330")
                        .replace("o-1", &order_ref),
                )
                .replace("{asset}", &asset)
            });
            let build = |path: &str,
                         authorization: Option<String>,
                         headers: &[(header::HeaderName, String)],
                         body: Option<&str>| {
                let mut request = Request::builder()
                    .method(method.clone())
                    .uri(path)
                    .header(header::IF_MATCH, "*");
                if let Some(value) = authorization {
                    request = request.header(header::AUTHORIZATION, value);
                }
                for (name, value) in headers {
                    request = request.header(name, value);
                }
                match body {
                    Some("{png}") => request
                        .header(header::CONTENT_TYPE, "image/png")
                        .body(Body::from(png(n + 40))),
                    Some(body) => request
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body.to_owned())),
                    None => request.body(Body::empty()),
                }
                .unwrap()
            };

            // As it succeeds, with the client's extra headers.
            let mut headers: Vec<(header::HeaderName, String)> = extra.to_vec();
            headers.push((HOLD_TOKEN, a.token.clone()));
            let idempotency = canary(&format!("idempotency-{n}"));
            headers.push((header("idempotency-key"), idempotency.clone()));
            secrets.push(idempotency);
            let (path_ok, authorization) = match needs {
                Needs::Admin => (path.clone(), Some(format!("Bearer {ADMIN}"))),
                Needs::Staff(_) => (path.clone(), Some(format!("Bearer {}", a.secret))),
                Needs::Publishable => (path.clone(), Some(format!("Bearer {}", a.publishable))),
                Needs::QueryKey => (with_key(&path, &a.publishable), None),
                Needs::Member(_) | Needs::Console => {
                    // The console's own cookie replaces the extra one.
                    headers.retain(|(name, _)| name != header::COOKIE);
                    headers.extend(members.headers(owner, &a, &b));
                    (path.clone(), None)
                }
                Needs::Nothing => (path.clone(), None),
            };
            // The credential valid, the secrets past it wrong: each refused
            // by its own check, after the extractors, so a handler that
            // logged what it refused would show. Before the success, which
            // may end the session.
            let swapped_path = path_ok.replace(&a.token, &hold_token);
            let swapped_body = body.as_deref().map(|b| {
                b.replace(&a.token, &hold_token)
                    .replace(PASSWORD, &password)
                    .replace("not-a-real-password-1", &password)
                    .replace("\"0000\"", &format!("\"{token}\""))
            });
            let fresh = canary(&format!("idempotency-wrong-{n}"));
            secrets.push(fresh.clone());
            let swapped_headers: Vec<(header::HeaderName, String)> = headers
                .iter()
                .map(|(name, value)| match name.as_str() {
                    "cavea-hold-token" => (name.clone(), hold_token.clone()),
                    "idempotency-key" => (name.clone(), fresh.clone()),
                    _ => (name.clone(), value.clone()),
                })
                .collect();
            if swapped_path != path_ok || swapped_body != body || *template == HEADER_TOKEN {
                let request = build(
                    &swapped_path,
                    authorization.clone(),
                    &swapped_headers,
                    swapped_body.as_deref(),
                );
                let (response, answer) = send(app.clone(), request).await;
                let status = response.status().as_u16();
                assert!(
                    [401, 403, 404].contains(&status),
                    "{method} {template} with a wrong secret: {answer}"
                );
                downstream += 1;
            }
            if takes_session {
                let mut with_session = headers.clone();
                with_session.push((SESSION, session_token.clone()));
                let request = build(
                    &path_ok,
                    authorization.clone(),
                    &with_session,
                    body.as_deref(),
                );
                let (response, answer) = send(app.clone(), request).await;
                assert_problem(&response, &answer, Code::Unauthorized);
                assert_eq!(answer["details"]["reason"], "session_token", "{template}");
                downstream += 1;
            }

            let request = build(&path_ok, authorization, &headers, body.as_deref());
            let (response, answer) = send(app.clone(), request).await;
            assert_eq!(
                response.status().as_u16(),
                *ok,
                "{method} {template}: {answer}"
            );
            handed_out(&answer, &mut secrets);
            if let Some(cookie) = response.headers().get(header::SET_COOKIE) {
                let value = cookie.to_str().unwrap();
                let id = value.split(';').next().unwrap().split('=').nth(1).unwrap();
                if !id.is_empty() {
                    secrets.push(id.to_owned());
                }
            }

            // Wrong secrets everywhere. Event and chart keys stay: they are
            // the customer's identifiers, logged on purpose (08 §15).
            let wrong_path = path_ok
                .replace(&a.token, &hold_token)
                .replace(&a.spare_id, &identifier)
                .replace(&a.id, &format!("ws_{identifier}"));
            let wrong_path = format!(
                "{wrong_path}{}canary={query}",
                if wrong_path.contains('?') { '&' } else { '?' }
            );
            let wrong_path = if needs == Needs::QueryKey {
                wrong_path.replace(&a.publishable, &bearer)
            } else {
                wrong_path
            };
            let wrong_body = body.as_deref().map(|b| {
                b.replace(&a.token, &hold_token)
                    .replace(PASSWORD, &password)
                    .replace("not-a-real-password-1", &password)
                    .replace("\"0000\"", &format!("\"{token}\""))
            });
            let wrong_headers = vec![
                (
                    header::COOKIE,
                    format!("__Host-cavea_console={session}; other={session}"),
                ),
                (crate::http::staff::CSRF, csrf.clone()),
                (crate::http::staff::WORKSPACE, format!("ws_{identifier}")),
                (SESSION, session_token.clone()),
                (HOLD_TOKEN, hold_token.clone()),
                (
                    header::ORIGIN,
                    format!("https://{}.example", canary("origin")),
                ),
            ];
            let request = build(
                &wrong_path,
                Some(format!("Bearer {bearer}")),
                &wrong_headers,
                wrong_body.as_deref(),
            );
            let (response, answer) = send(app.clone(), request).await;
            let refused = !response.status().is_success();
            assert!(
                refused || needs == Needs::Nothing,
                "{method} {template}: {answer}"
            );

            // A route that ends the session it was sent with: new ones.
            if template.starts_with("/console/api/password")
                || template.starts_with("/console/api/session")
            {
                for (_, id) in &members.sessions {
                    secrets.push(id.as_str().to_owned());
                    secrets.push(id.csrf());
                }
                members.sign_in(&pool).await;
            }
        }
        // Hold tokens in paths, bodies and the header; passwords; setup and
        // invitation tokens; session tokens.
        assert!(downstream >= 15, "{downstream}");
        // Unknown paths, with a secret in them.
        for uri in [
            format!("/v1/hold-tokens/{hold_token}/nope"),
            format!("/{token}"),
            format!("/v1/public/events/x/nope?key={bearer}"),
            format!("/console/api/{session}"),
        ] {
            let request = Request::builder().uri(&uri).body(Body::empty()).unwrap();
            let (response, _) = send(app.clone(), request).await;
            assert_eq!(response.status(), 404, "{uri}");
        }

        for (_, id) in &members.sessions {
            secrets.push(id.as_str().to_owned());
            secrets.push(id.csrf());
        }
        secrets.extend(
            a.secrets()
                .into_iter()
                .chain(b.secrets())
                .map(str::to_owned),
        );
        secrets.extend(["@a.example", "@b.example", PASSWORD].map(str::to_owned));
        // The routes handed out new keys, tokens and links.
        assert!(secrets.len() > 40, "{}", secrets.len());
        let refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
        assert_no_secrets(&logs, &refs);
        // And something was logged at the levels below `info`.
        assert!(logs.text().contains("\"level\":\"DEBUG\""));
    })
    .await;
}
