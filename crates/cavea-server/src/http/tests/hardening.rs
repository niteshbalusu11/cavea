//! SEC-004 and SEC-006 without a database: every route and the console's
//! files carry their family's security headers (08 §6.5), the public routes
//! keep the CORS that embedding needs, and each insecure setting is named
//! once at start.

use axum::http::Method;

use super::*;
use crate::http::headers::{API_CSP, CONSOLE_CSP, HSTS, PERMISSIONS_POLICY};
use crate::http::web::{File, Files};

static CONSOLE_FILES: &[File] = &[
    File {
        path: "/index.html",
        body: b"<!doctype html><title>console</title>",
        br: None,
    },
    File {
        path: "/assets/index-abc123.js",
        body: b"console.log('console')",
        br: None,
    },
];

/// What a family's response must carry, written out from 08 §6.5's table
/// rather than taken from `headers`, so a change to either shows.
fn expected(path: &str) -> Vec<(&'static str, &'static str)> {
    let mut want = vec![
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("strict-transport-security", HSTS),
    ];
    let public = path.starts_with("/v1/public/");
    let api = path.starts_with("/v1/")
        || path.starts_with("/console/api/")
        || ["/healthz", "/readyz", "/metrics"].contains(&path);
    if public {
        want.extend([
            ("cross-origin-resource-policy", "cross-origin"),
            ("content-security-policy", API_CSP),
        ]);
    } else if api {
        want.extend([
            ("cross-origin-resource-policy", "same-origin"),
            ("content-security-policy", API_CSP),
        ]);
    } else {
        want.extend([
            ("cross-origin-resource-policy", "same-origin"),
            ("cross-origin-opener-policy", "same-origin"),
            ("permissions-policy", PERMISSIONS_POLICY),
            ("content-security-policy", CONSOLE_CSP),
        ]);
    }
    want
}

fn app_with(public_url: Option<&str>) -> Router {
    let mut state = state();
    Arc::make_mut(&mut state.config).public_url = public_url.map(|u| u.parse().unwrap());
    state.console = Files::new(CONSOLE_FILES);
    finish(super::super::routes().into_router(), state)
}

/// A route template with every parameter filled.
fn concrete(template: &str) -> String {
    template
        .replace("{file}", "0123.ccf")
        .split('/')
        .map(|part| if part.starts_with('{') { "x" } else { part })
        .collect::<Vec<_>>()
        .join("/")
}

/// SEC-004: every operation of the route table, and the console's pages,
/// files and unknown paths, answer with their family's headers whatever the
/// status (here mostly `401`: no credential is sent), so that a route added
/// later cannot miss them; HSTS only with an HTTPS public URL; no
/// credentialed CORS and no embedder policy anywhere; the public routes stay
/// readable from any origin.
#[tokio::test]
async fn every_route_carries_its_familys_headers() {
    let _serial = serial();
    let https = app_with(Some("https://seats.example.com"));
    let http = app_with(Some("http://localhost:8080"));
    let mut requests: Vec<(Method, String, Option<&str>)> = super::super::routes()
        .ops()
        .iter()
        .map(|op| (op.method.clone(), concrete(op.path), None))
        .collect();
    assert!(requests.len() > 40, "{}", requests.len());
    for path in ["/", "/index.html", "/assets/index-abc123.js", "/charts/x"] {
        requests.push((Method::GET, path.to_owned(), Some("text/html")));
    }
    for path in [
        "/assets/missing.js",
        "/v1/nope",
        "/console/api/nope",
        "/nope",
    ] {
        requests.push((Method::GET, path.to_owned(), None));
    }
    // A preflight, a method no route has.
    requests.push((Method::OPTIONS, "/v1/public/hold-tokens".to_owned(), None));
    requests.push((Method::PUT, "/v1/events".to_owned(), None));
    let mut public = 0;
    for (method, path, accept) in &requests {
        for (app, tls) in [(&https, true), (&http, false)] {
            let mut request = Request::builder()
                .method(method.clone())
                .uri(path.as_str())
                .header(header::ORIGIN, "https://tickets.example");
            if let Some(accept) = accept {
                request = request.header(header::ACCEPT, *accept);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            let h = response.headers();
            let at = format!("{method} {path} ({})", response.status());
            for (name, value) in expected(path) {
                if name == "strict-transport-security" && !tls {
                    assert!(h.get(name).is_none(), "{at}: HSTS over HTTP");
                    continue;
                }
                assert_eq!(
                    h.get(name).map(|v| v.to_str().unwrap()),
                    Some(value),
                    "{at}: {name}"
                );
            }
            assert!(h.get("cross-origin-embedder-policy").is_none(), "{at}");
            assert!(
                h.get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS).is_none(),
                "{at}"
            );
            let cors = h.get(header::ACCESS_CONTROL_ALLOW_ORIGIN);
            if path.starts_with("/v1/public/") {
                assert_eq!(cors.map(|v| v.to_str().unwrap()), Some("*"), "{at}");
                public += 1;
            } else {
                assert!(cors.is_none(), "{at}");
            }
        }
    }
    assert!(public >= 20, "{public}");
}

/// SEC-006 (12 §4): each insecure setting is one `warn` record naming the
/// variable; a production set-up and the development defaults warn about
/// nothing but an unset admin key.
#[tokio::test]
async fn insecure_settings_are_named_once_at_start() {
    let _serial = serial();
    let production = || {
        let mut config = (*state().config).clone();
        config.public_url = Some("https://seats.example.com".parse().unwrap());
        config.admin_key = Some("an-admin-key-of-32-characters-0000".parse().unwrap());
        config.rate_limit_sign_in = 10;
        config.trusted_proxies = "10.0.0.0/8,2001:db8::/32".parse().unwrap();
        config
    };
    let flagged = |config: &Config| -> Vec<&'static str> {
        config.insecure().iter().map(|i| i.setting).collect()
    };
    assert_eq!(flagged(&production()), Vec::<&str>::new());
    for url in [
        None,
        Some("http://localhost:8080"),
        Some("http://LOCALHOST"),
        Some("http://127.0.0.1:8080/cavea"),
        Some("http://[::1]:8080"),
    ] {
        let mut config = production();
        config.public_url = url.map(|u| u.parse().unwrap());
        assert_eq!(flagged(&config), Vec::<&str>::new(), "{url:?}");
    }
    let cases: [(&str, fn(&mut Config)); 10] = [
        ("CAVEA_PUBLIC_URL", |c| {
            c.public_url = Some("http://seats.example.com".parse().unwrap());
        }),
        ("CAVEA_PUBLIC_URL", |c| {
            c.public_url = Some("http://localhost.example.com".parse().unwrap());
        }),
        ("CAVEA_CONSOLE_COOKIE_SECURE", |c| {
            c.console_cookie_secure = false;
        }),
        ("CAVEA_ADMIN_KEY", |c| c.admin_key = None),
        ("CAVEA_RATE_LIMIT_HOLDS", |c| c.rate_limit_holds = 0),
        ("CAVEA_RATE_LIMIT_READS", |c| c.rate_limit_reads = 0),
        ("CAVEA_RATE_LIMIT_SIGN_IN", |c| c.rate_limit_sign_in = 0),
        ("CAVEA_TRUSTED_PROXIES", |c| {
            c.trusted_proxies = "10.0.0.0/8, 0.0.0.0/0".parse().unwrap();
        }),
        ("CAVEA_TRUSTED_PROXIES", |c| {
            c.trusted_proxies = "::/0".parse().unwrap();
        }),
        ("CAVEA_TRUSTED_PROXIES", |c| {
            c.trusted_proxies = "0.0.0.0/0".parse().unwrap();
        }),
    ];
    for (setting, change) in cases {
        let mut config = production();
        change(&mut config);
        assert_eq!(flagged(&config), [setting]);
    }
    // `0.0.0.0/1` is half the addresses, not all: a mistake, but not this one.
    let mut config = production();
    config.trusted_proxies = "0.0.0.0/1".parse().unwrap();
    assert_eq!(flagged(&config), Vec::<&str>::new());

    // As logged: one record each, the variable in `setting`, never a value.
    let mut config = production();
    config.public_url = Some("http://seats.example.com".parse().unwrap());
    config.console_cookie_secure = false;
    config.rate_limit_holds = 0;
    let logs = Captured::default();
    let _guard = logs.install();
    config.warn_insecure();
    let records = logs.records();
    let settings: Vec<&Value> = records.iter().map(|r| &r["setting"]).collect();
    assert_eq!(
        settings,
        [
            "CAVEA_PUBLIC_URL",
            "CAVEA_CONSOLE_COOKIE_SECURE",
            "CAVEA_RATE_LIMIT_HOLDS"
        ]
    );
    for record in &records {
        assert_eq!(record["level"], "WARN");
        assert_eq!(record["message"], "insecure setting");
    }
    let text = logs.text();
    assert!(
        !text.contains("seats.example.com") && !text.contains("an-admin-key"),
        "{text}"
    );
}
