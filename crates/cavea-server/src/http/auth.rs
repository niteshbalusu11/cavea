//! Credentials and tenancy (08 §11): API keys, the admin key, and the
//! extractors a handler declares to say which credential it needs.
//!
//! A handler that takes [`super::staff::Staff`] or [`PublishableKey`] gets an
//! [`Auth`], and [`Auth::workspace`] is the only way to obtain a
//! [`WorkspaceId`]: every query is scoped by the workspace of the credential
//! (a key's own, or one a console session's user is a member of), never of
//! the request alone.

use std::convert::Infallible;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{Method, header};
use cavea_engine::{Authority, Clock as _};
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;
use tracing::Instrument as _;

use super::{ApiError, AppState, Code};
use crate::console::Role;
use crate::limiter::{Budget, Limited};
use crate::net::{client_addr, group};

/// Random bytes in a new API key: 256 bits from the operating system.
const KEY_BYTES: usize = 32;
/// Hex digits of the key kept in `api_keys.prefix` for display.
const PREFIX_HEX: usize = 8;
/// The shortest admin key accepted at start-up.
const ADMIN_KEY_MIN: usize = 32;
/// The longest bearer token read from `Authorization`.
const TOKEN_MAX: usize = 512;

/// Whether a key is for a backend (`sk_`) or a browser or app (`pk_`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyKind {
    Secret,
    Publishable,
}

impl KeyKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Secret => "secret",
            Self::Publishable => "publishable",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::Secret => "sk_",
            Self::Publishable => "pk_",
        }
    }

    pub(super) fn from_db(kind: &str) -> Result<Self, ApiError> {
        match kind {
            "secret" => Ok(Self::Secret),
            "publishable" => Ok(Self::Publishable),
            _ => Err(ApiError::internal("api_keys.kind holds an unknown kind")),
        }
    }
}

/// A newly generated API key. `key` is returned to the caller once and never
/// stored or logged; only `hash` and `prefix` are kept.
pub struct NewKey {
    pub key: String,
    pub prefix: String,
    pub hash: [u8; 32],
}

impl NewKey {
    /// `sk_` or `pk_` and 64 lowercase hex digits of the operating system's
    /// secure random source: URL-safe and selectable with a double click.
    pub fn generate(kind: KeyKind) -> Result<Self, ApiError> {
        let mut bytes = [0; KEY_BYTES];
        getrandom::fill(&mut bytes).map_err(|e| ApiError::internal(e.to_string()))?;
        let mut key = String::with_capacity(3 + 2 * KEY_BYTES);
        key.push_str(kind.prefix());
        push_hex(&mut key, &bytes);
        Ok(Self {
            prefix: key[..3 + PREFIX_HEX].to_owned(),
            hash: sha256(&key),
            key,
        })
    }
}

impl fmt::Debug for NewKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NewKey({}…)", self.prefix)
    }
}

pub(super) fn sha256(s: &str) -> [u8; 32] {
    Sha256::digest(s.as_bytes()).into()
}

pub(super) fn push_hex(out: &mut String, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for b in bytes {
        out.push(char::from(HEX[usize::from(b >> 4)]));
        out.push(char::from(HEX[usize::from(b & 0xf)]));
    }
}

/// Whether `s` is `prefix` followed by `len` lowercase hex digits.
pub(super) fn has_hex_tail(s: &str, prefix: &str, len: usize) -> bool {
    s.strip_prefix(prefix).is_some_and(|t| {
        t.len() == len && t.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// `CAVEA_ADMIN_KEY`: the operator's key for the workspace endpoints. Only its
/// SHA-256 is held, and it is never printed.
#[derive(Clone)]
pub struct AdminKey([u8; 32]);

impl AdminKey {
    /// Constant-time by construction: both sides are 32-byte digests compared
    /// with `subtle`, so the time taken depends on neither the admin key nor
    /// how much of it the presented token shares.
    fn matches(&self, token: &str) -> bool {
        sha256(token).ct_eq(&self.0).into()
    }
}

impl FromStr for AdminKey {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() < ADMIN_KEY_MIN || s.len() > TOKEN_MAX {
            return Err("must be 32 to 512 characters");
        }
        if !s.bytes().all(|b| b.is_ascii_graphic()) {
            return Err("must be printable ASCII without spaces");
        }
        Ok(Self(sha256(s)))
    }
}

impl fmt::Debug for AdminKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AdminKey(..)")
    }
}

/// A workspace's id: `ws_` and 32 hex digits of its UUID. Obtainable only
/// from an authenticated [`Auth`].
#[derive(Clone, PartialEq, Eq)]
pub struct WorkspaceId(String);

impl WorkspaceId {
    /// The UUID as 32 hex digits: bind it as `$n::uuid`.
    #[must_use]
    pub fn uuid(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ws_{}", self.0)
    }
}

impl fmt::Debug for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// An API key's id: `key_` and 32 hex digits of its UUID. Not a secret.
#[derive(Clone, PartialEq, Eq)]
pub struct KeyId(String);

impl KeyId {
    /// Parse a key id from a request; `None` if it cannot be one.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        has_hex_tail(s, "key_", 32).then(|| Self(s[4..].to_owned()))
    }

    /// The UUID as 32 hex digits: bind it as `$n::uuid`.
    #[must_use]
    pub fn uuid(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "key_{}", self.0)
    }
}

impl fmt::Debug for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// An authenticated caller in one workspace: what a handler acts as.
#[derive(Debug, Clone)]
pub struct Auth {
    workspace: WorkspaceId,
    principal: Principal,
}

#[derive(Debug, Clone)]
enum Principal {
    Key {
        key: KeyId,
        kind: KeyKind,
    },
    /// A signed-in console user (32 hex digits of the UUID) and their role in
    /// the workspace, already checked against the route's permission.
    Member {
        user: String,
        role: Role,
    },
}

impl Auth {
    /// A console user acting in `workspace` (a UUID as 32 hex digits) with
    /// `role`: made only once the membership was read for the session.
    pub(super) fn member(workspace: String, user: String, role: Role) -> Self {
        Self {
            workspace: WorkspaceId(workspace),
            principal: Principal::Member { user, role },
        }
    }

    /// The workspace every query of this request is scoped to.
    #[must_use]
    pub fn workspace(&self) -> &WorkspaceId {
        &self.workspace
    }

    /// The API key, unless a console user is acting.
    #[must_use]
    pub fn key_id(&self) -> Option<&KeyId> {
        match &self.principal {
            Principal::Key { key, .. } => Some(key),
            Principal::Member { .. } => None,
        }
    }

    /// The console user's UUID as 32 hex digits (bind as `$n::uuid`), when a
    /// console user is acting.
    #[must_use]
    pub fn user(&self) -> Option<&str> {
        match &self.principal {
            Principal::Key { .. } => None,
            Principal::Member { user, .. } => Some(user),
        }
    }

    fn kind(&self) -> Option<KeyKind> {
        match &self.principal {
            Principal::Key { kind, .. } => Some(*kind),
            Principal::Member { .. } => None,
        }
    }

    /// The engine authority (`cavea_engine::Request::authority`): a key's
    /// kind; a console member acts with the secret key's authority, their
    /// role having been checked against the route's permission (09 §2).
    #[must_use]
    pub fn authority(&self) -> Authority {
        match &self.principal {
            Principal::Key {
                kind: KeyKind::Publishable,
                ..
            } => Authority::Publishable,
            Principal::Key { .. } | Principal::Member { .. } => Authority::Secret,
        }
    }

    /// The identity written to the event log (`cavea_engine::Request::actor`)
    /// and to logs as `by`: the key id (`key_…`), or the console user's id
    /// (`usr_…`), never an email.
    #[must_use]
    pub fn actor(&self) -> Arc<str> {
        match &self.principal {
            Principal::Key { key, .. } => key.to_string().into(),
            Principal::Member { user, .. } => format!("usr_{user}").into(),
        }
    }

    /// The console member's role, if a console user is acting.
    #[must_use]
    pub fn role(&self) -> Option<Role> {
        match &self.principal {
            Principal::Key { .. } => None,
            Principal::Member { role, .. } => Some(*role),
        }
    }
}

/// Requires a publishable key (`pk_…`), from an allowed origin if the key has
/// an allow-list.
#[derive(Debug, Clone)]
pub struct PublishableKey(pub Auth);

/// Requires a publishable key in the `key` query parameter, from an allowed
/// origin if the key has an allow-list: for the realtime routes, which
/// `WebSocket` and `EventSource` reach without custom headers and a CDN caches
/// by URL (08 §8.2, §8.5). A publishable key is not a secret; nothing else
/// travels in a URL.
#[derive(Debug, Clone)]
pub struct QueryKey(pub Auth);

/// Requires the admin key (`CAVEA_ADMIN_KEY`).
#[derive(Debug, Clone, Copy)]
pub struct AdminKeyAuth;

/// What the bearer token turned out to be.
pub(super) enum Credential {
    Admin,
    Key {
        auth: Auth,
        allowed_origins: Option<Vec<String>>,
    },
}

impl FromRequestParts<AppState> for PublishableKey {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        match credential(parts, state).await? {
            Credential::Key {
                auth,
                allowed_origins,
            } if auth.kind() == Some(KeyKind::Publishable) => {
                check_origin(parts, allowed_origins.as_deref())?;
                limit(parts, state, &auth)?;
                Ok(Self(auth))
            }
            _ => Err(wrong_kind("This endpoint requires a publishable key.")),
        }
    }
}

impl FromRequestParts<AppState> for QueryKey {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        match resolve(query_key(parts)?, state).await? {
            Credential::Key {
                auth,
                allowed_origins,
            } if auth.kind() == Some(KeyKind::Publishable) => {
                check_origin(parts, allowed_origins.as_deref())?;
                limit(parts, state, &auth)?;
                Ok(Self(auth))
            }
            _ => Err(wrong_kind("This endpoint requires a publishable key.")),
        }
    }
}

impl FromRequestParts<AppState> for AdminKeyAuth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        match credential(parts, state).await? {
            Credential::Admin => Ok(Self),
            Credential::Key { .. } => Err(wrong_kind("This endpoint requires the admin key.")),
        }
    }
}

/// The client's address as it is counted: [`client_addr`] behind
/// `CAVEA_TRUSTED_PROXIES`, [`group`]ed. `None` without a socket (tests).
#[derive(Debug, Clone, Copy)]
pub struct ClientIp(pub Option<IpAddr>);

impl ClientIp {
    pub(super) fn of(parts: &Parts, state: &AppState) -> Self {
        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>();
        Self(peer.map(|ConnectInfo(peer)| {
            group(client_addr(
                peer.ip(),
                &parts.headers,
                &state.config.trusted_proxies,
            ))
        }))
    }
}

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Infallible> {
        Ok(Self::of(parts, state))
    }
}

/// The rate limit of a publishable key from the client's address (08 §12):
/// `POST`s (hold-token creation, hold, release) draw on the holds budget,
/// everything else, realtime connections included, on the reads budget.
fn limit(parts: &Parts, state: &AppState, auth: &Auth) -> Result<(), ApiError> {
    let budget = if parts.method == Method::POST {
        Budget::Holds
    } else {
        Budget::Reads
    };
    let key = auth
        .key_id()
        .and_then(|k| u128::from_str_radix(k.uuid(), 16).ok())
        .unwrap_or_default();
    let ClientIp(addr) = ClientIp::of(parts, state);
    let now = state.seats.clock.now().0;
    state
        .limiter
        .check(budget, key, addr, now)
        .map_err(|Limited(seconds)| {
            ApiError::new(Code::RateLimited)
                .with_message("Too many requests from this client; retry after the given delay.")
                .with_retry_after(seconds)
        })
}

pub(super) fn wrong_kind(message: &'static str) -> ApiError {
    ApiError::new(Code::Forbidden)
        .with_message(message)
        .with_detail("reason", "key_kind")
}

pub(super) fn unauthorized(reason: &'static str) -> ApiError {
    ApiError::new(Code::Unauthorized).with_detail("reason", reason)
}

/// Resolve the bearer token: `401` unless it is the admin key or an unrevoked
/// API key.
pub(super) async fn credential(parts: &Parts, state: &AppState) -> Result<Credential, ApiError> {
    resolve(bearer(parts)?, state).await
}

async fn resolve(token: &str, state: &AppState) -> Result<Credential, ApiError> {
    if let Some(admin) = &state.config.admin_key
        && admin.matches(token)
    {
        return Ok(Credential::Admin);
    }
    if !has_hex_tail(token, "sk_", 2 * KEY_BYTES) && !has_hex_tail(token, "pk_", 2 * KEY_BYTES) {
        return Err(unauthorized("invalid"));
    }
    let row: Option<(String, String, String, Option<Vec<String>>)> = sqlx::query_as(LOOKUP)
        .bind(sha256(token).as_slice())
        .fetch_optional(&state.db)
        .instrument(cavea_telemetry::db_span("authenticate"))
        .await?;
    let Some((key, workspace, kind, allowed_origins)) = row else {
        return Err(unauthorized("invalid"));
    };
    let auth = Auth {
        workspace: WorkspaceId(workspace),
        principal: Principal::Key {
            key: KeyId(key),
            kind: KeyKind::from_db(&kind)?,
        },
    };
    cavea_telemetry::record_workspace(&auth.workspace.to_string());
    Ok(Credential::Key {
        auth,
        allowed_origins,
    })
}

/// Find an unrevoked key by the SHA-256 of the presented token, and update its
/// `last_used_at` if it is unset or more than an hour old (08 §11), so that
/// a key in constant use costs one write an hour, not one per request.
const LOOKUP: &str = "
WITH k AS (
  SELECT id, workspace_id, kind, allowed_origins FROM api_keys
   WHERE hash = $1 AND revoked_at IS NULL
), touch AS (
  UPDATE api_keys SET last_used_at = now() FROM k
   WHERE api_keys.id = k.id
     AND (api_keys.last_used_at IS NULL OR api_keys.last_used_at < now() - interval '1 hour')
)
SELECT replace(id::text, '-', ''), replace(workspace_id::text, '-', ''), kind, allowed_origins
  FROM k";

/// The token of `Authorization: Bearer <token>`.
fn bearer(parts: &Parts) -> Result<&str, ApiError> {
    let value = parts
        .headers
        .get(header::AUTHORIZATION)
        .ok_or_else(|| unauthorized("missing"))?;
    let value = value.to_str().map_err(|_| unauthorized("malformed"))?;
    let (scheme, token) = value
        .split_once(' ')
        .ok_or_else(|| unauthorized("malformed"))?;
    if !scheme.eq_ignore_ascii_case("bearer") || !well_formed(token) {
        return Err(unauthorized("malformed"));
    }
    Ok(token)
}

/// The value of the one `key` query parameter. Keys are hex, so it is taken
/// as it stands: anything percent-encoded is malformed.
fn query_key(parts: &Parts) -> Result<&str, ApiError> {
    let mut keys = parts
        .uri
        .query()
        .unwrap_or_default()
        .split('&')
        .filter_map(|pair| pair.strip_prefix("key="));
    let token = keys.next().ok_or_else(|| unauthorized("missing"))?;
    if keys.next().is_some() || !well_formed(token) || token.contains('%') {
        return Err(unauthorized("malformed"));
    }
    Ok(token)
}

fn well_formed(token: &str) -> bool {
    !token.is_empty() && token.len() <= TOKEN_MAX && token.bytes().all(|b| b.is_ascii_graphic())
}

/// The origin allow-list of a publishable key (12 §3 T3). A request without
/// `Origin` passes: native apps send none, and any non-browser client could
/// send whatever it liked. A request whose `Origin` is not on the list,
/// `null` included, is `403`.
fn check_origin(parts: &Parts, allowed: Option<&[String]>) -> Result<(), ApiError> {
    let (Some(allowed), Some(origin)) = (allowed, parts.headers.get(header::ORIGIN)) else {
        return Ok(());
    };
    let permitted = origin
        .to_str()
        .is_ok_and(|o| allowed.iter().any(|a| a.eq_ignore_ascii_case(o)));
    if permitted {
        Ok(())
    } else {
        Err(ApiError::new(Code::Forbidden)
            .with_message("The key may not be used from this origin.")
            .with_detail("reason", "origin"))
    }
}

/// Check and normalise an origin allow-list entry: `http` or `https`, a host
/// and an optional port, as a browser sends it in `Origin`. Lowercased.
pub fn normalise_origin(origin: &str) -> Option<String> {
    let lower = origin.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))?;
    let valid = origin.len() <= 255
        && !rest.is_empty()
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'));
    valid.then_some(lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_key_start_up_rule() {
        assert!("".parse::<AdminKey>().is_err());
        assert!("a".repeat(31).parse::<AdminKey>().is_err());
        assert!(format!("{} x", "a".repeat(40)).parse::<AdminKey>().is_err());
        assert!("a".repeat(513).parse::<AdminKey>().is_err());
        let key: AdminKey = "fake-admin-key-0123456789abcdef!".parse().unwrap();
        assert!(key.matches("fake-admin-key-0123456789abcdef!"));
        assert!(!key.matches("fake-admin-key-0123456789abcdef"));
        assert_eq!(format!("{key:?}"), "AdminKey(..)");
    }
}
