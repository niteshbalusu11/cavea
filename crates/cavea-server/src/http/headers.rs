//! Security headers (12 §3 T7, SEC-004; 08 §6.5): one layer on the whole
//! router, so a route or fallback added later gets its family's headers with
//! no handler involved. The family is decided by the path alone:
//!
//! - [`Family::Public`]: `/v1/public/…`, called from customers' pages on other
//!   origins (the renderer, its chart and image files, realtime). Their CORS
//!   is `public::cors` and `public::cors_open`, untouched here; they are
//!   `Cross-Origin-Resource-Policy: cross-origin`, so that a page isolated
//!   with `Cross-Origin-Embedder-Policy` may still load them.
//! - [`Family::Api`]: the rest of `/v1`, `/console/api`, `/healthz`,
//!   `/readyz` and `/metrics`: data, never a page, so a policy that loads
//!   nothing and may not be framed, and same-origin resource policy.
//! - [`Family::Console`]: everything else, the console's pages and files from
//!   `web`: the console's Content Security Policy, which allows only this
//!   origin's scripts, stylesheets, images and connections, WebAssembly
//!   compilation (`'wasm-unsafe-eval'`) and `style` attributes, and no
//!   inline script, `<style>` element, `eval`, plugin, framing or `<base>`.
//!
//! Every family: `X-Content-Type-Options: nosniff`, `Referrer-Policy:
//! no-referrer`, and `Strict-Transport-Security` when `CAVEA_PUBLIC_URL` is
//! `https://` (a browser ignores it over plain HTTP, and a development server
//! must not pin `localhost` to HTTPS). A header the handler set is kept: an
//! uploaded image's own `Content-Security-Policy: default-src 'none';
//! sandbox` (`http::assets`) is stricter than the family's.

use axum::extract::{Request, State};
use axum::http::header::{self, HeaderMap, HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

use super::AppState;

/// Which headers a path gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Console,
    Api,
    Public,
}

/// The family of a request path.
#[must_use]
pub fn family(path: &str) -> Family {
    let under = |prefix: &str| {
        path.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    };
    if under("/v1/public") {
        Family::Public
    } else if under("/v1")
        || under("/console/api")
        || matches!(path, "/healthz" | "/readyz" | "/metrics")
    {
        Family::Api
    } else {
        Family::Console
    }
}

/// The console's policy. Scripts: this origin's files and WebAssembly
/// compilation only. Styles: this origin's stylesheets, and `style`
/// attributes (`style-src-attr 'unsafe-inline'`, D-66), which Solid writes
/// into its compiled templates for a constant `style={{…}}`; no `<style>`
/// element. `connect-src 'self'` covers the API, the WebAssembly and atlas
/// fetches (the atlas is decoded from a fetched blob, which `img-src` does
/// not govern), and same-origin WebSockets.
pub const CONSOLE_CSP: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; \
style-src 'self'; style-src-attr 'unsafe-inline'; img-src 'self'; font-src 'self'; connect-src 'self'; \
worker-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; \
frame-ancestors 'none'";

/// Data, never a document: nothing loads, nothing frames it.
pub const API_CSP: &str = "default-src 'none'; frame-ancestors 'none'";

/// Powerful features the console never uses, off for it and anything it
/// could be made to embed.
pub const PERMISSIONS_POLICY: &str = "accelerometer=(), camera=(), display-capture=(), \
geolocation=(), gyroscope=(), magnetometer=(), microphone=(), midi=(), payment=(), \
publickey-credentials-get=(), serial=(), usb=(), xr-spatial-tracking=()";

/// A year; no `includeSubDomains`: the operator's other hosts are not ours.
pub const HSTS: &str = "max-age=31536000";

const PERMISSIONS: HeaderName = HeaderName::from_static("permissions-policy");
const COOP: HeaderName = HeaderName::from_static("cross-origin-opener-policy");
const CORP: HeaderName = HeaderName::from_static("cross-origin-resource-policy");

type Pairs = &'static [(HeaderName, &'static str)];

// Statics, not constants: a `HeaderName` cannot be borrowed from a constant.

static EVERY: [(HeaderName, &str); 2] = [
    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    (header::REFERRER_POLICY, "no-referrer"),
];
static CONSOLE: [(HeaderName, &str); 4] = [
    (header::CONTENT_SECURITY_POLICY, CONSOLE_CSP),
    (PERMISSIONS, PERMISSIONS_POLICY),
    (COOP, "same-origin"),
    (CORP, "same-origin"),
];
static API: [(HeaderName, &str); 2] = [
    (header::CONTENT_SECURITY_POLICY, API_CSP),
    (CORP, "same-origin"),
];
static PUBLIC: [(HeaderName, &str); 2] = [
    (header::CONTENT_SECURITY_POLICY, API_CSP),
    (CORP, "cross-origin"),
];
static TLS: [(HeaderName, &str); 1] = [(header::STRICT_TRANSPORT_SECURITY, HSTS)];

/// The headers `family` gets; `hsts` adds `Strict-Transport-Security`.
pub fn for_family(
    family: Family,
    hsts: bool,
) -> impl Iterator<Item = &'static (HeaderName, &'static str)> {
    let own: Pairs = match family {
        Family::Console => &CONSOLE,
        Family::Api => &API,
        Family::Public => &PUBLIC,
    };
    let tls: Pairs = if hsts { &TLS } else { &[] };
    EVERY.iter().chain(own).chain(tls)
}

/// The layer: the family's headers on every response, errors, `304`, `404`
/// and preflights included.
pub(super) async fn layer(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let family = family(request.uri().path());
    let hsts = state
        .config
        .public_url
        .as_ref()
        .is_some_and(|u| u.is_https());
    let mut response = next.run(request).await;
    add(response.headers_mut(), family, hsts);
    response
}

fn add(headers: &mut HeaderMap, family: Family, hsts: bool) {
    for (name, value) in for_family(family, hsts) {
        headers
            .entry(name.clone())
            .or_insert(HeaderValue::from_static(value));
    }
}
