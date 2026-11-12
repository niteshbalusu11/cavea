//! `/v1/events`: create, list, read, change and archive events (08 §6.2).
//! Secret key; every query is scoped by the workspace of the key. `{event}`
//! is the customer's event key. Seat operations are in `http::seats`.

use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tracing::Instrument as _;

use super::Reason;
use super::charts::Version;
use super::extract::{
    Json, Page, Path, Query, check_text, invalid, invalid_field, object, optional_object,
};
use super::openapi::{Needs, Op, Routes};
use super::seats::{Find, target};
use super::staff::Staff;
use super::{ApiError, AppState, Code, List};
use crate::audit::{self, Entry};
use crate::console::Permission;

pub(super) fn routes() -> Routes {
    let reading = |op: Op| op.needs(Needs::Staff(Permission::EventsRead));
    let writing = |op: Op| op.needs(Needs::Staff(Permission::EventsWrite));
    Routes::new()
        .add(
            reading(Op::get("/v1/events", "listEvents", "List events"))
                .about(
                    "Archived events included unless `archived` says, oldest first unless \
`order=newest`. The cursor is an event key. `chart` keeps one chart's events; a chart the \
workspace does not have is `404`. Each event has its `counts` and committed `seq`.",
                )
                .page()
                .query(
                    "chart",
                    serde_json::json!({ "type": "string" }),
                    "A chart's key: only events on one of its versions.",
                )
                .query(
                    "q",
                    serde_json::json!({ "type": "string", "maxLength": MAX_SEARCH }),
                    "Events whose key or name contains this text, ignoring case; every \
character matches itself (no wildcards).",
                )
                .query(
                    "archived",
                    serde_json::json!({ "type": "boolean" }),
                    "`true`: archived events only; `false`: unarchived only.",
                )
                .query(
                    "order",
                    serde_json::json!({ "enum": ["oldest", "newest"] }),
                    "By creation: `oldest` first (the default) or `newest` first.",
                )
                .query(
                    "created_after",
                    serde_json::json!({ "type": "string" }),
                    "Events created at or after this time: `YYYY-MM-DD` (00:00 UTC) or \
`YYYY-MM-DDTHH:MM:SSZ`, with optional fractions of a second.",
                )
                .query(
                    "created_before",
                    serde_json::json!({ "type": "string" }),
                    "Events created before this time, in the same forms. With \
`created_after`, a range: `created_after=2027-03-01&created_before=2027-04-01` is March.",
                )
                .reply(200, "EventList", "A page of events")
                .errors(&[404]),
            list,
        )
        .add(
            writing(Op::post("/v1/events", "createEvent", "Create an event"))
                .about("Pins the chart's published version, or `version`.")
                .body("CreateEvent")
                .reply(201, "Event", "The event")
                .errors(&[404, 409]),
            create,
        )
        .add(
            reading(Op::get(
                "/v1/events/{event}",
                "getEvent",
                "An event, with its counts",
            ))
            .reply(200, "Event", "The event, with `counts`"),
            read,
        )
        .add(
            reading(Op::get(
                "/v1/events/{event}/state",
                "eventState",
                "The event's seat state for staff",
            ))
            .about(
                "For staff: the realtime frames in domain statuses (0 to 15). Without `after_seq`, or with 0: the manager SNAPSHOT. With it: one manager DELTA from \
it, the SNAPSHOT when the deltas from it are no longer kept (about 60 s), or `204` when nothing \
was published after it. Encoding 0 or 1, never 2. `Cache-Control: no-store`.",
            )
            .query(
                "after_seq",
                serde_json::json!({ "type": "integer", "minimum": 0 }),
                "The last `seq` the caller has.",
            )
            .response(
                200,
                "A frame",
                Some(serde_json::json!({ super::realtime::FRAME_TYPE: {} })),
            )
            .response(204, "Nothing published after `after_seq`", None)
            .errors(&[503]),
            staff_state,
        )
        .add(
            writing(Op::patch(
                "/v1/events/{event}",
                "updateEvent",
                "Change an event's name or settings",
            ))
            .body("UpdateEvent")
            .reply(200, "Event", "The event"),
            update,
        )
        .add(
            writing(Op::delete(
                "/v1/events/{event}",
                "archiveEvent",
                "Archive an event",
            ))
            .about("Idempotent. The event becomes read-only.")
            .reply(200, "Event", "The event, archived"),
            archive,
        )
}

/// Default hold lifetime in seconds (02 §5).
pub(super) const DEFAULT_HOLD_TTL: u32 = 900;
/// Default per-token object limit for publishable keys (02 §5).
pub(super) const DEFAULT_MAX_OBJECTS: u32 = 10;
/// The highest per-token object limit an event may set for publishable keys.
const MAX_MAX_OBJECTS: u32 = 100;
/// Default live hold tokens one session `sub` may have on an event (08 §11).
const DEFAULT_TOKENS_PER_SUB: u32 = 2;
/// The highest `max_tokens_per_sub` an event may set.
const MAX_TOKENS_PER_SUB: u32 = 10;

/// An event's settings as stored: only what the customer set. A field left
/// out takes its default.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Settings {
    #[serde(skip_serializing_if = "Option::is_none")]
    hold_ttl_seconds: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_objects: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    require_session: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens_per_sub: Option<u32>,
}

impl Settings {
    pub(super) fn parse(stored: &str) -> Result<Self, ApiError> {
        serde_json::from_str(stored).map_err(ApiError::internal)
    }

    /// The default lifetime of this event's hold tokens.
    pub(super) fn hold_ttl(self) -> u32 {
        self.hold_ttl_seconds.unwrap_or(DEFAULT_HOLD_TTL)
    }

    /// The object limit of this event's publishable-key hold tokens.
    pub(super) fn max_objects(self) -> u32 {
        self.max_objects.unwrap_or(DEFAULT_MAX_OBJECTS)
    }

    /// Whether the public hold routes require a session token (08 §11).
    pub(super) fn require_session(self) -> bool {
        self.require_session.unwrap_or(false)
    }

    /// How many live public hold tokens one session `sub` may have.
    pub(super) fn max_tokens_per_sub(self) -> u32 {
        self.max_tokens_per_sub.unwrap_or(DEFAULT_TOKENS_PER_SUB)
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
        if !(1..=MAX_TOKENS_PER_SUB).contains(&self.max_tokens_per_sub()) {
            return Err(invalid_field(
                "settings.max_tokens_per_sub",
                "must be 1 to 10",
            ));
        }
        Ok(self)
    }

    /// Every setting with its effective value.
    fn effective(self) -> Value {
        json!({
            "hold_ttl_seconds": self.hold_ttl(),
            "max_objects": self.max_objects(),
            "require_session": self.require_session(),
            "max_tokens_per_sub": self.max_tokens_per_sub(),
        })
    }
}

/// `events` columns aliased `e`, its chart's key (`c`) and its pinned
/// version's columns (`v`), in [`EventRow`] order.
macro_rules! event_columns {
    () => {
        concat!(
            "e.key, e.name, c.key, e.settings::text, ",
            rfc3339!("e", ".archived_at"),
            ", ",
            rfc3339!("e", ".created_at"),
            ", e.seq, e.counts, ",
            version_columns!(),
            " FROM ",
        )
    };
}

type EventRow = (
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    String,
    i64,
    Vec<i32>,
    String,
    i32,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
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
    /// The committed `seq`: it changes exactly when the seat state does.
    seq: i64,
    /// Objects per status, committed.
    counts: Value,
    #[serde(skip)]
    version_id: String,
}

impl Event {
    fn new(row: EventRow) -> Result<Self, ApiError> {
        let (key, name, chart, settings, archived_at, created_at, seq, counts) =
            (row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7);
        let version = Version::new((row.8, row.9, row.10, row.11, row.12, row.13, row.14))?;
        Ok(Self {
            key,
            name,
            chart,
            version_id: version.uuid().to_owned(),
            counts: counts_json(version.objects()?, &counts),
            version,
            settings: Settings::parse(&settings)?.effective(),
            archived_at,
            created_at,
            seq,
        })
    }
}

/// `{free, held, booked, blocked, custom}` from `events.counts` (statuses 1
/// to 15) and the version's object count; a custom status is listed only
/// when some object has it.
fn counts_json(objects: i64, counts: &[i32]) -> Value {
    let n = |code: usize| i64::from(counts.get(code - 1).copied().unwrap_or(0));
    let taken: i64 = (1..=15).map(n).sum();
    let custom: Map<String, Value> = (4..=15)
        .filter(|&code| n(code) != 0)
        .map(|code| (code.to_string(), n(code).into()))
        .collect();
    json!({
        "free": objects - taken,
        "held": n(1),
        "booked": n(2),
        "blocked": n(3),
        "custom": custom,
    })
}

fn not_found() -> ApiError {
    ApiError::new(Code::NotFound).with_message("No such event.")
}

pub(super) fn archived() -> ApiError {
    invalid(
        Reason::Archived,
        "The event is archived: it is read-only and takes no seat operations.",
    )
}

/// A page of events in creation order (`$cmp` `>` and `$dir` `ASC` for
/// oldest first). `after` ($2) is an event key; when not null, `$4` a chart
/// key, `$5` text the key or name contains (ignoring case, no wildcards:
/// `strpos`, not `LIKE`), `$6` whether archived, `$7` and `$8` the creation
/// range in microseconds since the epoch, from inclusive, to exclusive.
macro_rules! list_events {
    ($cmp:literal, $dir:literal) => {
        concat!(
            "SELECT ",
            event_columns!(),
            "events",
            " e JOIN chart_versions v ON v.id = e.chart_version_id JOIN charts c ON c.id = v.chart_id
 WHERE e.workspace_id = $1::uuid
   AND ($2::text IS NULL OR (e.created_at, e.id) ",
            $cmp,
            "
        (SELECT created_at, id FROM events WHERE workspace_id = $1::uuid AND key = $2))
   AND ($4::text IS NULL OR c.key = $4)
   AND ($5::text IS NULL OR strpos(lower(e.key), lower($5)) > 0
        OR strpos(lower(coalesce(e.name, '')), lower($5)) > 0)
   AND ($6::bool IS NULL OR (e.archived_at IS NOT NULL) = $6)
   AND ($7::int8 IS NULL OR e.created_at >= timestamptz 'epoch' + $7 * interval '1 microsecond')
   AND ($8::int8 IS NULL OR e.created_at < timestamptz 'epoch' + $8 * interval '1 microsecond')
 ORDER BY e.created_at ",
            $dir,
            ", e.id ",
            $dir,
            "
 LIMIT $3"
        )
    };
}

const LIST_OLDEST: &str = list_events!(">", "ASC");
const LIST_NEWEST: &str = list_events!("<", "DESC");

/// The longest search text, in characters: a key or a name is at most 128.
const MAX_SEARCH: usize = 128;

const CHART_EXISTS: &str =
    "SELECT EXISTS (SELECT FROM charts WHERE workspace_id = $1::uuid AND key = $2)";

/// `?limit=&after=&chart=&q=&archived=&order=&created_after=&created_before=`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListQuery {
    limit: Option<u32>,
    after: Option<String>,
    chart: Option<String>,
    q: Option<String>,
    archived: Option<bool>,
    order: Option<Order>,
    created_after: Option<String>,
    created_before: Option<String>,
}

/// A time of the creation filter, in microseconds since the epoch:
/// `YYYY-MM-DD` (its 00:00 UTC) or `YYYY-MM-DDTHH:MM:SS[.fraction]Z`, from
/// 1970 to 9999. Anything else, offsets included, is `None`.
fn instant(text: &str) -> Option<i64> {
    let (date, time) = match text.split_once('T') {
        Some((date, time)) => (date, Some(time.strip_suffix('Z')?)),
        None => (text, None),
    };
    let day = i64::from(cavea_licence::Date::parse(date)?.days());
    let Some(time) = time else {
        return Some(day * 86_400_000_000);
    };
    let (hms, fraction) = match time.split_once('.') {
        Some((hms, fraction)) => (hms, Some(fraction)),
        None => (time, None),
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
    let parts: Vec<&str> = hms.split(':').collect();
    let [h, m, sec] = parts.as_slice() else {
        return None;
    };
    let two = |s: &str| (s.len() == 2 && digits(s)).then(|| s.parse::<i64>().ok())?;
    let (h, m, sec) = (two(h)?, two(m)?, two(sec)?);
    if h > 23 || m > 59 || sec > 59 {
        return None;
    }
    let micros = match fraction {
        None => 0,
        Some(f) if f.len() <= 9 && digits(f) => format!("{:0<6}", f.get(..f.len().min(6))?)
            .parse::<i64>()
            .ok()?,
        Some(_) => return None,
    };
    Some(day * 86_400_000_000 + ((h * 60 + m) * 60 + sec) * 1_000_000 + micros)
}

fn created(text: Option<&str>, name: &str) -> Result<Option<i64>, ApiError> {
    text.map(|t| {
        instant(t).ok_or_else(|| {
            ApiError::from(Reason::Query).with_message(format!(
                "`{name}` must be `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SSZ`."
            ))
        })
    })
    .transpose()
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Order {
    #[default]
    Oldest,
    Newest,
}

async fn list(
    Staff(auth): Staff,
    Query(query): Query<ListQuery>,
    State(state): State<AppState>,
) -> Result<axum::Json<List<Event>>, ApiError> {
    let limit = Page::new(query.limit, None).limit()?;
    let search = query.q.as_deref().filter(|q| !q.is_empty());
    if search.is_some_and(|q| q.chars().count() > MAX_SEARCH) {
        return Err(invalid(Reason::Query, "`q` is longer than 128 characters."));
    }
    let after = created(query.created_after.as_deref(), "created_after")?;
    let before = created(query.created_before.as_deref(), "created_before")?;
    let sql = match query.order.unwrap_or_default() {
        Order::Oldest => LIST_OLDEST,
        Order::Newest => LIST_NEWEST,
    };
    let rows: Vec<EventRow> = sqlx::query_as(sql)
        .bind(auth.workspace().uuid())
        .bind(query.after.as_deref())
        .bind(i64::from(limit) + 1)
        .bind(query.chart.as_deref())
        .bind(search)
        .bind(query.archived)
        .bind(after)
        .bind(before)
        .fetch_all(&state.db)
        .instrument(cavea_telemetry::db_span("list_events"))
        .await?;
    // An empty page may mean no such chart: asked only then.
    if rows.is_empty()
        && let Some(chart) = &query.chart
    {
        let exists: bool = sqlx::query_scalar(CHART_EXISTS)
            .bind(auth.workspace().uuid())
            .bind(chart)
            .fetch_one(&state.db)
            .instrument(cavea_telemetry::db_span("chart_exists"))
            .await?;
        if !exists {
            return Err(ApiError::new(Code::NotFound)
                .with_message("No such chart.")
                .with_detail("missing", "chart"));
        }
    }
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
    #[serde(default, deserialize_with = "object")]
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
    Staff(auth): Staff,
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
                Reason::Archived,
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
                Reason::Unpublished,
                "The chart has no published version.",
            ));
        }
    };
    let settings_text = serde_json::to_string(&settings).map_err(ApiError::internal)?;
    let actor = auth.actor();
    let create = async {
        let mut tx = state.db.begin().await?;
        let row: Result<EventRow, _> = sqlx::query_as(CREATE)
            .bind(auth.workspace().uuid())
            .bind(&body.key)
            .bind(body.name.as_deref())
            .bind(&version)
            .bind(&settings_text)
            .fetch_one(&mut *tx)
            .await;
        let row = match row {
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Err(ApiError::new(Code::AlreadyExists)
                    .with_message("An event with this key already exists, possibly archived."));
            }
            row => row?,
        };
        let entry = Entry::by(&auth, &actor, "event.created").target(&body.key);
        audit::record(&mut tx, entry).await?;
        tx.commit().await?;
        Ok(row)
    };
    let row = create
        .instrument(cavea_telemetry::db_span("create_event"))
        .await?;
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

/// The event with its objects counted by status and its committed `seq`.
/// Counts are kept by each commit with the rows (`events.counts`), so they
/// are exactly what has been acknowledged, read without a scan and without
/// loading the event's actor. A hold past its expiry counts as held until
/// its expiry commits, which a loaded event does at once.
async fn read(
    Staff(auth): Staff,
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
    Ok(axum::Json(Event::new(row.ok_or_else(not_found)?)?))
}

/// `?after_seq=`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateQuery {
    #[serde(default)]
    after_seq: Option<u64>,
}

/// The staff counterpart of the public snapshot and polling (08 §8.3,
/// §8.4): manager frames, in domain statuses, read from the engine's
/// published state without a message to the actor (loading the event if it
/// is not loaded). Never cached: it is the staff's view.
async fn staff_state(
    Staff(auth): Staff,
    Path(key): Path<String>,
    Query(query): Query<StateQuery>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let target = target(&state, &auth, Find::Key(&key)).await?;
    let frame = state
        .seats
        .registry
        .manager_after(target.id, query.after_seq.unwrap_or(0))
        .await
        .map_err(|_| {
            ApiError::new(Code::Unavailable)
                .with_message("The event's state is unavailable; retry shortly.")
        })?;
    let mut response = match frame {
        Some(frame) => super::realtime::frame_body(frame),
        None => StatusCode::NO_CONTENT.into_response(),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateEvent {
    /// Left out: unchanged. `null`: cleared.
    #[serde(default, deserialize_with = "present")]
    name: Option<Option<String>>,
    /// Replaces the settings as a whole: a field left out takes its default.
    #[serde(default, deserialize_with = "optional_object")]
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
    Staff(auth): Staff,
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

/// Whether the event is unarchived, locking it: only an archive that changes
/// it is audited.
const LIVE_EVENT: &str = "SELECT archived_at IS NULL FROM events
 WHERE workspace_id = $1::uuid AND key = $2 FOR UPDATE";

/// Archive an event (08 §6.2): it becomes read-only. Seat operations, token
/// creation and changes are refused; outstanding holds are left to expire on
/// schedule; bookings stay; the loaded actor is left to evict itself once it
/// has no holds. The key stays taken.
async fn archive(
    Staff(auth): Staff,
    Path(key): Path<String>,
    State(state): State<AppState>,
) -> Result<axum::Json<Event>, ApiError> {
    cavea_telemetry::record_event(&key);
    let actor = auth.actor();
    let archive = async {
        let mut tx = state.db.begin().await?;
        let live: Option<bool> = sqlx::query_scalar(LIVE_EVENT)
            .bind(auth.workspace().uuid())
            .bind(&key)
            .fetch_optional(&mut *tx)
            .await?;
        let row: Option<EventRow> = sqlx::query_as(ARCHIVE)
            .bind(auth.workspace().uuid())
            .bind(&key)
            .fetch_optional(&mut *tx)
            .await?;
        if live == Some(true) {
            let entry = Entry::by(&auth, &actor, "event.archived").target(&key);
            audit::record(&mut tx, entry).await?;
        }
        tx.commit().await?;
        Ok::<_, sqlx::Error>(row)
    };
    let row = archive
        .instrument(cavea_telemetry::db_span("archive_event"))
        .await?;
    let event = Event::new(row.ok_or_else(not_found)?)?;
    tracing::info!("event archived");
    Ok(axum::Json(event))
}

#[cfg(test)]
mod tests {
    use super::instant;

    #[test]
    fn creation_times_parse_strictly() {
        let day = 20_513 * 86_400_000_000; // 2026-03-01
        assert_eq!(instant("2026-03-01"), Some(day));
        assert_eq!(instant("2026-03-01T00:00:00Z"), Some(day));
        assert_eq!(instant("2026-03-01T01:02:03Z"), Some(day + 3_723_000_000));
        assert_eq!(instant("2026-03-01T00:00:00.5Z"), Some(day + 500_000));
        assert_eq!(
            instant("2026-03-01T00:00:00.123456789Z"),
            Some(day + 123_456)
        );
        for bad in [
            "",
            "2026-02-30",
            "2026-3-01",
            "1969-12-31",
            "2026-03-01T00:00:00",
            "2026-03-01T00:00:00+01:00",
            "2026-03-01T24:00:00Z",
            "2026-03-01T00:60:00Z",
            "2026-03-01T0:00:00Z",
            "2026-03-01T00:00:00.Z",
            "2026-03-01T00:00:00.1234567890Z",
            "2026-03-01T+1:00:00Z",
            "2026-03-01 00:00:00Z",
        ] {
            assert_eq!(instant(bad), None, "{bad}");
        }
    }
}
