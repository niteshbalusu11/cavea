//! Public endpoints (08 §6.2): what a buyer's browser calls, and their CORS
//! policy (08 §11).
//!
//! `GET /v1/public/charts/{hash}.ccf` serves a published chart's public
//! artefact by its content hash, to anyone: it takes no credential, so the URL
//! alone is the cache key of browsers and CDNs, and it is the same for every
//! workspace that publishes the same bytes. The full artefact is never served.
//!
//! The other public routes take a publishable key. `GET /v1/public/events/{event}`
//! is here; the seat routes are in `http::seats`. [`cors`] answers preflights
//! and adds the CORS headers to every response under `/v1/public/`, errors
//! included, so that the renderer can read them.

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{Next, map_response};
use axum::response::{IntoResponse, Response};
use cavea_engine::Clock as _;
use serde_json::json;
use tracing::Instrument as _;

use super::auth::PublishableKey;
use super::extract::Path;
use super::openapi::{Needs, Op, Routes};
use super::seats::{Find, target};
use super::{ApiError, AppState, Code};
use crate::config::CorsOrigins;

pub(super) fn routes() -> Routes {
    Routes::new()
        .layered(
            Op::get(
                "/v1/public/charts/{file}",
                "publicChart",
                "A compiled chart",
            )
            .about(
                "No credential: the content hash names the file. Brotli when \
`Accept-Encoding` allows it. Immutable; CORS open.",
            )
            .response(
                200,
                "The public artefact",
                Some(serde_json::json!({ CCF_TYPE: {} })),
            )
            .etag()
            .response(304, "Not modified", None)
            .etag(),
            chart,
            |route| route.layer(map_response(cors_open)),
        )
        .add(
            Op::get(
                "/v1/public/events/{event}",
                "publicEvent",
                "What the renderer needs to start",
            )
            .needs(Needs::Publishable)
            .reply(
                200,
                "PublicEvent",
                "The chart file, hold lifetime and selection limit",
            ),
            event,
        )
        .merge(super::seats::public_routes())
}

/// Browsers may cache a preflight this long (Chromium's cap).
const PREFLIGHT_MAX_AGE: &str = "7200";
const ALLOW_METHODS: &str = "GET, POST";
const ALLOW_HEADERS: &str =
    "Authorization, Content-Type, Cavea-Hold-Token, Cavea-Request-Id, Cavea-Session";
const EXPOSE_HEADERS: &str = "Cavea-Request-Id, Retry-After";

/// CORS for the publishable-key routes, as a layer on the whole router (08
/// §11). Only paths under `/v1/public/` are touched, so a secret-key route
/// never carries a CORS header (12 §4), and the chart and asset files keep their own
/// open policy. `OPTIONS` there is a preflight: it carries no credential, so
/// it is answered from `CAVEA_CORS_ORIGINS` alone; the key's own allow-list is
/// checked on the request itself (`403`, with these headers). Credentials mode
/// stays off: no `Access-Control-Allow-Credentials`, no cookies.
pub(super) async fn cors(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if !path.starts_with("/v1/public/")
        || path.starts_with("/v1/public/charts/")
        || path.starts_with("/v1/public/assets/")
    {
        return next.run(request).await;
    }
    let allow = match &state.config.cors_origins {
        CorsOrigins::Any => Some(HeaderValue::from_static("*")),
        CorsOrigins::List(list) => request
            .headers()
            .get(header::ORIGIN)
            .filter(|o| {
                o.to_str()
                    .is_ok_and(|o| list.iter().any(|a| a.eq_ignore_ascii_case(o)))
            })
            .cloned(),
    };
    let preflight = request.method() == Method::OPTIONS;
    let mut response = if preflight {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(request).await
    };
    let headers = response.headers_mut();
    if matches!(state.config.cors_origins, CorsOrigins::List(_)) {
        headers.append(header::VARY, HeaderValue::from_static("Origin"));
    }
    let Some(allow) = allow else {
        return response;
    };
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, allow);
    if preflight {
        let pairs = [
            (header::ACCESS_CONTROL_ALLOW_METHODS, ALLOW_METHODS),
            (header::ACCESS_CONTROL_ALLOW_HEADERS, ALLOW_HEADERS),
            (header::ACCESS_CONTROL_MAX_AGE, PREFLIGHT_MAX_AGE),
        ];
        for (name, value) in pairs {
            headers.insert(name, HeaderValue::from_static(value));
        }
    } else {
        headers.insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static(EXPOSE_HEADERS),
        );
    }
    response
}

/// What the renderer needs to start (05 §4): the compiled chart of the pinned
/// version, the hold lifetime and the selection limit of a buyer's token, and
/// whether holding needs a session token from the host page (08 §11), and
/// what the renderer needs of the licence to decide its mark (12 §7.4,
/// `Licences::public`).
/// Browsers may keep it briefly, per credential; shared caches may not, since
/// the answer depends on the credential.
async fn event(
    PublishableKey(auth): PublishableKey,
    Path(event): Path<String>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let target = target(&state, &auth, Find::Key(&event)).await?;
    let body = json!({
        "event": target.key,
        "chart": {
            "hash": target.hash,
            "url": format!("/v1/public/charts/{}.ccf", target.hash),
        },
        "hold_ttl_seconds": target.settings.hold_ttl(),
        "max_objects": target.settings.max_objects(),
        "require_session": target.settings.require_session(),
        // For the renderer's mark (12 §7.4); from memory, never failing.
        "licence": state.licences.public(state.seats.clock.now().0),
    });
    let mut response = axum::Json(body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=30"),
    );
    // One browser may hold keys of two workspaces with the same event key.
    headers.append(header::VARY, HeaderValue::from_static("Authorization"));
    Ok(response)
}

const CCF_TYPE: &str = "application/vnd.cavea.ccf";
pub(super) const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// CORS open (03 §4.6): any page may read the response, whatever
/// `CAVEA_CORS_ORIGINS` says: the file is public, and one answer for every
/// origin keeps it one cache entry at a CDN. A GET without custom headers
/// needs no preflight.
pub(super) async fn cors_open(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    response
}

const IDENTITY: &str = "SELECT public_ccf FROM chart_versions WHERE hash = $1 LIMIT 1";
const BROTLI: &str = "SELECT public_ccf_br FROM chart_versions WHERE hash = $1 LIMIT 1";
const EXISTS: &str = "SELECT EXISTS (SELECT FROM chart_versions WHERE hash = $1)";

/// The public artefact with this content hash: brotli if `Accept-Encoding`
/// allows it, else identity. Every row with the hash holds the same bytes.
async fn chart(
    Path(file): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let hash = file
        .strip_suffix(".ccf")
        .and_then(parse_hash)
        .ok_or_else(|| ApiError::new(Code::NotFound).with_message("No such chart."))?;
    let brotli = accepts_brotli(&headers);
    let hex = file.get(..32).unwrap_or_default();
    // Each encoding is its own representation, with its own strong tag.
    let etag = if brotli {
        format!("\"{hex}.br\"")
    } else {
        format!("\"{hex}\"")
    };
    let etag = HeaderValue::try_from(etag).map_err(ApiError::internal)?;
    let not_found = || ApiError::new(Code::NotFound).with_message("No such chart.");
    let (listed, any) = none_match(&headers, &etag);
    let response = if listed || any {
        // The caller's copy is current if the file exists: the bytes behind a
        // hash never change, but a hash need not be (or still be) published.
        let exists: bool = sqlx::query_scalar(EXISTS)
            .bind(hash.as_slice())
            .fetch_one(&state.db)
            .instrument(cavea_telemetry::db_span("read_public_chart"))
            .await?;
        if !exists {
            return Err(not_found());
        }
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let bytes: Option<Vec<u8>> = sqlx::query_scalar(if brotli { BROTLI } else { IDENTITY })
            .bind(hash.as_slice())
            .fetch_optional(&state.db)
            .instrument(cavea_telemetry::db_span("read_public_chart"))
            .await?;
        let mut response = bytes.ok_or_else(not_found)?.into_response();
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(CCF_TYPE));
        if brotli {
            headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("br"));
        }
        response
    };
    Ok(cache_headers(response, etag))
}

/// The caching headers of every successful answer, `304` included.
fn cache_headers(mut response: Response, etag: HeaderValue) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(IMMUTABLE));
    headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    headers.insert(header::ETAG, etag);
    response
}

/// 32 lowercase hex digits as 16 bytes.
pub(crate) fn parse_hash(hex: &str) -> Option<[u8; 16]> {
    let digits = hex.as_bytes();
    if digits.len() != 32 {
        return None;
    }
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    };
    let mut out = [0; 16];
    let (pairs, _) = digits.as_chunks::<2>();
    for (byte, [hi, lo]) in out.iter_mut().zip(pairs) {
        *byte = digit(*hi)? << 4 | digit(*lo)?;
    }
    Some(out)
}

/// Whether `Accept-Encoding` accepts `br`: listed with a non-zero weight, or
/// not listed and `*` is, with a non-zero weight.
fn accepts_brotli(headers: &HeaderMap) -> bool {
    let mut star = None;
    for item in headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
    {
        let mut parts = item.split(';');
        let coding = parts.next().unwrap_or_default().trim();
        let accepted = parts.all(|p| {
            let p = p.trim();
            let weight = p.strip_prefix("q=").or_else(|| p.strip_prefix("Q="));
            weight.is_none_or(|q| q.trim().parse::<f32>().is_ok_and(|q| q > 0.0))
        });
        if coding.eq_ignore_ascii_case("br") {
            return accepted;
        }
        if coding == "*" {
            star = Some(accepted);
        }
    }
    star.unwrap_or(false)
}

/// Whether `If-None-Match` lists `etag`, compared weakly (RFC 9110 §13.1.2),
/// and whether it is `*`, which matches if the file exists.
pub(super) fn none_match(headers: &HeaderMap, etag: &HeaderValue) -> (bool, bool) {
    let etag = etag.as_bytes();
    let (mut listed, mut any) = (false, false);
    for tag in headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
    {
        listed |= tag.strip_prefix("W/").unwrap_or(tag).as_bytes() == etag;
        any |= tag == "*";
    }
    (listed, any)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brotli_is_chosen_only_when_accepted() {
        for (accept, brotli) in [
            (None, false),
            (Some(""), false),
            (Some("gzip, deflate"), false),
            (Some("gzip, deflate, br, zstd"), true),
            (Some("BR"), true),
            (Some("br;q=0"), false),
            (Some("br; q=0.0, gzip"), false),
            (Some("br;q=0.5"), true),
            (Some("*"), true),
            (Some("br;q=0, *"), false),
            (Some("*;q=0"), false),
            (Some("identity"), false),
        ] {
            let mut headers = HeaderMap::new();
            if let Some(accept) = accept {
                headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_static(accept));
            }
            assert_eq!(accepts_brotli(&headers), brotli, "{accept:?}");
        }
    }
}
