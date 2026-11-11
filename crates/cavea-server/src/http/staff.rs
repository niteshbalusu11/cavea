//! The console session as a credential (09 §2, §10; 08 §11): the cookie, the
//! CSRF token and the origin checks, and the two extractors that accept it.
//!
//! - [`Staff`]: the `/v1` routes a secret key may call. With
//!   `Authorization`, exactly the secret key as before; without it and with
//!   the console cookie, a signed-in member acting in the workspace named by
//!   `Cavea-Workspace`, whose role grants the route's permission (declared in
//!   the route table, `openapi::Needs::Staff`).
//! - [`Member`]: the `/console/api` routes of a workspace (its members): the
//!   console session only, with the same workspace and permission checks.
//! - [`ConsoleUser`]: the `/console/api` routes of a signed-in user.
//!
//! Every non-safe method authenticated by the cookie must carry the session's
//! CSRF token in `Cavea-Csrf-Token`, and must come from the server's own
//! origin (`Sec-Fetch-Site`, else `Origin` against `Host`). A request with an
//! API key is never read for a cookie, so neither check applies to it. No
//! route answers cross-origin with credentials: CORS is only on
//! `/v1/public/`, without `Access-Control-Allow-Credentials` (08 §11).

use axum::extract::FromRequestParts;
use axum::http::header::{self, HeaderName};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method};
use subtle::ConstantTimeEq as _;

use super::auth::{Auth, Credential, credential, unauthorized, wrong_kind};
use super::{ApiError, AppState, Code};
use crate::console::sessions::{self, Found};
use crate::console::{ABSOLUTE, Permission, SessionId};

/// The CSRF token's header.
pub const CSRF: HeaderName = HeaderName::from_static("cavea-csrf-token");
/// The workspace a console session acts on, on `/v1`.
pub const WORKSPACE: HeaderName = HeaderName::from_static("cavea-workspace");
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
        Some(None) => return Err(console_session_refused()),
        None => return Err(unauthorized("missing")),
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

/// The workspace a console request names: `ws_` and 32 lowercase hex digits.
fn named_workspace(headers: &HeaderMap) -> Result<String, ApiError> {
    let value = headers.get(WORKSPACE).ok_or_else(|| {
        ApiError::new(Code::InvalidRequest)
            .with_message("Name a workspace you are a member of in `Cavea-Workspace`.")
            .with_detail("reason", "workspace")
    })?;
    let value = value.to_str().map_err(|_| not_a_member())?;
    value
        .strip_prefix("ws_")
        .filter(|_| super::auth::has_hex_tail(value, "ws_", 32))
        .map(str::to_owned)
        .ok_or_else(not_a_member)
}

/// Requires the workspace's secret key, or a console session of a member
/// whose role grants the route's permission (09 §2). Every query is scoped by
/// `auth.workspace()`, which for a member is the workspace they named and
/// belong to.
#[derive(Debug, Clone)]
pub struct Staff(pub Auth);

impl FromRequestParts<AppState> for Staff {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let cookie = session_cookie(&parts.headers, state).is_some();
        if parts.headers.contains_key(header::AUTHORIZATION) || !cookie {
            return match credential(parts, state).await? {
                Credential::Key { auth, .. }
                    if auth.authority() == cavea_engine::Authority::Secret =>
                {
                    Ok(Self(auth))
                }
                _ => Err(wrong_kind(
                    "This endpoint requires a secret key or a console session.",
                )),
            };
        }
        member(parts, state).await.map(Self)
    }
}

/// Requires a console session of a member whose role grants the route's
/// permission, in the workspace the request names: the `/console/api` routes
/// of a workspace (its members), which no API key may call.
#[derive(Debug, Clone)]
pub struct Member(pub Auth);

impl FromRequestParts<AppState> for Member {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        member(parts, state).await.map(Self)
    }
}

/// The signed-in member acting in the workspace named by `Cavea-Workspace`:
/// the cookie (CSRF and origin checked on a non-safe method before anything
/// is read), then in one query the session and the user's role in that
/// workspace, read afresh on every request, so a role changed or a
/// membership removed applies to the next one. A workspace the user is not
/// in, existing or not, is `403` `workspace`; a role without the route's
/// permission `403` `role`.
async fn member(parts: &Parts, state: &AppState) -> Result<Auth, ApiError> {
    let permission = parts
        .extensions
        .get::<Permission>()
        .copied()
        .ok_or_else(|| ApiError::internal("the route declares no permission"))?;
    let id = cookie_session(parts, state).map_err(|e| {
        if e.code() == Code::Unauthorized {
            console_session_refused()
        } else {
            e
        }
    })?;
    let workspace = named_workspace(&parts.headers)?;
    let found = sessions::find(&state.db, &id.hash(), Some(&workspace))
        .await?
        .ok_or_else(console_session_refused)?;
    let Some(role) = found.role else {
        return Err(not_a_member());
    };
    let auth = Auth::member(workspace, found.user, role);
    cavea_telemetry::record_workspace(&auth.workspace().to_string());
    if !role.allows(permission) {
        return Err(ApiError::new(Code::Forbidden)
            .with_message("Your role in this workspace does not allow this.")
            .with_detail("reason", "role")
            .with_detail("permission", permission.as_str()));
    }
    Ok(auth)
}

fn not_a_member() -> ApiError {
    ApiError::new(Code::Forbidden)
        .with_message("Name a workspace you are a member of in `Cavea-Workspace`.")
        .with_detail("reason", "workspace")
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
        let id = cookie_session(parts, state).map_err(|e| {
            if e.code() == Code::Unauthorized {
                console_session_refused()
            } else {
                e
            }
        })?;
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
