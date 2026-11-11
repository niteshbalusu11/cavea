//! The staff's console session as a credential (09 §2, §10; 08 §11): the
//! cookie, the CSRF token and the origin checks, and [`ConsoleUser`], the
//! extractor of the `/console/api` routes of a signed-in user. The `/v1`
//! routes take API keys only, as before (CON-004 adds the session there).
//!
//! Every non-safe method authenticated by the cookie must carry the session's
//! CSRF token in `Cavea-Csrf-Token`, and must come from the server's own
//! origin (`Sec-Fetch-Site`, else `Origin` against `Host`). No route answers
//! cross-origin with credentials: CORS is only on `/v1/public/`, without
//! `Access-Control-Allow-Credentials`, and those routes take no cookie (08
//! §11).

use axum::extract::FromRequestParts;
use axum::http::header::{self, HeaderName};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method};
use subtle::ConstantTimeEq as _;

use super::{ApiError, AppState, Code};
use crate::console::sessions::{self, Found};
use crate::console::{ABSOLUTE, SessionId};

/// The CSRF token's header.
pub const CSRF: HeaderName = HeaderName::from_static("cavea-csrf-token");
const SEC_FETCH_SITE: HeaderName = HeaderName::from_static("sec-fetch-site");

/// The cookie's name: `__Host-` prefixed (only over HTTPS, for this host
/// alone, path `/`), unless `CAVEA_CONSOLE_COOKIE_SECURE=false`.
#[must_use]
pub fn cookie_name(state: &AppState) -> &'static str {
    if state.config.console_cookie_secure {
        "__Host-cavea_console"
    } else {
        "cavea_console"
    }
}

/// `Set-Cookie` for a new session: `HttpOnly`, `SameSite=Lax`, `Path=/`,
/// `Secure` unless turned off, kept by the browser for the absolute lifetime
/// (the server ends it sooner when idle).
pub fn set_cookie(state: &AppState, id: &SessionId) -> Result<HeaderValue, ApiError> {
    cookie(state, id.as_str(), ABSOLUTE.as_secs())
}

/// `Set-Cookie` that removes the session's cookie.
pub fn clear_cookie(state: &AppState) -> Result<HeaderValue, ApiError> {
    cookie(state, "", 0)
}

fn cookie(state: &AppState, value: &str, max_age: u64) -> Result<HeaderValue, ApiError> {
    let secure = if state.config.console_cookie_secure {
        "; Secure"
    } else {
        ""
    };
    let name = cookie_name(state);
    HeaderValue::try_from(format!(
        "{name}={value}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Lax{secure}"
    ))
    .map_err(ApiError::internal)
}

/// The session identifier in the request's cookie, if one is there.
pub fn session_cookie(headers: &HeaderMap, state: &AppState) -> Option<Option<SessionId>> {
    let name = cookie_name(state);
    let value = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find_map(|(n, v)| (n == name).then_some(v))?;
    Some(SessionId::parse(value))
}

fn console_session_refused() -> ApiError {
    ApiError::new(Code::Unauthorized)
        .with_message("Sign in to the console again.")
        .with_detail("reason", "console_session")
}

/// A non-safe request with a console cookie must come from this server's own
/// pages (12 §3 T9, defence in depth behind the CSRF token): `Sec-Fetch-Site`
/// `same-origin` (or `none`, a user's own navigation), or without it an
/// `Origin` whose host is the request's `Host`. A request with neither (not
/// from a browser) passes this check; the token still applies.
pub fn same_origin(headers: &HeaderMap) -> Result<(), ApiError> {
    let refused = || {
        ApiError::new(Code::Forbidden)
            .with_message("Console requests must come from the console's own origin.")
            .with_detail("reason", "origin")
    };
    if let Some(site) = headers.get(SEC_FETCH_SITE) {
        return match site.as_bytes() {
            b"same-origin" | b"none" => Ok(()),
            _ => Err(refused()),
        };
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Ok(());
    };
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    let origin_host = origin.to_str().ok().and_then(|o| {
        o.strip_prefix("https://")
            .or_else(|| o.strip_prefix("http://"))
    });
    match (origin_host, host) {
        (Some(o), Some(h)) if o.eq_ignore_ascii_case(h) => Ok(()),
        _ => Err(refused()),
    }
}

fn is_safe(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// The session from the cookie, checked for CSRF and origin on a non-safe
/// method before anything is looked up.
fn cookie_session(parts: &Parts, state: &AppState) -> Result<SessionId, ApiError> {
    let id = match session_cookie(&parts.headers, state) {
        Some(Some(id)) => id,
        Some(None) | None => return Err(console_session_refused()),
    };
    if !is_safe(&parts.method) {
        same_origin(&parts.headers)?;
        let sent = parts.headers.get(CSRF).map(HeaderValue::as_bytes);
        let valid = sent.is_some_and(|sent| bool::from(sent.ct_eq(id.csrf().as_bytes())));
        if !valid {
            return Err(ApiError::new(Code::Forbidden)
                .with_message("Send the session's CSRF token in `Cavea-Csrf-Token`.")
                .with_detail("reason", "csrf"));
        }
    }
    Ok(id)
}

/// Requires a console session: the `/console/api` routes of a signed-in user.
#[derive(Debug, Clone)]
pub struct ConsoleUser {
    pub id: SessionId,
    pub session: Found,
}

impl FromRequestParts<AppState> for ConsoleUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let id = cookie_session(parts, state)?;
        let session = sessions::find(&state.db, &id.hash(), None)
            .await?
            .ok_or_else(console_session_refused)?;
        Ok(Self { id, session })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]

    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (n, v) in pairs {
            h.append(
                HeaderName::try_from(*n).unwrap(),
                HeaderValue::try_from(*v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn only_the_servers_own_origin_passes() {
        for (pairs, ok) in [
            (vec![], true),
            (vec![("sec-fetch-site", "same-origin")], true),
            (vec![("sec-fetch-site", "none")], true),
            (vec![("sec-fetch-site", "same-site")], false),
            (vec![("sec-fetch-site", "cross-site")], false),
            // Sec-Fetch-Site decides when present.
            (
                vec![
                    ("sec-fetch-site", "cross-site"),
                    ("origin", "http://localhost:5173"),
                    ("host", "localhost:5173"),
                ],
                false,
            ),
            (
                vec![
                    ("origin", "http://localhost:5173"),
                    ("host", "localhost:5173"),
                ],
                true,
            ),
            (
                vec![
                    ("origin", "https://Seats.Example.com"),
                    ("host", "seats.example.com"),
                ],
                true,
            ),
            (
                vec![
                    ("origin", "https://evil.example"),
                    ("host", "seats.example.com"),
                ],
                false,
            ),
            (
                vec![("origin", "null"), ("host", "seats.example.com")],
                false,
            ),
            (vec![("origin", "https://seats.example.com")], false),
        ] {
            assert_eq!(same_origin(&headers(&pairs)).is_ok(), ok, "{pairs:?}");
        }
    }
}
