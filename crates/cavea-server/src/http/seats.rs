//! Hold tokens and seat operations (08 §6.2, §7) with secret keys, and
//! `Idempotency-Key` handling (08 §6.1). Each handler maps its request onto an
//! engine command, sends it through the registry, and renders the reply with
//! each object's `label` and `oid`.
//!
//! A hold token is a bearer secret: it is minted here, returned once, and only
//! its SHA-256 is stored or given to the engine. Routes that carry it in the
//! path log only their route template.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use cavea_engine::{
    BookDetail, ChartVersionId, Clock as _, Command, EventId, Failure, Idempotency, Idx,
    ObjectFailure, ObjectRef, Outcome, Reply, Request, Status, StoreError, Timestamp, TokenHash,
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tracing::Instrument as _;

use super::auth::{Auth, SecretKey, has_hex_tail, push_hex, sha256};
use super::events::{Settings, archived};
use super::extract::{Json, Path, check_object_count, invalid, invalid_field};
use super::{ApiError, AppState, Code};
use crate::seats::{Chart, event_id};
use crate::store::reply;
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
        .route("/v1/events/{event}/book", post(book))
        .route("/v1/events/{event}/change-status", post(change_status))
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
    Id(EventId),
}

async fn target(state: &AppState, auth: &Auth, find: Find<'_>) -> Result<Target, ApiError> {
    let query = match &find {
        Find::Key(key) => sqlx::query_as(target_query!("e.key = $1")).bind(*key),
        Find::Token(hash) => sqlx::query_as(target_query!(
            "e.id = (SELECT event_id FROM holds WHERE token_hash = $1)"
        ))
        .bind(hash.0.as_slice()),
        Find::Id(id) => {
            sqlx::query_as(target_query!("e.id = $1::uuid")).bind(format!("{:032x}", id.0))
        }
    };
    let row: Option<TargetRow> = query
        .bind(auth.workspace().uuid())
        .fetch_optional(&state.db)
        .instrument(cavea_telemetry::db_span("find_event"))
        .await?;
    let Some((id, key, version, hash, archived, settings)) = row else {
        return Err(match find {
            Find::Token(_) => no_token(),
            _ => ApiError::new(Code::NotFound).with_message("No such event."),
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

// ---- idempotency (08 §6.1) ----

/// An `Idempotency-Key` and the request's fingerprint: SHA-256 over the
/// method, the route template, the path parameter (event key or hold token)
/// and the body as parsed and serialised again (fields in a fixed order,
/// without whitespace; `extra` objects as sent).
struct Key {
    key: Arc<str>,
    fingerprint: [u8; 32],
}

fn idempotency_key(
    headers: &HeaderMap,
    route: &str,
    param: &str,
    body: &impl Serialize,
) -> Result<Option<Key>, ApiError> {
    let Some(value) = headers.get("idempotency-key") else {
        return Ok(None);
    };
    let key = value
        .to_str()
        .ok()
        .filter(|k| {
            !k.is_empty()
                && k.chars().count() <= cavea_engine::limits::MAX_IDEMPOTENCY_KEY_CHARS
                && !k.contains('\0')
        })
        .ok_or_else(|| {
            invalid(
                "idempotency_key_length",
                "`Idempotency-Key` must be 1 to 255 visible ASCII characters.",
            )
        })?;
    let body = serde_json::to_vec(body).map_err(ApiError::internal)?;
    let mut hash = Sha256::new();
    for part in [route.as_bytes(), param.as_bytes(), &body] {
        hash.update(part);
        hash.update([0]);
    }
    Ok(Some(Key {
        key: key.into(),
        fingerprint: hash.finalize().into(),
    }))
}

const LOOKUP: &str = "SELECT replace(event_id::text, '-', ''), request_hash, response::text
  FROM idempotency
 WHERE workspace_id = $1::uuid AND key = $2 AND created_at > now() - interval '24 hours'";

/// The stored reply for the key: its event and reply, or `idempotency_mismatch`.
async fn lookup(
    state: &AppState,
    auth: &Auth,
    key: &Key,
) -> Result<Option<(EventId, Reply)>, ApiError> {
    let row: Option<(Option<String>, Vec<u8>, String)> = sqlx::query_as(LOOKUP)
        .bind(auth.workspace().uuid())
        .bind(&*key.key)
        .fetch_optional(&state.db)
        .instrument(cavea_telemetry::db_span("idempotency_lookup"))
        .await?;
    let Some((event, hash, response)) = row else {
        return Ok(None);
    };
    if hash != key.fingerprint {
        return Err(Failure::IdempotencyMismatch.into());
    }
    let event = event.as_deref().and_then(event_id);
    let reply = reply::decode(&response);
    match (event, reply) {
        (Some(event), Some(reply)) => Ok(Some((event, reply))),
        _ => Err(ApiError::internal("unreadable idempotency record")),
    }
}

/// A command's result, ready to render.
struct Done {
    target: Target,
    chart: Arc<Chart>,
    reply: Reply,
    /// From the store's idempotency record, not applied now.
    replayed: bool,
}

/// Replay a stored reply for the key, or find the event, refuse an archived
/// one, build the command and send it with the key.
async fn submit(
    state: &AppState,
    auth: &Auth,
    find: Find<'_>,
    key: Option<Key>,
    build: impl FnOnce(&Target) -> Result<Command, ApiError>,
) -> Result<Done, ApiError> {
    // Before the lookup: a record the lookup cannot see is younger than this.
    let checked_at = state.seats.clock.now();
    if let Some(key) = &key
        && let Some((event, reply)) = lookup(state, auth, key).await?
    {
        let target = target(state, auth, Find::Id(event)).await?;
        let chart = chart(state, target.version).await?;
        return Ok(Done {
            target,
            chart,
            reply,
            replayed: true,
        });
    }
    let target = target(state, auth, find).await?;
    if target.archived {
        return Err(archived());
    }
    let command = build(&target)?;
    let request = Request {
        actor: auth.actor(),
        authority: auth.authority(),
        command,
        idempotency: key.map(|k| Idempotency {
            key: k.key,
            fingerprint: k.fingerprint,
            checked_at,
        }),
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
        replayed: false,
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
    let outcome = done.reply.map_err(|f| failure(chart, f))?;
    let body = match &outcome {
        Outcome::Held {
            seq,
            expires_at,
            objects,
        } => json!({
            "seq": seq,
            "expires_at": rfc3339(*expires_at),
            "objects": objects_json(chart, objects)?,
        }),
        Outcome::Released { seq, objects }
        | Outcome::Booked { seq, objects }
        | Outcome::StatusChanged { seq, objects } => {
            json!({ "seq": seq, "objects": objects_json(chart, objects)? })
        }
        Outcome::Extended { seq, expires_at } => {
            json!({ "seq": seq, "expires_at": rfc3339(*expires_at) })
        }
        Outcome::HoldCreated { .. } => {
            return Err(ApiError::internal("unexpected reply to a seat command"));
        }
    };
    Ok(axum::Json(body).into_response())
}

// ---- objects in a request ----

/// Labels, or `idx` in a named chart version (08 §6.1).
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum Objects {
    Labels(Vec<String>),
    Indices(Indices),
}

#[derive(Debug, Deserialize, Serialize)]
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

fn optional_refs(objects: Option<&Objects>, target: &Target) -> Result<Vec<ObjectRef>, ApiError> {
    objects.map_or(Ok(Vec::new()), |o| o.refs(target))
}

// ---- hold tokens ----

#[derive(Debug, Deserialize, Serialize)]
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

const ROUTE_TOKENS: &str = "POST /v1/hold-tokens";

/// Mint a hold token, register its hash with the event, and return the token,
/// once. A replay of an `Idempotency-Key` returns the original status and
/// fields with `hold_token: null`: the token was never stored, and the token
/// whose reply was lost holds nothing and expires.
async fn create_token(
    SecretKey(auth): SecretKey,
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<CreateToken>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers, ROUTE_TOKENS, "", &body)?;
    let keyed = key.is_some();
    let mut bytes = [0; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(|e| ApiError::internal(e.to_string()))?;
    let mut token = String::from("ht_");
    push_hex(&mut token, &bytes);
    let hash = token_hash(&token);
    // Stored as `int`; no chart has more objects than this anyway.
    if body.max_objects.is_some_and(|m| m > MAX_OBJECTS) {
        return Err(invalid_field("max_objects", "must be at most 1,000,000"));
    }
    let done = submit(&state, &auth, Find::Key(&body.event), key, |target| {
        Ok(Command::CreateHold {
            token: hash,
            ttl_secs: body.ttl_seconds.unwrap_or(target.settings.hold_ttl()),
            max_objects: body.max_objects,
        })
    })
    .await?;
    let (seq, hold, expires_at) = match done.reply {
        Ok(Outcome::HoldCreated {
            seq,
            hold,
            expires_at,
        }) => (seq, hold, expires_at),
        Ok(_) => return Err(ApiError::internal("unexpected reply to create_hold")),
        Err(f) => return Err(failure(&done.chart, f)),
    };
    // The actor's window may have answered a concurrent request with the same
    // key: then this token was never registered.
    let mine = !done.replayed
        && (!keyed || {
            sqlx::query_scalar(
                "SELECT EXISTS (SELECT FROM holds
                  WHERE event_id = $1::uuid AND id = $2 AND token_hash = $3)",
            )
            .bind(format!("{:032x}", done.target.id.0))
            .bind(i64::try_from(hold.0).map_err(ApiError::internal)?)
            .bind(hash.0.as_slice())
            .fetch_one(&state.db)
            .instrument(cavea_telemetry::db_span("hold_is_mine"))
            .await?
        });
    let body = json!({
        "hold_token": mine.then_some(token),
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

#[derive(Debug, Default, Deserialize, Serialize)]
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
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<Extend>,
) -> Result<Response, ApiError> {
    let hash = path_token(&token)?;
    let key = idempotency_key(
        &headers,
        "POST /v1/hold-tokens/{token}/extend",
        &token,
        &body,
    )?;
    let done = submit(&state, &auth, Find::Token(&hash), key, |target| {
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
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let hash = path_token(&token)?;
    let key = idempotency_key(&headers, "DELETE /v1/hold-tokens/{token}", &token, &())?;
    let done = submit(&state, &auth, Find::Token(&hash), key, |_| {
        Ok(Command::Release {
            token: hash,
            objects: Vec::new(),
        })
    })
    .await?;
    respond(done)
}

// ---- seat operations ----

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HoldBody {
    hold_token: String,
    objects: Objects,
}

async fn hold(
    SecretKey(auth): SecretKey,
    Path(event): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<HoldBody>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers, "POST /v1/events/{event}/hold", &event, &body)?;
    let done = submit(&state, &auth, Find::Key(&event), key, |target| {
        Ok(Command::Hold {
            token: token_hash(&body.hold_token),
            objects: body.objects.refs(target)?,
        })
    })
    .await?;
    respond(done)
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReleaseBody {
    /// With a token: held objects (all of the token's if `objects` is left
    /// out). Without: booked objects.
    #[serde(default)]
    hold_token: Option<String>,
    #[serde(default)]
    objects: Option<Objects>,
}

async fn release(
    SecretKey(auth): SecretKey,
    Path(event): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<ReleaseBody>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers, "POST /v1/events/{event}/release", &event, &body)?;
    let done = submit(&state, &auth, Find::Key(&event), key, |target| {
        Ok(match &body.hold_token {
            // An empty list releases nothing; leaving it out releases all.
            Some(_) if body.objects.as_ref().is_some_and(Objects::is_empty) => {
                return Err(invalid_field(
                    "objects",
                    "must not be empty; leave it out to release all",
                ));
            }
            Some(token) => Command::Release {
                token: token_hash(token),
                objects: optional_refs(body.objects.as_ref(), target)?,
            },
            None => Command::ReleaseBooked {
                objects: body
                    .objects
                    .as_ref()
                    .ok_or_else(|| invalid_field("objects", "is required without `hold_token`"))?
                    .refs(target)?,
            },
        })
    })
    .await?;
    respond(done)
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BookBody {
    /// Book held objects (all of the token's if `objects` is left out), or,
    /// without a token, free objects directly.
    #[serde(default)]
    hold_token: Option<String>,
    #[serde(default)]
    objects: Option<Objects>,
    order_ref: String,
    /// Ticket type per object, keyed by label.
    #[serde(default)]
    ticket_types: BTreeMap<String, String>,
    /// A JSON object per object, keyed by label, stored as sent.
    #[serde(default)]
    extra: BTreeMap<String, Box<RawValue>>,
}

async fn book(
    SecretKey(auth): SecretKey,
    Path(event): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<BookBody>,
) -> Result<Response, ApiError> {
    if body
        .extra
        .values()
        .any(|e| !e.get().trim_start().starts_with('{'))
    {
        return Err(invalid_field("extra", "values must be JSON objects"));
    }
    check_object_count(body.ticket_types.len().max(body.extra.len()))?;
    let key = idempotency_key(&headers, "POST /v1/events/{event}/book", &event, &body)?;
    let done = submit(&state, &auth, Find::Key(&event), key, |target| {
        let mut details: BTreeMap<&str, BookDetail> = BTreeMap::new();
        let detail = |label: &str| BookDetail {
            object: ObjectRef::Label(label.into()),
            ticket_type: None,
            extra: None,
        };
        for (label, ticket_type) in &body.ticket_types {
            details
                .entry(label)
                .or_insert_with(|| detail(label))
                .ticket_type = Some(ticket_type.as_str().into());
        }
        for (label, extra) in &body.extra {
            details.entry(label).or_insert_with(|| detail(label)).extra = Some(extra.get().into());
        }
        Ok(Command::Book {
            token: body.hold_token.as_deref().map(token_hash),
            objects: optional_refs(body.objects.as_ref(), target)?,
            order_ref: body.order_ref.as_str().into(),
            details: details.into_values().collect(),
        })
    })
    .await?;
    respond(done)
}

/// `free`, `held`, `booked` or `blocked`, or a custom status code, 4 to 15.
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum StatusWire {
    Name(String),
    Code(u8),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ChangeStatusBody {
    objects: Objects,
    status: StatusWire,
}

async fn change_status(
    SecretKey(auth): SecretKey,
    Path(event): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<ChangeStatusBody>,
) -> Result<Response, ApiError> {
    let status = match &body.status {
        StatusWire::Name(name) => match name.as_str() {
            "free" => Some(Status::FREE),
            "held" => Some(Status::HELD),
            "booked" => Some(Status::BOOKED),
            "blocked" => Some(Status::BLOCKED),
            _ => None,
        },
        StatusWire::Code(code) => Status::from_code(*code),
    }
    .ok_or_else(|| {
        invalid_field(
            "status",
            "must be \"free\", \"blocked\" or a custom status code from 4 to 15",
        )
    })?;
    let key = idempotency_key(
        &headers,
        "POST /v1/events/{event}/change-status",
        &event,
        &body,
    )?;
    let done = submit(&state, &auth, Find::Key(&event), key, |target| {
        Ok(Command::ChangeStatus {
            objects: body.objects.refs(target)?,
            status,
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
