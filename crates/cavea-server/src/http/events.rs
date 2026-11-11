//! `/v1/events`: create, list, read, change and archive events (08 §6.2).
//! Secret key; every query is scoped by the workspace of the key. `{event}`
//! is the customer's event key. Seat operations are in `http::seats`.

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tracing::Instrument as _;

use super::auth::SecretKey;
use super::charts::Version;
use super::extract::{Json, Page, Path, Query, check_text, invalid, invalid_field};
use super::{ApiError, AppState, Code, List};
use crate::seats::event_id;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/events", get(list).post(create))
        .route(
            "/v1/events/{event}",
            get(read).patch(update).delete(archive),
        )
}

/// Default hold lifetime in seconds (02 §5).
pub(super) const DEFAULT_HOLD_TTL: u32 = 900;
/// Default per-token object limit for publishable keys (02 §5).
pub(super) const DEFAULT_MAX_OBJECTS: u32 = 10;
/// The highest per-token object limit an event may set for publishable keys.
const MAX_MAX_OBJECTS: u32 = 100;

/// An event's settings as stored: only what the customer set. A field left
/// out takes its default.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Settings {
    #[serde(skip_serializing_if = "Option::is_none")]
    hold_ttl_seconds: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_objects: Option<u32>,
}

impl Settings {
    pub(super) fn parse(stored: &str) -> Result<Self, ApiError> {
        serde_json::from_str(stored).map_err(ApiError::internal)
    }

    /// The default lifetime of this event's hold tokens.
    pub(super) fn hold_ttl(self) -> u32 {
        self.hold_ttl_seconds.unwrap_or(DEFAULT_HOLD_TTL)
    }

    fn check(self) -> Result<Self, ApiError> {
        let ttl = cavea_engine::limits::MIN_HOLD_TTL_SECS..=cavea_engine::limits::MAX_HOLD_TTL_SECS;
        if !ttl.contains(&self.hold_ttl()) {
            return Err(invalid_field(
                "settings.hold_ttl_seconds",
                "must be 60 to 7,200",
            ));
        }
        if self
            .max_objects
            .is_some_and(|m| m == 0 || m > MAX_MAX_OBJECTS)
        {
            return Err(invalid_field("settings.max_objects", "must be 1 to 100"));
        }
        Ok(self)
    }

    /// Every setting with its effective value.
    fn effective(self) -> Value {
        json!({
            "hold_ttl_seconds": self.hold_ttl(),
            "max_objects": self.max_objects.unwrap_or(DEFAULT_MAX_OBJECTS),
        })
    }
}

/// `events` columns aliased `e`, its chart's key (`c`) and its pinned
/// version's columns (`v`), in [`EventRow`] order.
macro_rules! event_columns {
    () => {
        concat!(
            "replace(e.id::text, '-', ''), e.key, e.name, c.key, e.settings::text, ",
            rfc3339!("e", ".archived_at"),
            ", ",
            rfc3339!("e", ".created_at"),
            ", ",
            version_columns!(),
            " FROM ",
        )
    };
}

type EventRow = (
    String,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    String,
    String,
    i32,
    String,
    String,
    String,
);

/// An event as the API shows it.
#[derive(Debug, Serialize)]
struct Event {
    key: String,
    name: Option<String>,
    /// The chart's key.
    chart: String,
    /// The pinned chart version; its `url` is the public chart file.
    version: Version,
    settings: Value,
    archived_at: Option<String>,
    created_at: String,
    /// Objects per status: only in `GET /v1/events/{event}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    counts: Option<Value>,
    #[serde(skip)]
    id: String,
    #[serde(skip)]
    version_id: String,
}

impl Event {
    fn new(row: EventRow) -> Result<Self, ApiError> {
        let (id, key, name, chart, settings, archived_at, created_at, v0, v1, v2, v3, v4) = row;
        Ok(Self {
            key,
            name,
            chart,
            version_id: v0.clone(),
            version: Version::new((v0, v1, v2, v3, v4))?,
            settings: Settings::parse(&settings)?.effective(),
            archived_at,
            created_at,
            counts: None,
            id,
        })
    }
}

fn not_found() -> ApiError {
    ApiError::new(Code::NotFound).with_message("No such event.")
}

pub(super) fn archived() -> ApiError {
    invalid(
        "archived",
        "The event is archived: it is read-only and takes no seat operations.",
    )
}

/// Oldest first; archived events included. `after` is an event key.
const LIST: &str = concat!(
    "SELECT ",
    event_columns!(),
    "events",
    " e JOIN chart_versions v ON v.id = e.chart_version_id JOIN charts c ON c.id = v.chart_id
 WHERE e.workspace_id = $1::uuid
   AND ($2::text IS NULL OR (e.created_at, e.id) >
        (SELECT created_at, id FROM events WHERE workspace_id = $1::uuid AND key = $2))
 ORDER BY e.created_at, e.id
 LIMIT $3"
);

async fn list(
    SecretKey(auth): SecretKey,
    Query(page): Query<Page>,
    State(state): State<AppState>,
) -> Result<axum::Json<List<Event>>, ApiError> {
    let limit = page.limit()?;
    let rows: Vec<EventRow> = sqlx::query_as(LIST)
        .bind(auth.workspace().uuid())
        .bind(page.after.as_deref())
        .bind(i64::from(limit) + 1)
        .fetch_all(&state.db)
        .instrument(cavea_telemetry::db_span("list_events"))
        .await?;
    let events = rows
        .into_iter()
        .map(Event::new)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(axum::Json(List::new(events, limit, |e| e.key.clone())))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateEvent {
    key: String,
    /// The chart's key.
    chart: String,
    /// A version number of the chart; its published version by default.
    #[serde(default)]
    version: Option<i32>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    settings: Settings,
}

/// The chart's archive flag and the version to pin: the numbered one, or the
/// published one.
const CHART_VERSION: &str = "SELECT c.archived_at IS NOT NULL,
       replace((CASE WHEN $3::int IS NULL THEN c.published_version_id ELSE v.id END)::text, '-', '')
  FROM charts c LEFT JOIN chart_versions v ON v.chart_id = c.id AND v.number = $3
 WHERE c.workspace_id = $1::uuid AND c.key = $2";

const CREATE: &str = concat!(
    "WITH e AS (
  INSERT INTO events (id, workspace_id, key, name, chart_version_id, settings)
  VALUES (gen_random_uuid(), $1::uuid, $2, $3, $4::uuid, $5::jsonb)
  RETURNING *
)
SELECT ",
    event_columns!(),
    "e JOIN chart_versions v ON v.id = e.chart_version_id JOIN charts c ON c.id = v.chart_id"
);

/// Create an event pinned to one immutable chart version (D-11).
async fn create(
    SecretKey(auth): SecretKey,
    State(state): State<AppState>,
    Json(body): Json<CreateEvent>,
) -> Result<(StatusCode, axum::Json<Event>), ApiError> {
    check_text("key", &body.key)?;
    if let Some(name) = &body.name {
        check_text("name", name)?;
    }
    let settings = body.settings.check()?;
    let chart: Option<(bool, Option<String>)> = sqlx::query_as(CHART_VERSION)
        .bind(auth.workspace().uuid())
        .bind(&body.chart)
        .bind(body.version)
        .fetch_optional(&state.db)
        .instrument(cavea_telemetry::db_span("event_chart_version"))
        .await?;
    let version = match chart {
        None => {
            return Err(ApiError::new(Code::NotFound)
                .with_message("No such chart.")
                .with_detail("missing", "chart"));
        }
        Some((true, _)) => {
            return Err(invalid(
                "archived",
                "The chart is archived: events can no longer be created on it.",
            ));
        }
        Some((false, Some(version))) => version,
        Some((false, None)) if body.version.is_some() => {
            return Err(ApiError::new(Code::NotFound)
                .with_message("The chart has no version with this number.")
                .with_detail("missing", "version"));
        }
        Some((false, None)) => {
            return Err(invalid(
                "unpublished",
                "The chart has no published version.",
            ));
        }
    };
    let settings_text = serde_json::to_string(&settings).map_err(ApiError::internal)?;
    let row: Result<EventRow, _> = sqlx::query_as(CREATE)
        .bind(auth.workspace().uuid())
        .bind(&body.key)
        .bind(body.name.as_deref())
        .bind(&version)
        .bind(&settings_text)
        .fetch_one(&state.db)
        .instrument(cavea_telemetry::db_span("create_event"))
        .await;
    let row = match row {
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(ApiError::new(Code::AlreadyExists)
                .with_message("An event with this key already exists, possibly archived."));
        }
        row => row?,
    };
    let event = Event::new(row)?;
    cavea_telemetry::record_event(&event.key);
    tracing::info!(chart_version = %format!("cv_{}", event.version_id), "event created");
    Ok((StatusCode::CREATED, axum::Json(event)))
}

const READ: &str = concat!(
    "SELECT ",
    event_columns!(),
    "events",
    " e JOIN chart_versions v ON v.id = e.chart_version_id JOIN charts c ON c.id = v.chart_id
 WHERE e.workspace_id = $1::uuid AND e.key = $2"
);

const COUNTS: &str = "SELECT status, count(*) FROM object_state
 WHERE event_id = $1::uuid GROUP BY status";

/// The event with its objects counted by status. Counts are the committed
/// state in PostgreSQL, which is exactly what has been acknowledged: reading
/// them does not load the event's actor. A hold past its expiry counts as held
/// until its expiry commits, which a loaded event does at once.
async fn read(
    SecretKey(auth): SecretKey,
    Path(key): Path<String>,
    State(state): State<AppState>,
) -> Result<axum::Json<Event>, ApiError> {
    cavea_telemetry::record_event(&key);
    let row: Option<EventRow> = sqlx::query_as(READ)
        .bind(auth.workspace().uuid())
        .bind(&key)
        .fetch_optional(&state.db)
        .instrument(cavea_telemetry::db_span("read_event"))
        .await?;
    let mut event = Event::new(row.ok_or_else(not_found)?)?;
    let counts: Vec<(i16, i64)> = sqlx::query_as(COUNTS)
        .bind(&event.id)
        .fetch_all(&state.db)
        .instrument(cavea_telemetry::db_span("count_objects"))
        .await?;
    let version = event_id(&event.version_id)
        .ok_or_else(|| ApiError::internal("unreadable chart version id"))?;
    let chart = super::seats::chart(&state, cavea_engine::ChartVersionId(version.0)).await?;
    let mut by_name = Map::new();
    let mut custom = Map::new();
    let mut taken = 0;
    // A free object may have a row (for its channel): it is not taken.
    for (status, count) in counts.into_iter().filter(|(status, _)| *status != 0) {
        taken += count;
        match status {
            1 => by_name.insert("held".into(), count.into()),
            2 => by_name.insert("booked".into(), count.into()),
            3 => by_name.insert("blocked".into(), count.into()),
            code => custom.insert(code.to_string(), count.into()),
        };
    }
    let total = i64::try_from(chart.view.len()).map_err(ApiError::internal)?;
    let mut counts = json!({ "free": total - taken, "held": 0, "booked": 0, "blocked": 0 });
    if let Some(counts) = counts.as_object_mut() {
        counts.extend(by_name);
        counts.insert("custom".into(), custom.into());
    }
    event.counts = Some(counts);
    Ok(axum::Json(event))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateEvent {
    /// Left out: unchanged. `null`: cleared.
    #[serde(default, deserialize_with = "present")]
    name: Option<Option<String>>,
    /// Replaces the settings as a whole: a field left out takes its default.
    #[serde(default)]
    settings: Option<Settings>,
}

/// A field that is present, `null` included: `Some(None)` for `null`.
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::deserialize(d).map(Some)
}

const UPDATE: &str = concat!(
    "WITH e AS (
  UPDATE events SET name = CASE WHEN $5 THEN $3 ELSE name END,
                    settings = coalesce($4::jsonb, settings)
   WHERE workspace_id = $1::uuid AND key = $2 AND archived_at IS NULL
  RETURNING *
)
SELECT ",
    event_columns!(),
    "e JOIN chart_versions v ON v.id = e.chart_version_id JOIN charts c ON c.id = v.chart_id"
);

const EXISTS: &str =
    "SELECT EXISTS (SELECT FROM events WHERE workspace_id = $1::uuid AND key = $2)";

/// Change the name or the settings. New settings apply to tokens created and
/// extended from then on; existing tokens keep their expiry and limit.
async fn update(
    SecretKey(auth): SecretKey,
    Path(key): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<UpdateEvent>,
) -> Result<axum::Json<Event>, ApiError> {
    cavea_telemetry::record_event(&key);
    if let Some(Some(name)) = &body.name {
        check_text("name", name)?;
    }
    let settings = match body.settings {
        Some(s) => Some(serde_json::to_string(&s.check()?).map_err(ApiError::internal)?),
        None => None,
    };
    let update = async {
        let row: Option<EventRow> = sqlx::query_as(UPDATE)
            .bind(auth.workspace().uuid())
            .bind(&key)
            .bind(body.name.as_ref().and_then(Option::as_deref))
            .bind(settings.as_deref())
            .bind(body.name.is_some())
            .fetch_optional(&state.db)
            .await?;
        if let Some(row) = row {
            return Ok(row);
        }
        let exists: bool = sqlx::query_scalar(EXISTS)
            .bind(auth.workspace().uuid())
            .bind(&key)
            .fetch_one(&state.db)
            .await?;
        Err(if exists { archived() } else { not_found() })
    };
    let row = update
        .instrument(cavea_telemetry::db_span("update_event"))
        .await?;
    Ok(axum::Json(Event::new(row)?))
}

/// Idempotent: archiving an archived event returns it unchanged.
const ARCHIVE: &str = concat!(
    "WITH e AS (
  UPDATE events SET archived_at = coalesce(archived_at, now())
   WHERE workspace_id = $1::uuid AND key = $2
  RETURNING *
)
SELECT ",
    event_columns!(),
    "e JOIN chart_versions v ON v.id = e.chart_version_id JOIN charts c ON c.id = v.chart_id"
);

/// Archive an event (08 §6.2): it becomes read-only. Seat operations, token
/// creation and changes are refused; outstanding holds are left to expire on
/// schedule; bookings stay; the loaded actor is left to evict itself once it
/// has no holds. The key stays taken.
async fn archive(
    SecretKey(auth): SecretKey,
    Path(key): Path<String>,
    State(state): State<AppState>,
) -> Result<axum::Json<Event>, ApiError> {
    cavea_telemetry::record_event(&key);
    let row: Option<EventRow> = sqlx::query_as(ARCHIVE)
        .bind(auth.workspace().uuid())
        .bind(&key)
        .fetch_optional(&state.db)
        .instrument(cavea_telemetry::db_span("archive_event"))
        .await?;
    let event = Event::new(row.ok_or_else(not_found)?)?;
    tracing::info!("event archived");
    Ok(axum::Json(event))
}
