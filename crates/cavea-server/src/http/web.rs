//! The console's files, served from the binary (09 §9, §10; CON-015): the
//! router's fallback, so that no API route is ever shadowed. `cargo xtask
//! build-server` builds the console and compiles its files in (`build.rs`);
//! without that the table is empty, and the console's paths say so.
//!
//! - A file is found by its exact path in a fixed table: nothing is read from
//!   disk, and no request path can name anything else.
//! - Files under `/assets/` carry Vite's content hash in their names and are
//!   `public, max-age=31536000, immutable`; the rest (`index.html`) `no-cache`.
//!   Each has a strong `ETag` (`304` for `If-None-Match`).
//! - A brotli form, compressed at build time, is sent to a request that
//!   accepts `br`, with `Vary: Accept-Encoding`.
//! - A path that is no file answers `index.html` to a navigation (`Accept`
//!   naming `text/html`), for the console's own routes (`/charts/…`,
//!   `/console/setup`), and `404` otherwise, so a missing script or image is
//!   never HTML. `/v1`, `/console/api`, `/healthz`, `/readyz` and `/metrics`
//!   are never the console's: their unknown paths answer problem+json `404`.
//! - The same bytes for everyone: no credential is read, and the console
//!   cookie's `no-store` (`staff::no_store_for_cookies`) is not applied.

use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use sha2::{Digest as _, Sha256};

use super::{ApiError, AppState, Code};

/// One file of the built console, as `cargo xtask build-server` lists it.
#[derive(Debug)]
pub struct File {
    /// The request path: `/index.html`, `/assets/index-4f2a….js`.
    pub path: &'static str,
    pub body: &'static [u8],
    /// The brotli form, when it is smaller.
    pub br: Option<&'static [u8]>,
}

include!(concat!(env!("OUT_DIR"), "/console.rs"));

/// The console's files, ready to serve. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Files(Arc<[Served]>);

#[derive(Debug)]
struct Served {
    file: &'static File,
    /// 32 hex digits of the body's SHA-256.
    etag: String,
    content_type: &'static str,
    immutable: bool,
}

/// Marks a response that is the same for every request (`no-store` skips it).
#[derive(Debug, Clone, Copy)]
pub struct Shared;

impl Files {
    /// The files compiled into this binary; none in a plain `cargo build`.
    #[must_use]
    pub fn embedded() -> Self {
        Self::new(EMBEDDED)
    }

    #[must_use]
    pub fn new(files: &'static [File]) -> Self {
        let mut served: Vec<Served> = files
            .iter()
            .map(|file| Served {
                file,
                etag: Sha256::digest(file.body)[..16]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
                content_type: content_type(file.path),
                immutable: file.path.starts_with("/assets/"),
            })
            .collect();
        served.sort_by_key(|s| s.file.path);
        Self(served.into())
    }

    fn get(&self, path: &str) -> Option<&Served> {
        let found = self.0.binary_search_by_key(&path, |s| s.file.path).ok()?;
        self.0.get(found)
    }

    /// How many files, and their bytes with their brotli forms.
    #[must_use]
    pub fn size(&self) -> (usize, usize) {
        let bytes = self
            .0
            .iter()
            .map(|s| s.file.body.len() + s.file.br.map_or(0, <[u8]>::len));
        (self.0.len(), bytes.sum())
    }
}

/// The router's fallback: a console file, `index.html` for a navigation, or
/// the API's problem+json `404`.
pub(super) async fn fallback(State(state): State<AppState>, request: Request) -> Response {
    let method = request.method();
    let path = request.uri().path();
    let headers = request.headers();
    if !(method == Method::GET || method == Method::HEAD) || is_api(path) {
        return no_endpoint();
    }
    let head = method == Method::HEAD;
    if let Some(file) = state.console.get(path) {
        return serve(file, headers, head);
    }
    let navigation = path == "/"
        || (!path.starts_with("/assets/")
            && headers
                .get_all(header::ACCEPT)
                .iter()
                .any(|v| v.to_str().is_ok_and(|v| v.contains("text/html"))));
    if !navigation {
        return no_endpoint();
    }
    match state.console.get("/index.html") {
        Some(index) => serve(index, headers, head),
        None => not_built(),
    }
}

fn no_endpoint() -> Response {
    ApiError::new(Code::NotFound)
        .with_message("No such endpoint.")
        .into_response()
}

/// The paths that are the API's, never the console's.
fn is_api(path: &str) -> bool {
    let under = |prefix: &str| {
        path.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    };
    under("/v1") || under("/console/api") || matches!(path, "/healthz" | "/readyz" | "/metrics")
}

const NOT_BUILT: &str = "The console is not built into this cavea-server binary.\n\
Build a server with it: `cargo xtask build-server` (or the Docker image, deploy/Dockerfile).\n\
The API is served at /v1.\n";

fn not_built() -> Response {
    let mut response = (StatusCode::NOT_FOUND, NOT_BUILT).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn serve(file: &Served, request: &HeaderMap, head: bool) -> Response {
    let br = file.file.br.filter(|_| accepts_br(request));
    let (body, etag) = match br {
        Some(br) => (br, format!("\"{}-br\"", file.etag)),
        None => (file.file.body, format!("\"{}\"", file.etag)),
    };
    let mut headers = HeaderMap::new();
    let cache = if file.immutable {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Ok(etag) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, etag);
    }
    if file.file.br.is_some() {
        headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    }
    let mut response = if matches(request, &etag) {
        (StatusCode::NOT_MODIFIED, headers).into_response()
    } else {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(file.content_type),
        );
        if br.is_some() {
            headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("br"));
        }
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
        let body = if head {
            Body::empty()
        } else {
            Body::from(Bytes::from_static(body))
        };
        (headers, body).into_response()
    };
    response.extensions_mut().insert(Shared);
    response
}

/// Whether `Accept-Encoding` takes `br`: named with a non-zero `q`, or `*`
/// with a non-zero `q` and `br` not named.
fn accepts_br(headers: &HeaderMap) -> bool {
    let mut any = false;
    let items = headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','));
    for item in items {
        let mut parts = item.split(';');
        let coding = parts.next().unwrap_or_default().trim();
        let q = parts
            .find_map(|p| p.trim().strip_prefix("q="))
            .map_or(1.0, |q| q.trim().parse::<f32>().unwrap_or(0.0));
        if coding.eq_ignore_ascii_case("br") {
            return q > 0.0;
        }
        any |= coding == "*" && q > 0.0;
    }
    any
}

/// Whether `If-None-Match` names `etag` (weak comparison) or is `*`.
fn matches(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|tag| tag.trim())
        .any(|tag| tag == "*" || tag.trim_start_matches("W/") == etag)
}

fn content_type(path: &str) -> &'static str {
    let extension = path.rsplit_once('.').map_or("", |(_, e)| e);
    match extension {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "wasm" => "application/wasm",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
