//! Public endpoints (08 §6.2): so far, compiled charts.
//!
//! `GET /v1/public/charts/{hash}.ccf` serves a published chart's public
//! artefact by its content hash, to anyone: it takes no credential, so the URL
//! alone is the cache key of browsers and CDNs, and it is the same for every
//! workspace that publishes the same bytes. The full artefact is never served.

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::map_response;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tracing::Instrument as _;

use super::extract::Path;
use super::{ApiError, AppState, Code};

pub(super) fn routes() -> Router<AppState> {
    Router::new().route(
        "/v1/public/charts/{file}",
        get(chart).layer(map_response(cors_open)),
    )
}

const CCF_TYPE: &str = "application/vnd.cavea.ccf";
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// CORS open (03 §4.6): any page may read the response. A GET without custom
/// headers needs no preflight. SRV-015 replaces this with the configured
/// policy of the public endpoints.
async fn cors_open(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    response
}

const IDENTITY: &str = "SELECT public_ccf FROM chart_versions WHERE hash = $1 LIMIT 1";
const BROTLI: &str = "SELECT public_ccf_br FROM chart_versions WHERE hash = $1 LIMIT 1";

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
    let (listed, any) = none_match(&headers, &etag);
    let response = if listed {
        // The content behind a hash never changes: the caller's copy is current.
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let bytes: Option<Vec<u8>> = sqlx::query_scalar(if brotli { BROTLI } else { IDENTITY })
            .bind(hash.as_slice())
            .fetch_optional(&state.db)
            .instrument(cavea_telemetry::db_span("read_public_chart"))
            .await?;
        let bytes =
            bytes.ok_or_else(|| ApiError::new(Code::NotFound).with_message("No such chart."))?;
        if any {
            // `If-None-Match: *` and the file exists.
            return Ok(cache_headers(
                StatusCode::NOT_MODIFIED.into_response(),
                etag,
            ));
        }
        let mut response = bytes.into_response();
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
fn parse_hash(hex: &str) -> Option<[u8; 16]> {
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
fn none_match(headers: &HeaderMap, etag: &HeaderValue) -> (bool, bool) {
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
