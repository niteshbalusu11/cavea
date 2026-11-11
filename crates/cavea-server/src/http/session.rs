//! Session tokens (08 §11): a JWT the customer's backend signs for one buyer
//! and one event, which the buyer's browser sends in `Cavea-Session` on the
//! public hold-token, hold and release routes.
//!
//! The signing key is the SHA-256 of one of the workspace's secret keys, the
//! raw 32 bytes the server already stores, named by the header's `kid` (the
//! key's id). Only `HS256` is accepted: the code computes one HMAC-SHA-256 and
//! nothing else, so `none` and every other algorithm fail by construction. The
//! workspace is the publishable key's, never the token's.
//!
//! What a caller learns from a refusal: `missing`, `invalid` (anything wrong
//! before or in the signature: structure, base64, JSON, algorithm, an unknown,
//! revoked, foreign or publishable `kid`, a bad signature), and, only once the
//! signature has verified, `invalid` for claims of the wrong shape, `expired`
//! and `event`. Whoever can sign already knows which keys exist; nobody else
//! learns whether a `kid` exists.
//!
//! The `sub` is the customer's identifier for a buyer: personal data. It is
//! never logged or stored; `holds.session_sub` keeps its SHA-256 until the
//! hold is deleted (on expiry, or when its last objects are booked).

use axum::http::HeaderMap;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cavea_engine::{Clock as _, EventId, Timestamp, TokenHash};
use hmac::{Hmac, KeyInit as _, Mac as _};
use serde::Deserialize;
use serde::de::IgnoredAny;
use sha2::Sha256;
use subtle::ConstantTimeEq as _;
use tracing::Instrument as _;

use super::auth::{Auth, KeyId, push_hex, sha256};
use super::seats::Target;
use super::{ApiError, AppState, Code};

/// The request header that carries a session token on the public routes.
pub(super) const HEADER: &str = "cavea-session";
/// The longest session token: the same as SUBSCRIBE's field (08 §8.2).
pub(super) const MAX_TOKEN_BYTES: usize = cavea_format::wire::MAX_SESSION_TOKEN_BYTES;
/// How long after `exp` a token is still accepted, for clock skew between the
/// customer's backend and this server.
pub(super) const LEEWAY_MS: u64 = 30_000;
/// The longest `sub`, and the longest channel key, in characters.
const MAX_CLAIM_CHARS: usize = 128;

/// Why a session token was refused: `details.session` of the `401`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal {
    /// The event requires a session token and none was sent.
    Missing,
    Invalid,
    /// Correctly signed, past `exp` and the leeway.
    Expired,
    /// Correctly signed, for another event.
    Event,
}

impl Refusal {
    pub(super) fn error(self) -> ApiError {
        let (session, message) = match self {
            Self::Missing => (
                "missing",
                "This event requires a session token in `Cavea-Session`.",
            ),
            Self::Invalid => (
                "invalid",
                "The session token is malformed, or not signed (HS256) with the SHA-256 of a \
                 live secret key of this workspace whose id is its `kid`.",
            ),
            Self::Expired => ("expired", "The session token has expired."),
            Self::Event => ("event", "The session token is for another event."),
        };
        ApiError::new(Code::Unauthorized)
            .with_message(message)
            .with_detail("reason", "session_token")
            .with_detail("session", session)
    }
}

/// What a verified session token says about its buyer.
pub(super) struct Session {
    /// The SHA-256 of `sub`, as hex: what `holds.session_sub` stores.
    pub(super) sub: String,
    /// The most objects a hold token created with this session may hold.
    pub(super) max_objects: Option<u32>,
}

/// A session token split and decoded, its signature not yet checked.
struct Unverified<'a> {
    kid: KeyId,
    /// `header.payload`, the bytes the signature covers.
    signed: &'a str,
    payload: Vec<u8>,
    signature: Vec<u8>,
}

#[derive(Deserialize)]
struct Header {
    alg: Option<String>,
    kid: Option<String>,
    /// Extensions the token says must be understood: none are.
    crit: Option<IgnoredAny>,
}

/// Unknown claims are ignored; a repeated one is refused by the parser.
#[derive(Deserialize)]
struct Claims {
    sub: String,
    /// Seconds since the Unix epoch, a whole number.
    exp: u64,
    event: String,
    #[serde(default)]
    max_objects: Option<u32>,
    /// Carried for channels (02 §7); shape-checked, no effect yet.
    #[serde(default)]
    channels: Option<Vec<String>>,
}

fn parse(token: &str) -> Result<Unverified<'_>, Refusal> {
    use Refusal::Invalid;
    if token.len() > MAX_TOKEN_BYTES {
        return Err(Invalid);
    }
    let (signed, signature) = token.rsplit_once('.').ok_or(Invalid)?;
    let (header, payload) = signed.split_once('.').ok_or(Invalid)?;
    if payload.contains('.') {
        return Err(Invalid);
    }
    let decode = |part: &str| URL_SAFE_NO_PAD.decode(part).map_err(|_| Invalid);
    let header: Header = serde_json::from_slice(&decode(header)?).map_err(|_| Invalid)?;
    if header.alg.as_deref() != Some("HS256") || header.crit.is_some() {
        return Err(Invalid);
    }
    Ok(Unverified {
        kid: header
            .kid
            .as_deref()
            .and_then(KeyId::parse)
            .ok_or(Invalid)?,
        signed,
        payload: decode(payload)?,
        signature: decode(signature)?,
    })
}

/// Check the signature with `key` (the stored SHA-256 of the secret key named
/// by `kid`, `None` if there is no such live key), then the claims.
fn check(
    token: &Unverified<'_>,
    key: Option<&[u8; 32]>,
    now: Timestamp,
    event: &str,
) -> Result<Session, Refusal> {
    use Refusal::Invalid;
    // The same work whether or not the key exists.
    let mut mac = Hmac::<Sha256>::new_from_slice(key.unwrap_or(&[0; 32])).map_err(|_| Invalid)?;
    mac.update(token.signed.as_bytes());
    let verified: bool = mac
        .finalize()
        .into_bytes()
        .as_slice()
        .ct_eq(&token.signature)
        .into();
    if !verified || key.is_none() {
        return Err(Invalid);
    }
    let claims: Claims = serde_json::from_slice(&token.payload).map_err(|_| Invalid)?;
    let short = |s: &str| !s.is_empty() && s.chars().count() <= MAX_CLAIM_CHARS;
    if !short(&claims.sub)
        || claims.max_objects == Some(0)
        || !claims.channels.iter().flatten().all(|c| short(c))
    {
        return Err(Invalid);
    }
    if now.0 >= claims.exp.saturating_mul(1000).saturating_add(LEEWAY_MS) {
        return Err(Refusal::Expired);
    }
    if claims.event != event {
        return Err(Refusal::Event);
    }
    let mut sub = String::with_capacity(64);
    push_hex(&mut sub, &sha256(&claims.sub));
    Ok(Session {
        sub,
        max_objects: claims.max_objects,
    })
}

/// A live secret key of the workspace, by id.
const KEY: &str = "SELECT hash FROM api_keys
 WHERE id = $1::uuid AND workspace_id = $2::uuid AND kind = 'secret' AND revoked_at IS NULL";

/// The session of a public request on `target`: verified if `Cavea-Session` is
/// sent, whether or not the event requires one; `None` if it is not sent and
/// the event does not require one.
pub(super) async fn verify(
    state: &AppState,
    auth: &Auth,
    headers: &HeaderMap,
    target: &Target,
) -> Result<Option<Session>, ApiError> {
    let Some(value) = headers.get(HEADER) else {
        return if target.settings.require_session() {
            Err(Refusal::Missing.error())
        } else {
            Ok(None)
        };
    };
    let token = value.to_str().map_err(|_| Refusal::Invalid.error())?;
    let token = parse(token).map_err(Refusal::error)?;
    let key: Option<Vec<u8>> = sqlx::query_scalar(KEY)
        .bind(token.kid.uuid())
        .bind(auth.workspace().uuid())
        .fetch_optional(&state.db)
        .instrument(cavea_telemetry::db_span("session_key"))
        .await?;
    let key = key.and_then(|k| <[u8; 32]>::try_from(k).ok());
    check(&token, key.as_ref(), state.seats.clock.now(), &target.key)
        .map(Some)
        .map_err(Refusal::error)
}

fn event_hex(id: EventId) -> String {
    format!("{:032x}", id.0)
}

/// Live hold tokens bound to one `sub` on one event, by the server clock.
const LIVE: &str = "SELECT count(*) FROM holds
 WHERE event_id = $1::uuid AND session_sub = $3
   AND expires_at > to_timestamp(0) + $4 * interval '1 millisecond'
   AND EXISTS (SELECT FROM events WHERE id = $1::uuid AND workspace_id = $2::uuid)";

/// Serialises binding per (event, `sub`); a collision only serialises two
/// unrelated buyers for a moment.
const LOCK: &str = "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))";

/// Bind a new hold to a `sub` if the `sub` has fewer than `$6` live tokens.
const BIND: &str = "UPDATE holds SET session_sub = $3
 WHERE event_id = $1::uuid AND token_hash = $5
   AND EXISTS (SELECT FROM events WHERE id = $1::uuid AND workspace_id = $2::uuid)
   AND (SELECT count(*) FROM holds
         WHERE event_id = $1::uuid AND session_sub = $3
           AND expires_at > to_timestamp(0) + $4 * interval '1 millisecond') < $6";

fn over_cap(cap: u32) -> ApiError {
    ApiError::new(Code::HoldLimitExceeded)
        .with_message(
            "This session already has as many live hold tokens on the event as it may; reuse \
             one, or wait for one to expire.",
        )
        .with_detail("reason", "tokens_per_sub")
        .with_detail("limit", cap)
}

/// Refuse early when the `sub` already has the event's `max_tokens_per_sub`
/// live tokens, so that a buyer at the cap does not make the actor create a
/// hold for nothing. [`bind`] decides; this only saves the work.
pub(super) async fn check_cap(
    state: &AppState,
    auth: &Auth,
    target: &Target,
    session: &Session,
) -> Result<(), ApiError> {
    let cap = target.settings.max_tokens_per_sub();
    let live: i64 = sqlx::query_scalar(LIVE)
        .bind(event_hex(target.id))
        .bind(auth.workspace().uuid())
        .bind(&session.sub)
        .bind(i64::try_from(state.seats.clock.now().0).unwrap_or(i64::MAX))
        .fetch_one(&state.db)
        .instrument(cavea_telemetry::db_span("session_live_tokens"))
        .await?;
    if live >= i64::from(cap) {
        return Err(over_cap(cap));
    }
    Ok(())
}

/// Bind the hold `token` just created on `event` to the session's `sub`,
/// under the cap. The engine's `CreateHold` has committed the hold; its token
/// has not been returned yet, so nobody can use it unbound. Two creations for
/// one `sub` take the same lock in turn, and the second counts the first's
/// binding. A hold over the cap stays unbound and is never revealed: it holds
/// nothing and expires.
pub(super) async fn bind(
    state: &AppState,
    auth: &Auth,
    target: &Target,
    token: &TokenHash,
    session: &Session,
) -> Result<(), ApiError> {
    let cap = target.settings.max_tokens_per_sub();
    let event = event_hex(target.id);
    let now = i64::try_from(state.seats.clock.now().0).unwrap_or(i64::MAX);
    let bind = async {
        let mut tx = state.db.begin().await?;
        sqlx::query(LOCK)
            .bind(format!("{event}:{}", session.sub))
            .execute(&mut *tx)
            .await?;
        // A statement after the lock sees every binding committed before it.
        let bound = sqlx::query(BIND)
            .bind(&event)
            .bind(auth.workspace().uuid())
            .bind(&session.sub)
            .bind(now)
            .bind(token.0.as_slice())
            .bind(i64::from(cap))
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok::<_, ApiError>(bound)
    };
    let bound = bind
        .instrument(cavea_telemetry::db_span("session_bind"))
        .await?;
    if bound == 0 {
        return Err(over_cap(cap));
    }
    Ok(())
}

/// The `sub` a hold token was created for, if any.
const BOUND: &str = "SELECT h.session_sub FROM holds h JOIN events e ON e.id = h.event_id
 WHERE h.event_id = $1::uuid AND e.workspace_id = $2::uuid AND h.token_hash = $3";

/// A hold token created with a session is used only with a session of the
/// same `sub` (12 §3 T2). A token created without one (by the backend, or on
/// an event without sessions) is not bound. The binding is written before the
/// token is revealed and never changes, so reading it before the command is
/// not a race; a token that does not exist is left to the engine to refuse.
pub(super) async fn check_binding(
    state: &AppState,
    auth: &Auth,
    target: &Target,
    token: &TokenHash,
    session: Option<&Session>,
) -> Result<(), ApiError> {
    let bound: Option<Option<String>> = sqlx::query_scalar(BOUND)
        .bind(event_hex(target.id))
        .bind(auth.workspace().uuid())
        .bind(token.0.as_slice())
        .fetch_optional(&state.db)
        .instrument(cavea_telemetry::db_span("session_binding"))
        .await?;
    match bound {
        Some(Some(sub)) if session.is_none_or(|s| s.sub != sub) => {
            Err(ApiError::new(Code::Forbidden)
                .with_message("The hold token was created with another session.")
                .with_detail("reason", "session"))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;
