//! The console's files (CON-015, `http::web`), served from a small fixture
//! table, so that no built console is needed: cache headers per kind, the
//! brotli form by `Accept-Encoding`, `index.html` for navigations only, API
//! paths never shadowed, `HEAD` and `If-None-Match`, and nothing that depends
//! on a credential.

use super::*;
use crate::http::web::{File, Files};

static FIXTURE: &[File] = &[
    File {
        path: "/index.html",
        body: b"<!doctype html><title>console</title>",
        br: None,
    },
    File {
        path: "/assets/index-abc123.js",
        body: b"console.log('console')",
        br: Some(b"brotli js"),
    },
    File {
        path: "/assets/designer-def456.wasm",
        body: b"\0asm\x01\0\0\0",
        br: Some(b"brotli wasm"),
    },
];

fn console(files: &'static [File]) -> Router {
    let mut state = state();
    state.console = Files::new(files);
    finish(Router::new(), state)
}

/// Send `method path` with `headers`: status, headers, body.
async fn get(
    app: &Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, axum::http::HeaderMap, Bytes) {
    let mut request = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    (parts.status, parts.headers, body)
}

fn header<'a>(headers: &'a axum::http::HeaderMap, name: &str) -> &'a str {
    headers.get(name).map_or("", |v| v.to_str().unwrap())
}

#[tokio::test]
async fn console_files_follow_the_serving_rules() {
    let _serial = serial();
    let app = console(FIXTURE);
    let html = [("accept", "text/html,application/xhtml+xml,*/*;q=0.8")];

    // A hashed asset: immutable, typed, sniffing off, brotli on request.
    let js = "/assets/index-abc123.js";
    let (status, headers, body) = get(&app, "GET", js, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(&body[..], b"console.log('console')");
    assert_eq!(
        header(&headers, "content-type"),
        "text/javascript; charset=utf-8"
    );
    let immutable = "public, max-age=31536000, immutable";
    assert_eq!(header(&headers, "cache-control"), immutable);
    assert_eq!(header(&headers, "x-content-type-options"), "nosniff");
    assert_eq!(header(&headers, "vary"), "Accept-Encoding");
    assert_eq!(header(&headers, "content-encoding"), "");
    let identity = header(&headers, "etag").to_owned();
    for (accept, br) in [
        ("gzip, deflate, br, zstd", true),
        ("*", true),
        ("br;q=0, *", false),
        ("gzip", false),
        ("BR;q=0.5", true),
    ] {
        let (_, headers, body) = get(&app, "GET", js, &[("accept-encoding", accept)]).await;
        assert_eq!(header(&headers, "content-encoding") == "br", br, "{accept}");
        assert_eq!(&body[..] == b"brotli js", br, "{accept}");
        assert_eq!(header(&headers, "etag") != identity, br, "{accept}");
    }
    let (_, headers, _) = get(&app, "GET", "/assets/designer-def456.wasm", &[]).await;
    assert_eq!(header(&headers, "content-type"), "application/wasm");

    // Conditional and HEAD.
    for tag in [identity.clone(), format!("W/{identity}"), "*".to_owned()] {
        let (status, headers, body) = get(&app, "GET", js, &[("if-none-match", &tag)]).await;
        assert_eq!((status, body.len()), (StatusCode::NOT_MODIFIED, 0), "{tag}");
        assert_eq!(header(&headers, "etag"), identity);
    }
    let (status, headers, body) = get(&app, "HEAD", js, &[]).await;
    assert_eq!((status, body.len()), (StatusCode::OK, 0));
    assert_eq!(header(&headers, "content-length"), "22");

    // index.html: revalidated; the console's routes for navigations only.
    for path in [
        "/",
        "/index.html",
        "/charts/royal",
        "/console/setup",
        "/designer/local",
    ] {
        let (status, headers, body) = get(&app, "GET", path, &html).await;
        assert_eq!(status, 200, "{path}");
        assert_eq!(header(&headers, "cache-control"), "no-cache", "{path}");
        assert_eq!(header(&headers, "content-type"), "text/html; charset=utf-8");
        assert!(body.starts_with(b"<!doctype html>"), "{path}");
    }
    // Never HTML: a script or image that is not there, any other method, and
    // every API path, which keeps its problem+json.
    for (method, path, accept) in [
        ("GET", "/charts/royal", "*/*"),
        ("GET", "/assets/missing-0000.js", "text/html"),
        ("GET", "/assets/../index.html", "text/html"),
        ("GET", "/assets/..%2findex.html", "text/html"),
        ("POST", "/charts", "text/html"),
        ("GET", "/v1", "text/html"),
        ("GET", "/v1/no-such-thing", "text/html"),
        ("GET", "/console/api/no-such-thing", "text/html"),
        ("GET", "/metrics", "text/html"),
    ] {
        let (status, headers, _) = get(&app, method, path, &[("accept", accept)]).await;
        assert_eq!(status, 404, "{method} {path}");
        let problem = header(&headers, "content-type");
        assert_eq!(problem, "application/problem+json", "{method} {path}");
    }

    // The same bytes and headers whatever credential is sent, and the
    // console cookie's no-store does not apply.
    let plain = get(&app, "GET", js, &[]).await;
    for credential in [
        (
            "authorization",
            "Bearer sk_0000000000000000000000000000000000000000000000000000000000000000",
        ),
        (
            "cookie",
            "__Host-cavea_console=cs_0000000000000000000000000000000000000000000000000000000000000000",
        ),
    ] {
        let (status, mut headers, body) = get(&app, "GET", js, &[credential]).await;
        headers.remove(REQUEST_ID_HEADER);
        let mut expected = plain.1.clone();
        expected.remove(REQUEST_ID_HEADER);
        assert_eq!(
            (status, headers, body),
            (plain.0, expected, plain.2.clone())
        );
    }
}

#[tokio::test]
async fn without_a_built_console_its_paths_say_so() {
    let _serial = serial();
    let app = console(&[]);
    let (status, headers, body) =
        get(&app, "GET", "/console/setup", &[("accept", "text/html")]).await;
    assert_eq!(status, 404);
    assert_eq!(
        header(&headers, "content-type"),
        "text/plain; charset=utf-8"
    );
    assert!(String::from_utf8_lossy(&body).contains("cargo xtask build-server"));
    let (_, headers, _) = get(&app, "GET", "/v1/no-such-thing", &[("accept", "text/html")]).await;
    assert_eq!(header(&headers, "content-type"), "application/problem+json");
}
