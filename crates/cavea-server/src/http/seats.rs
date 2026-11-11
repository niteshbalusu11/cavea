//! Hold tokens and seat operations (08 §6.2, §7) with secret keys. Each
//! handler maps its request onto an engine command, sends it through the
//! registry, and renders the reply with each object's `label` and `oid`.
//!
//! A hold token is a bearer secret: it is minted here, returned once, and only
//! its SHA-256 is stored or given to the engine. Routes that carry it in the
//! path log only their route template.

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use cavea_engine::{
    ChartVersionId, Command, EventId, Failure, Idx, ObjectFailure, ObjectRef, Outcome, Reply,
    Request, StoreError, Timestamp, TokenHash,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::Instrument as _;

use super::auth::{Auth, SecretKey, has_hex_tail, push_hex, sha256};
use super::events::{Settings, archived};
use super::extract::{Json, Path, check_object_count, invalid_field};
use super::{ApiError, AppState, Code};
use crate::seats::{Chart, event_id};
use cavea_core::limits::MAX_OBJECTS;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/hold-tokens", post(create_token))
        .route(
            "/v1/hold-tokens/{token}",
            get(read_token).delete(release_token),
        )
        .route("/v1/hold-tokens/{token}/extend", post(extend_token))
        .route("/v1/events/{event}/hold", post(hold))
        .route("/v1/events/{event}/release", post(release))
}

/// Random bytes in a hold token: 256 bits from the operating system.
const TOKEN_BYTES: usize = 32;

fn token_hash(token: &str) -> TokenHash {
    TokenHash(sha256(token))
}

/// RFC 3339 UTC to the second, from Unix milliseconds (civil-from-days, H.
/// Hinnant).
fn rfc3339(t: Timestamp) -> String {
    let secs = t.0 / 1000;
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let z = days.cast_signed() + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

// ---- the event a request acts on ----

/// An event, as a seat request needs it.
pub(super) struct Target {
    id: EventId,
    key: String,
    version: ChartVersionId,
    /// The pinned version's public hash.
    hash: String,
    archived: bool,
    settings: Settings,
}

type TargetRow = (String, String, String, String, bool, String);

macro_rules! target_query {
    ($condition:literal) => {
        concat!(
            "SELECT replace(e.id::text, '-', ''), e.key, replace(e.chart_version_id::text, '-', ''),
       encode(v.hash, 'hex'), e.archived_at IS NOT NULL, e.settings::text
  FROM events e JOIN chart_versions v ON v.id = e.chart_version_id
 WHERE e.workspace_id = $2::uuid AND ",
            $condition
        )
    };
}

/// How a request names its event.
enum Find<'a> {
    Key(&'a str),
    /// The event of a hold token, by the token's hash. The lookup is one index
    /// probe on the unique `holds.token_hash`, scoped to the key's workspace:
    /// another workspace's token is as unknown as a token that never existed.
    Token(&'a TokenHash),
}

async fn target(state: &AppState, auth: &Auth, find: Find<'_>) -> Result<Target, ApiError> {
    let query = match &find {
        Find::Key(key) => sqlx::query_as(target_query!("e.key = $1")).bind(*key),
        Find::Token(hash) => sqlx::query_as(target_query!(
            "e.id = (SELECT event_id FROM holds WHERE token_hash = $1)"
        ))
        .bind(hash.0.as_slice()),
    };
    let row: Option<TargetRow> = query
        .bind(auth.workspace().uuid())
        .fetch_optional(&state.db)
        .instrument(cavea_telemetry::db_span("find_event"))
        .await?;
    let Some((id, key, version, hash, archived, settings)) = row else {
        return Err(match find {
            Find::Token(_) => no_token(),
            Find::Key(_) => ApiError::new(Code::NotFound).with_message("No such event."),
        });
    };
    cavea_telemetry::record_event(&key);
    let unreadable = || ApiError::internal("unreadable event id");
    Ok(Target {
        id: event_id(&id).ok_or_else(unreadable)?,
        version: ChartVersionId(event_id(&version).ok_or_else(unreadable)?.0),
        key,
        hash,
        archived,
        settings: Settings::parse(&settings)?,
    })
}

/// The chart of a version, for labels.
pub(super) async fn chart(
    state: &AppState,
    version: ChartVersionId,
) -> Result<Arc<Chart>, ApiError> {
    state.seats.charts.get(version).await.map_err(|e| match e {
        StoreError::Unavailable(_) => ApiError::new(Code::Unavailable),
        e => ApiError::internal(e),
    })
}

/// A command's result, ready to render.
struct Done {
    target: Target,
    chart: Arc<Chart>,
    reply: Reply,
}

/// Find the event, refuse an archived one, build the command and send it.
async fn submit(
    state: &AppState,
    auth: &Auth,
    find: Find<'_>,
    build: impl FnOnce(&Target) -> Result<Command, ApiError>,
) -> Result<Done, ApiError> {
    let target = target(state, auth, find).await?;
    if target.archived {
        return Err(archived());
    }
    let request = Request {
        actor: auth.actor(),
        authority: auth.authority(),
        command: build(&target)?,
        idempotency: None,
    };
    let reply = state.seats.registry.send(target.id, request).await;
    // The event could not be had (its chart included): nothing to render.
    if let Err(f @ (Failure::Unavailable | Failure::Overloaded)) = reply {
        return Err(f.into());
    }
    let chart = chart(state, target.version).await?;
    Ok(Done {
        target,
        chart,
        reply,
    })
}

// ---- rendering ----

fn objects_json(chart: &Chart, idxs: &[Idx]) -> Result<Value, ApiError> {
    idxs.iter()
        .map(|&idx| match (chart.label(idx), chart.view.oid(idx)) {
            (Some(label), Some(oid)) => Ok(json!({ "label": label, "oid": oid })),
            _ => Err(ApiError::internal("reply names an object not in the chart")),
        })
        .collect()
}

/// A refusal, with failing objects named by `label` and `oid` (08 §6.3).
fn failure(chart: &Chart, failure: Failure) -> ApiError {
    let objects: Option<Vec<ObjectFailure>> = match &failure {
        Failure::ObjectNotFree(o)
        | Failure::ObjectNotHeldByToken(o)
        | Failure::ObjectStatusConflict(o) => Some(o.clone()),
        _ => None,
    };
    let error = ApiError::from(failure);
    let Some(objects) = objects else {
        return error;
    };
    let listed: Vec<Value> = objects
        .iter()
        .map(|o| {
            json!({
                "label": chart.label(o.idx),
                "oid": chart.view.oid(o.idx),
                "reason": o.reason.code(),
            })
        })
        .collect();
    error.with_detail("objects", listed)
}

/// `200` with the outcome, or the refusal.
fn respond(done: Done) -> Result<Response, ApiError> {
    let chart = &done.chart;
    let body = match done.reply.map_err(|f| failure(chart, f))? {
        Outcome::Held {
            seq,
            expires_at,
            objects,
        } => json!({
            "seq": seq,
            "expires_at": rfc3339(expires_at),
            "objects": objects_json(chart, &objects)?,
        }),
        Outcome::Released { seq, objects } => {
            json!({ "seq": seq, "objects": objects_json(chart, &objects)? })
        }
        Outcome::Extended { seq, expires_at } => {
            json!({ "seq": seq, "expires_at": rfc3339(expires_at) })
        }
        _ => return Err(ApiError::internal("unexpected reply to a seat command")),
    };
    Ok(axum::Json(body).into_response())
}

// ---- objects in a request ----

/// Labels, or `idx` in a named chart version (08 §6.1).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Objects {
    Labels(Vec<String>),
    Indices(Indices),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Indices {
    /// The public hash of the event's chart version.
    version: String,
    idx: Vec<Idx>,
}

impl Objects {
    fn is_empty(&self) -> bool {
        match self {
            Self::Labels(labels) => labels.is_empty(),
            Self::Indices(i) => i.idx.is_empty(),
        }
    }

    /// Engine references. `idx` must be of the event's own chart version: a
    /// caller with another version's file is told which one to fetch.
    fn refs(&self, target: &Target) -> Result<Vec<ObjectRef>, ApiError> {
        match self {
            Self::Labels(labels) => {
                check_object_count(labels.len())?;
                Ok(labels
                    .iter()
                    .map(|l| ObjectRef::Label(l.as_str().into()))
                    .collect())
            }
            Self::Indices(i) => {
                check_object_count(i.idx.len())?;
                if i.version != target.hash {
                    return Err(ApiError::new(Code::VersionConflict)
                        .with_message("`objects.version` is not the event's chart version.")
                        .with_detail("version", target.hash.as_str()));
                }
                Ok(i.idx.iter().map(|&idx| ObjectRef::Idx(idx)).collect())
            }
        }
    }
}

// ---- hold tokens ----

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateToken {
    /// The event's key.
    event: String,
    /// The event's hold lifetime by default.
    #[serde(default)]
    ttl_seconds: Option<u32>,
    /// Unlimited by default: secret keys are not limited (02 §5).
    #[serde(default)]
    max_objects: Option<u32>,
}

/// Mint a hold token, register its hash with the event, and return the token,
/// once.
async fn create_token(
    SecretKey(auth): SecretKey,
    State(state): State<AppState>,
    Json(body): Json<CreateToken>,
) -> Result<Response, ApiError> {
    let mut bytes = [0; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(|e| ApiError::internal(e.to_string()))?;
    let mut token = String::from("ht_");
    push_hex(&mut token, &bytes);
    let hash = token_hash(&token);
    // Stored as `int`; no chart has more objects than this anyway.
    if body.max_objects.is_some_and(|m| m > MAX_OBJECTS) {
        return Err(invalid_field("max_objects", "must be at most 1,000,000"));
    }
    let done = submit(&state, &auth, Find::Key(&body.event), |target| {
        Ok(Command::CreateHold {
            token: hash,
            ttl_secs: body.ttl_seconds.unwrap_or(target.settings.hold_ttl()),
            max_objects: body.max_objects,
        })
    })
    .await?;
    let (seq, expires_at) = match done.reply {
        Ok(Outcome::HoldCreated {
            seq, expires_at, ..
        }) => (seq, expires_at),
        Ok(_) => return Err(ApiError::internal("unexpected reply to create_hold")),
        Err(f) => return Err(failure(&done.chart, f)),
    };
    let body = json!({
        "hold_token": token,
        "event": done.target.key,
        "seq": seq,
        "expires_at": rfc3339(expires_at),
        "max_objects": body.max_objects,
    });
    Ok((StatusCode::CREATED, axum::Json(body)).into_response())
}

fn no_token() -> ApiError {
    ApiError::new(Code::NotFound)
        .with_message("No such hold token: unknown, expired or of another workspace.")
        .with_detail("missing", "hold_token")
}

/// The token's hash, or `404` for anything that cannot be a token.
fn path_token(token: &str) -> Result<TokenHash, ApiError> {
    if has_hex_tail(token, "ht_", 2 * TOKEN_BYTES) {
        Ok(token_hash(token))
    } else {
        Err(no_token())
    }
}

const READ_TOKEN: &str = "SELECT e.key, replace(e.chart_version_id::text, '-', ''),
       (extract(epoch FROM h.expires_at) * 1000)::int8, h.max_objects,
       coalesce((SELECT array_agg(o.oid) FROM object_state o
                  WHERE o.event_id = h.event_id AND o.hold_id = h.id AND o.status = 1), '{}')
  FROM holds h JOIN events e ON e.id = h.event_id
 WHERE h.token_hash = $1 AND e.workspace_id = $2::uuid AND h.expires_at > now()";

type TokenRow = (String, String, i64, Option<i32>, Vec<i32>);

/// The token's event, expiry, limit and held objects, as committed.
async fn read_token(
    SecretKey(auth): SecretKey,
    Path(token): Path<String>,
    State(state): State<AppState>,
) -> Result<axum::Json<Value>, ApiError> {
    let hash = path_token(&token)?;
    let row: Option<TokenRow> = sqlx::query_as(READ_TOKEN)
        .bind(hash.0.as_slice())
        .bind(auth.workspace().uuid())
        .fetch_optional(&state.db)
        .instrument(cavea_telemetry::db_span("read_hold"))
        .await?;
    let (event, version, expires_ms, max_objects, oids) = row.ok_or_else(no_token)?;
    cavea_telemetry::record_event(&event);
    let version = event_id(&version).ok_or_else(|| ApiError::internal("unreadable version id"))?;
    let chart = chart(&state, ChartVersionId(version.0)).await?;
    // `oid` is a u32 stored bit for bit (08 §5).
    let mut idxs = oids
        .into_iter()
        .map(|oid| chart.view.idx_of_oid(oid as u32))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| ApiError::internal("held object not in the chart"))?;
    idxs.sort_unstable();
    Ok(axum::Json(json!({
        "event": event,
        "expires_at": rfc3339(Timestamp(u64::try_from(expires_ms).unwrap_or(0))),
        "max_objects": max_objects,
        "objects": objects_json(&chart, &idxs)?,
    })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Extend {
    /// The event's hold lifetime by default.
    #[serde(default)]
    ttl_seconds: Option<u32>,
}

/// Set the token's expiry to now plus the lifetime.
async fn extend_token(
    SecretKey(auth): SecretKey,
    Path(token): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<Extend>,
) -> Result<Response, ApiError> {
    let hash = path_token(&token)?;
    let done = submit(&state, &auth, Find::Token(&hash), |target| {
        Ok(Command::ExtendHold {
            token: hash,
            ttl_secs: body.ttl_seconds.unwrap_or(target.settings.hold_ttl()),
        })
    })
    .await?;
    respond(done)
}

/// Release everything the token holds. The token stays valid until it expires.
async fn release_token(
    SecretKey(auth): SecretKey,
    Path(token): Path<String>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let hash = path_token(&token)?;
    let done = submit(&state, &auth, Find::Token(&hash), |_| {
        Ok(Command::Release {
            token: hash,
            objects: Vec::new(),
        })
    })
    .await?;
    respond(done)
}

// ---- seat operations ----

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HoldBody {
    hold_token: String,
    objects: Objects,
}

async fn hold(
    SecretKey(auth): SecretKey,
    Path(event): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<HoldBody>,
) -> Result<Response, ApiError> {
    let done = submit(&state, &auth, Find::Key(&event), |target| {
        Ok(Command::Hold {
            token: token_hash(&body.hold_token),
            objects: body.objects.refs(target)?,
        })
    })
    .await?;
    respond(done)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseBody {
    hold_token: String,
    /// All of the token's objects if left out.
    #[serde(default)]
    objects: Option<Objects>,
}

async fn release(
    SecretKey(auth): SecretKey,
    Path(event): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<ReleaseBody>,
) -> Result<Response, ApiError> {
    let done = submit(&state, &auth, Find::Key(&event), |target| {
        Ok(Command::Release {
            token: token_hash(&body.hold_token),
            objects: match &body.objects {
                // An empty list releases nothing; leaving it out releases all.
                Some(objects) if objects.is_empty() => {
                    return Err(invalid_field(
                        "objects",
                        "must not be empty; leave it out to release all",
                    ));
                }
                Some(objects) => objects.refs(target)?,
                None => Vec::new(),
            },
        })
    })
    .await?;
    respond(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_from_unix_ms() {
        assert_eq!(rfc3339(Timestamp(0)), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(Timestamp(951_782_400_999)), "2000-02-29T00:00:00Z");
        assert_eq!(
            rfc3339(Timestamp(1_803_928_500_000)),
            "2027-03-01T19:15:00Z"
        );
    }
}
