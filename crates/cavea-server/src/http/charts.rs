//! `/v1/charts`: create, list, read and archive charts, save and validate the
//! draft, publish versions (08 §6.2). Secret key; every query is scoped by the
//! workspace of the key. `{chart}` is the customer's chart key.

use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use cavea_compiler::{Finding, Severity};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json, value::RawValue};
use tracing::Instrument as _;

use super::auth::{Auth, KeyId};
use super::extract::{Json, Page, Path, Query, RawJson, check_text, invalid, invalid_field};
use super::openapi::{Needs, Op, Routes};
use super::staff::Staff;
use super::{ApiError, AppState, Code, List};
use crate::audit::{self, Entry};
use crate::charts::{self, ChartError, MAX_DRAFT_BYTES};
use crate::console::Permission;

pub(super) fn routes() -> Routes {
    // A create body is a draft plus a key and a name of at most 128
    // characters each, however they are escaped.
    let create_limit = DefaultBodyLimit::max(MAX_DRAFT_BYTES + 4096);
    let reading = |op: Op| op.needs(Needs::Staff(Permission::ChartsRead));
    let writing = |op: Op| op.needs(Needs::Staff(Permission::ChartsWrite));
    let draft = || {
        Some(serde_json::json!({
            "application/json": { "schema": { "$ref": "#/components/schemas/ChartDocument" } },
        }))
    };
    Routes::new()
        .add(
            reading(Op::get("/v1/charts", "listCharts", "List charts"))
                .about("Archived charts included; never drafts. The cursor is a chart key.")
                .page()
                .reply(200, "ChartList", "A page of charts"),
            list,
        )
        .layered(
            writing(Op::post("/v1/charts", "createChart", "Create a chart"))
                .body("CreateChart")
                .reply(201, "Chart", "The chart")
                .errors(&[409]),
            create,
            |route| route.layer(create_limit),
        )
        .add(
            reading(Op::get("/v1/charts/{chart}", "getChart", "A chart")).reply(
                200,
                "Chart",
                "The chart: metadata, draft etag, published version",
            ),
            read,
        )
        .add(
            Op::delete("/v1/charts/{chart}", "archiveChart", "Archive a chart")
                .needs(Needs::Staff(Permission::ChartsArchive))
                .about("Idempotent. Versions stay served.")
                .reply(200, "Chart", "The chart, archived"),
            archive,
        )
        .add(
            reading(Op::get(
                "/v1/charts/{chart}/draft",
                "getDraft",
                "The draft document",
            ))
            .response(200, "The draft, with its etag in `ETag`", draft())
            .etag(),
            read_draft,
        )
        .layered(
            writing(Op::put(
                "/v1/charts/{chart}/draft",
                "saveDraft",
                "Replace the draft",
            ))
            .about("At most 16 MiB. It must parse as a chart document; it need not validate.")
            .if_match(true)
            .raw_body(serde_json::json!({ "required": true, "content": draft() }))
            .reply(200, "Chart", "The chart, with the new etag also in `ETag`")
            .etag()
            .errors(&[409]),
            save_draft,
            |route| route.layer(DefaultBodyLimit::max(MAX_DRAFT_BYTES)),
        )
        .add(
            writing(Op::post(
                "/v1/charts/{chart}/validate",
                "validateDraft",
                "Validate the draft",
            ))
            .reply(200, "Validation", "The compiler's findings")
            .errors(&[503]),
            validate,
        )
        .add(
            writing(Op::post(
                "/v1/charts/{chart}/publish",
                "publishChart",
                "Publish the draft",
            ))
            .if_match(false)
            .reply(201, "Version", "A new version")
            .reply(
                200,
                "Version",
                "The existing version this draft compiles to",
            )
            .errors(&[409, 422, 503]),
            publish,
        )
        .add(
            reading(Op::get(
                "/v1/charts/{chart}/versions",
                "listVersions",
                "List versions",
            ))
            .page()
            .reply(200, "VersionList", "A page of versions, oldest first")
            .about(
                "Each version names who published it (`created_by`) and, for a console member \
whose role may see the workspace's members, their email (`created_by_email`).",
            ),
            versions,
        )
        .add(
            writing(Op::put(
                "/v1/charts/{chart}/versions/{number}/thumbnail",
                "setVersionThumbnail",
                "Attach a thumbnail to a version",
            ))
            .about(
                "`asset` is the hash of an image this workspace uploaded (`POST /v1/assets`). \
Replaces any thumbnail the version had.",
            )
            .body("SetThumbnail")
            .reply(200, "Version", "The version, with its thumbnail")
            .errors(&[404]),
            set_thumbnail,
        )
        .add(
            reading(Op::get(
                "/v1/charts/{chart}/lock",
                "getChartLock",
                "Who is editing the chart",
            ))
            .about("07 §9: the designer's editing lock, or `null` when nobody holds a live one.")
            .reply(200, "ChartLockState", "The lock"),
            read_lock,
        )
        .add(
            writing(Op::post(
                "/v1/charts/{chart}/lock",
                "lockChart",
                "Take, renew or take over the editing lock",
            ))
            .about(
                "07 §9. Takes the lock, or renews it for its holder, for 60 seconds by the \
database's clock; renew every 20. Held by another and live: `409 chart_locked` with the lock in \
`details.lock`, unless `take_over` is true. While another holds a live lock, saving the draft and \
publishing are `409 chart_locked`.",
            )
            .body("LockChart")
            .reply(200, "ChartLock", "The lock, now the caller's")
            .errors(&[409]),
            lock,
        )
        .add(
            writing(Op::delete(
                "/v1/charts/{chart}/lock",
                "unlockChart",
                "Release the editing lock",
            ))
            .about("Releases the caller's lock; another's is left as it is. Idempotent.")
            .response(204, "Released, or not the caller's", None),
            unlock,
        )
}

/// `chart_versions` columns in [`VersionRow`] order, for a table aliased `v`.
macro_rules! version_columns {
    () => {
        concat!(
            "replace(v.id::text, '-', ''), v.number, encode(v.hash, 'hex'), v.stats::text, ",
            rfc3339!("v", ".created_at"),
            ", encode(v.thumbnail, 'hex'), CASE
  WHEN v.created_by IS NOT NULL THEN 'usr_' || replace(v.created_by::text, '-', '')
  WHEN v.created_by_key IS NOT NULL THEN 'key_' || replace(v.created_by_key::text, '-', '')
END"
        )
    };
}

/// `charts` columns aliased `c`, then its published version's, in [`ChartRow`]
/// order. Join the version as
/// `LEFT JOIN chart_versions v ON v.id = c.published_version_id`.
macro_rules! chart_columns {
    () => {
        concat!(
            "c.key, c.name, c.draft_etag, ",
            rfc3339!("c", ".draft_updated_at"),
            ", ",
            rfc3339!("c", ".archived_at"),
            ", ",
            rfc3339!("c", ".created_at"),
            ", ",
            version_columns!()
        )
    };
}

/// The lock's holder, the holder's email while a member, and its times;
/// `NULL` when it has expired. For a chart aliased `c` joined with
/// [`lock_joins`].
macro_rules! lock_columns {
    () => {
        concat!(
            "CASE WHEN c.lock_expires_at > now() THEN c.lock_actor END, u.email, ",
            rfc3339!("c", ".lock_since"),
            ", ",
            rfc3339!("c", ".lock_expires_at")
        )
    };
}

macro_rules! lock_joins {
    () => {
        "LEFT JOIN memberships m ON m.workspace_id = c.workspace_id
           AND m.user_id = CASE WHEN left(c.lock_actor, 4) = 'usr_'
                                THEN substr(c.lock_actor, 5)::uuid END
         LEFT JOIN users u ON u.id = m.user_id"
    };
}

pub(super) type VersionRow = (
    String,
    i32,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
);

type ChartRow = (
    String,
    String,
    i64,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<i32>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// A published chart version as the API shows it.
#[derive(Debug, Serialize)]
pub(super) struct Version {
    /// `cv_` and 32 hex digits.
    id: String,
    number: i32,
    /// The public artefact's content hash: 32 hex digits.
    hash: String,
    /// Where the compiled chart is served, relative to the server's base URL.
    url: String,
    stats: Value,
    created_at: String,
    /// The image the publisher attached (D-49), or `None`.
    thumbnail: Option<Thumbnail>,
    /// Who published it: `usr_…` or `key_…`; `None` for a version published
    /// before this was recorded with an API key.
    created_by: Option<String>,
    /// The publisher's email: only in the versions list, and `null` there
    /// unless the caller may see the workspace's members and the publisher
    /// is still one.
    #[serde(skip_serializing_if = "Option::is_none")]
    created_by_email: Option<Option<String>>,
}

/// A version's thumbnail: an asset, as `POST /v1/assets` returns it.
#[derive(Debug, Serialize)]
struct Thumbnail {
    hash: String,
    url: String,
}

impl Version {
    pub(super) fn new(row: VersionRow) -> Result<Self, ApiError> {
        let (id, number, hash, stats, created_at, thumbnail, created_by) = row;
        Ok(Self {
            id: format!("cv_{id}"),
            number,
            url: format!("/v1/public/charts/{hash}.ccf"),
            hash,
            stats: serde_json::from_str(&stats).map_err(ApiError::internal)?,
            created_at,
            thumbnail: thumbnail.map(|hash| Thumbnail {
                url: format!("/v1/public/assets/{hash}"),
                hash,
            }),
            created_by,
            created_by_email: None,
        })
    }

    /// The version's UUID as 32 hex digits.
    pub(super) fn uuid(&self) -> &str {
        self.id.strip_prefix("cv_").unwrap_or(&self.id)
    }

    /// How many objects the version has (`stats.seats`: every object is a
    /// seat in this chart format).
    pub(super) fn objects(&self) -> Result<i64, ApiError> {
        self.stats
            .get("seats")
            .and_then(Value::as_i64)
            .ok_or_else(|| ApiError::internal("version stats without seats"))
    }
}

/// A chart as the API shows it; never with its draft.
#[derive(Debug, Serialize)]
struct Chart {
    key: String,
    name: String,
    /// Pass back verbatim in `If-Match`.
    draft_etag: String,
    draft_updated_at: String,
    published_version: Option<Version>,
    archived_at: Option<String>,
    created_at: String,
}

impl Chart {
    fn new(row: ChartRow) -> Result<Self, ApiError> {
        let (
            key,
            name,
            etag,
            draft_updated_at,
            archived_at,
            created_at,
            v0,
            v1,
            v2,
            v3,
            v4,
            v5,
            v6,
        ) = row;
        let published_version = match (v0, v1, v2, v3, v4) {
            (Some(v0), Some(v1), Some(v2), Some(v3), Some(v4)) => {
                Some(Version::new((v0, v1, v2, v3, v4, v5, v6))?)
            }
            _ => None,
        };
        Ok(Self {
            key,
            name,
            draft_etag: etag_text(etag),
            draft_updated_at,
            published_version,
            archived_at,
            created_at,
        })
    }
}

fn etag_text(etag: i64) -> String {
    format!("\"{etag}\"")
}

fn not_found() -> ApiError {
    ApiError::new(Code::NotFound).with_message("No such chart.")
}

fn archived() -> ApiError {
    ApiError::new(Code::InvalidRequest)
        .with_message("The chart is archived: its draft can no longer change.")
        .with_detail("reason", "archived")
}

/// Another holds a live editing lock (07 §9).
fn locked(lock: Lock) -> ApiError {
    ApiError::new(Code::ChartLocked)
        .with_detail("lock", serde_json::to_value(lock).unwrap_or_default())
}

fn version_conflict(current: i64) -> ApiError {
    ApiError::new(Code::VersionConflict).with_detail("draft_etag", etag_text(current))
}

impl From<ChartError> for ApiError {
    fn from(error: ChartError) -> Self {
        match error {
            ChartError::Document(message) => ApiError::new(Code::InvalidRequest)
                .with_message(message)
                .with_detail("reason", "document"),
            ChartError::Schema(schema) => ApiError::new(Code::InvalidRequest)
                .with_message(format!("Chart document schema {schema} is not supported."))
                .with_detail("reason", "document"),
            ChartError::Nul => ApiError::new(Code::InvalidRequest)
                .with_message("The document holds U+0000, which cannot be stored.")
                .with_detail("reason", "document"),
            ChartError::Invalid(findings) => {
                // The compiler's documented form (03 §3.2); it cannot fail.
                let findings = serde_json::to_value(findings).unwrap_or_default();
                ApiError::new(Code::ChartInvalid).with_detail("findings", findings)
            }
            ChartError::Timeout => ApiError::new(Code::Overloaded)
                .with_message("The chart could not be compiled in time; retry shortly.")
                .with_detail("reason", "compile_timeout"),
            ChartError::Internal(cause) => ApiError::internal(cause),
        }
    }
}

/// `If-Match`: `*`, or one strong entity tag as `draft_etag` gives it.
#[derive(Debug, Clone, Copy)]
enum IfMatch {
    Any,
    Etag(i64),
}

fn if_match(headers: &HeaderMap) -> Result<Option<IfMatch>, ApiError> {
    let Some(value) = headers.get(header::IF_MATCH) else {
        return Ok(None);
    };
    let value = value.to_str().unwrap_or_default().trim();
    if value == "*" {
        return Ok(Some(IfMatch::Any));
    }
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|v| v.parse().ok())
        .map(|etag| Some(IfMatch::Etag(etag)))
        .ok_or_else(|| {
            invalid(
                "if_match",
                "`If-Match` must be a chart's `draft_etag`, such as \"3\", or `*`.",
            )
        })
}

fn matches(if_match: Option<IfMatch>, current: i64) -> bool {
    match if_match {
        None | Some(IfMatch::Any) => true,
        Some(IfMatch::Etag(etag)) => etag == current,
    }
}

// ---- list, create, read, archive ----

/// Oldest first; archived charts included. `after` is a chart key.
const LIST: &str = concat!(
    "SELECT ",
    chart_columns!(),
    " FROM charts c LEFT JOIN chart_versions v ON v.id = c.published_version_id
 WHERE c.workspace_id = $1::uuid
   AND ($2::text IS NULL OR (c.created_at, c.id) >
        (SELECT created_at, id FROM charts WHERE workspace_id = $1::uuid AND key = $2))
 ORDER BY c.created_at, c.id
 LIMIT $3"
);

async fn list(
    Staff(auth): Staff,
    Query(page): Query<Page>,
    State(state): State<AppState>,
) -> Result<axum::Json<List<Chart>>, ApiError> {
    let limit = page.limit()?;
    let rows: Vec<ChartRow> = sqlx::query_as(LIST)
        .bind(auth.workspace().uuid())
        .bind(page.after.as_deref())
        .bind(i64::from(limit) + 1)
        .fetch_all(&state.db)
        .instrument(cavea_telemetry::db_span("list_charts"))
        .await?;
    let charts = rows
        .into_iter()
        .map(Chart::new)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(axum::Json(List::new(charts, limit, |c| c.key.clone())))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateChart {
    key: String,
    name: String,
    /// The first draft; without it, an empty chart document named `name`.
    #[serde(default)]
    document: Option<Box<RawValue>>,
}

const CREATE: &str = concat!(
    "WITH c AS (
  INSERT INTO charts (id, workspace_id, key, name, draft, draft_updated_by)
  VALUES (gen_random_uuid(), $1::uuid, $2, $3, $4::jsonb, $5::uuid)
  RETURNING *
)
SELECT ",
    chart_columns!(),
    " FROM c LEFT JOIN chart_versions v ON v.id = c.published_version_id"
);

async fn create(
    Staff(auth): Staff,
    State(state): State<AppState>,
    Json(body): Json<CreateChart>,
) -> Result<(StatusCode, axum::Json<Chart>), ApiError> {
    check_text("key", &body.key)?;
    check_text("name", &body.name)?;
    let draft = match body.document {
        Some(document) => {
            let draft = String::from(Box::<str>::from(document));
            if draft.len() > MAX_DRAFT_BYTES {
                return Err(invalid(
                    "body_too_large",
                    "The body is over the size limit.",
                ));
            }
            charts::check_draft(draft.clone().into()).await?;
            draft
        }
        // One category, as the designer's "draw from scratch" (07 §16): every
        // object needs one (02, E002), and a chart with none compiles no
        // seat, so the first block drawn would not show.
        None => json!({
            "schema": cavea_compiler::SCHEMA,
            "name": body.name,
            "nextOid": 1,
            "categories": [
                { "key": "standard", "label": "Standard", "color": "#2563EB", "accessible": false }
            ],
        })
        .to_string(),
    };
    let row: Result<ChartRow, _> = sqlx::query_as(CREATE)
        .bind(auth.workspace().uuid())
        .bind(&body.key)
        .bind(&body.name)
        .bind(&draft)
        .bind(auth.user())
        .fetch_one(&state.db)
        .instrument(cavea_telemetry::db_span("create_chart"))
        .await;
    let row = match row {
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(ApiError::new(Code::AlreadyExists)
                .with_message("A chart with this key already exists, possibly archived."));
        }
        row => row?,
    };
    tracing::info!(chart = %body.key, draft_bytes = draft.len(), "chart created");
    Ok((StatusCode::CREATED, axum::Json(Chart::new(row)?)))
}

const READ: &str = concat!(
    "SELECT ",
    chart_columns!(),
    " FROM charts c LEFT JOIN chart_versions v ON v.id = c.published_version_id
 WHERE c.workspace_id = $1::uuid AND c.key = $2"
);

async fn read(
    Staff(auth): Staff,
    Path(key): Path<String>,
    State(state): State<AppState>,
) -> Result<axum::Json<Chart>, ApiError> {
    let row: Option<ChartRow> = sqlx::query_as(READ)
        .bind(auth.workspace().uuid())
        .bind(&key)
        .fetch_optional(&state.db)
        .instrument(cavea_telemetry::db_span("read_chart"))
        .await?;
    Ok(axum::Json(Chart::new(row.ok_or_else(not_found)?)?))
}

/// Idempotent: archiving an archived chart returns it unchanged.
const ARCHIVE: &str = concat!(
    "WITH c AS (
  UPDATE charts SET archived_at = coalesce(archived_at, now())
   WHERE workspace_id = $1::uuid AND key = $2
  RETURNING *
)
SELECT ",
    chart_columns!(),
    " FROM c LEFT JOIN chart_versions v ON v.id = c.published_version_id"
);

/// Whether the chart is unarchived, locking it: only an archive that changes
/// it is audited.
const LIVE_CHART: &str = "SELECT archived_at IS NULL FROM charts
 WHERE workspace_id = $1::uuid AND key = $2 FOR UPDATE";

/// Archive a chart: its draft can no longer be saved or published, its key
/// stays taken, and its versions and their public URLs stay as they are
/// (events pinned to them keep working; 08 §6.2).
async fn archive(
    Staff(auth): Staff,
    Path(key): Path<String>,
    State(state): State<AppState>,
) -> Result<axum::Json<Chart>, ApiError> {
    let actor = auth.actor();
    let archive = async {
        let mut tx = state.db.begin().await?;
        let live: Option<bool> = sqlx::query_scalar(LIVE_CHART)
            .bind(auth.workspace().uuid())
            .bind(&key)
            .fetch_optional(&mut *tx)
            .await?;
        let row: Option<ChartRow> = sqlx::query_as(ARCHIVE)
            .bind(auth.workspace().uuid())
            .bind(&key)
            .fetch_optional(&mut *tx)
            .await?;
        if live == Some(true) {
            let entry = Entry::by(&auth, &actor, "chart.archived").target(&key);
            audit::record(&mut tx, entry).await?;
        }
        tx.commit().await?;
        Ok::<_, sqlx::Error>(row)
    };
    let row = archive
        .instrument(cavea_telemetry::db_span("archive_chart"))
        .await?;
    let chart = Chart::new(row.ok_or_else(not_found)?)?;
    tracing::info!(chart = %key, "chart archived");
    Ok(axum::Json(chart))
}

// ---- the draft ----

const READ_DRAFT: &str = "SELECT draft::text, draft_etag FROM charts
 WHERE workspace_id = $1::uuid AND key = $2";

/// The draft document as stored, with its etag in `ETag`.
async fn read_draft(
    Staff(auth): Staff,
    Path(key): Path<String>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let row: Option<(String, i64)> = sqlx::query_as(READ_DRAFT)
        .bind(auth.workspace().uuid())
        .bind(&key)
        .fetch_optional(&state.db)
        .instrument(cavea_telemetry::db_span("read_draft"))
        .await?;
    let (draft, etag) = row.ok_or_else(not_found)?;
    let headers = [
        (
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        ),
        (header::ETAG, etag_header(etag)?),
    ];
    Ok((headers, draft).into_response())
}

fn etag_header(etag: i64) -> Result<HeaderValue, ApiError> {
    HeaderValue::try_from(etag_text(etag)).map_err(ApiError::internal)
}

/// Replace the draft if the etag matches (any etag for `If-Match: *`).
const SAVE_DRAFT: &str = concat!(
    "WITH c AS (
  UPDATE charts
     SET draft = $3::jsonb, draft_etag = draft_etag + 1, draft_updated_at = now(),
         draft_updated_by = $5::uuid
   WHERE workspace_id = $1::uuid AND key = $2 AND archived_at IS NULL
     AND ($4::bigint IS NULL OR draft_etag = $4)
     AND (lock_actor IS NULL OR lock_actor = $6 OR lock_expires_at <= now())
  RETURNING *
)
SELECT ",
    chart_columns!(),
    " FROM c LEFT JOIN chart_versions v ON v.id = c.published_version_id"
);

/// Why a draft could not be replaced: its etag, whether it is archived, and
/// a live lock held by another than `$3`.
const DRAFT_STATE: &str = concat!(
    "SELECT c.draft_etag, c.archived_at IS NOT NULL, ",
    lock_columns!(),
    " FROM charts c ",
    lock_joins!(),
    " WHERE c.workspace_id = $1::uuid AND c.key = $2"
);

/// Replace the draft. The body is the chart document; it must parse as one
/// (it need not validate) and `If-Match` is required.
async fn save_draft(
    Staff(auth): Staff,
    Path(key): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
    RawJson(body): RawJson,
) -> Result<Response, ApiError> {
    let expected = if_match(&headers)?.ok_or_else(|| {
        invalid(
            "if_match",
            "`If-Match` is required: the chart's `draft_etag`, or `*` to overwrite any draft.",
        )
    })?;
    let size = body.len();
    charts::check_draft(body.clone()).await?;
    let draft = std::str::from_utf8(&body).map_err(ApiError::internal)?;
    let actor = auth.actor();
    let etag = match expected {
        IfMatch::Any => None,
        IfMatch::Etag(etag) => Some(etag),
    };
    let save = async {
        let row: Option<ChartRow> = sqlx::query_as(SAVE_DRAFT)
            .bind(auth.workspace().uuid())
            .bind(&key)
            .bind(draft)
            .bind(etag)
            .bind(auth.user())
            .bind(&*actor)
            .fetch_optional(&state.db)
            .await?;
        if let Some(row) = row {
            return Ok(row);
        }
        let current: Option<DraftState> = sqlx::query_as(DRAFT_STATE)
            .bind(auth.workspace().uuid())
            .bind(&key)
            .fetch_optional(&state.db)
            .await?;
        let Some((current, is_archived, a, e, since, until)) = current else {
            return Err(not_found());
        };
        if is_archived {
            return Err(archived());
        }
        match Lock::live(&auth, a, e, since, until) {
            Some(lock) if lock.holder != *actor => Err(locked(lock)),
            _ => Err(version_conflict(current)),
        }
    };
    let row = save
        .instrument(cavea_telemetry::db_span("save_draft"))
        .await?;
    let chart = Chart::new(row)?;
    tracing::info!(chart = %key, draft_bytes = size, draft_etag = %chart.draft_etag, "draft saved");
    let etag = HeaderValue::try_from(&chart.draft_etag).map_err(ApiError::internal)?;
    Ok(([(header::ETAG, etag)], axum::Json(chart)).into_response())
}

// ---- validate and publish ----

const DRAFT_FOR_COMPILE: &str = "SELECT replace(id::text, '-', ''), draft::text, draft_etag,
       archived_at IS NOT NULL
  FROM charts WHERE workspace_id = $1::uuid AND key = $2";

type DraftRow = (String, String, i64, bool);

/// The draft's etag, whether it is archived, and its lock's columns.
type DraftState = (
    i64,
    bool,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

async fn draft_for_compile(
    state: &AppState,
    auth: &super::Auth,
    key: &str,
) -> Result<DraftRow, ApiError> {
    let row: Option<DraftRow> = sqlx::query_as(DRAFT_FOR_COMPILE)
        .bind(auth.workspace().uuid())
        .bind(key)
        .fetch_optional(&state.db)
        .instrument(cavea_telemetry::db_span("read_draft"))
        .await?;
    row.ok_or_else(not_found)
}

#[derive(Debug, Serialize)]
struct Validation {
    /// The draft that was validated.
    draft_etag: String,
    /// No finding has error severity: the draft can be published.
    valid: bool,
    /// In the compiler's documented form (03 §3.2).
    findings: Vec<Value>,
    /// The compiled chart's statistics, when it compiled.
    stats: Option<Value>,
}

/// Compile the draft and report its findings without publishing.
async fn validate(
    Staff(auth): Staff,
    Path(key): Path<String>,
    State(state): State<AppState>,
) -> Result<axum::Json<Validation>, ApiError> {
    let (_, draft, etag, _) = draft_for_compile(&state, &auth, &key).await?;
    let validation = match charts::compile(draft, false).await {
        Ok(output) => {
            let findings = unknown_assets(&state, &auth, &output).await?;
            Validation {
                draft_etag: etag_text(etag),
                valid: findings.is_empty(),
                findings,
                stats: Some(stats_json(&output)),
            }
        }
        Err(ChartError::Invalid(findings)) => Validation {
            draft_etag: etag_text(etag),
            valid: !findings.iter().any(|f| f.severity() == Severity::Error),
            findings: findings.iter().map(finding_json).collect(),
            stats: None,
        },
        Err(e) => return Err(e.into()),
    };
    Ok(axum::Json(validation))
}

fn finding_json(finding: &Finding) -> Value {
    // The compiler's documented form (03 §3.2); it cannot fail.
    serde_json::to_value(finding).unwrap_or_default()
}

/// E007 for each background image whose asset the workspace does not hold
/// (03 §2.7): the compiler holds no assets, so the server checks. The message
/// names no hash (03 §3.2).
async fn unknown_assets(
    state: &AppState,
    auth: &Auth,
    output: &charts::Output,
) -> Result<Vec<Value>, ApiError> {
    if output.images.is_empty() {
        return Ok(Vec::new());
    }
    let hashes: Vec<String> = output.images.iter().map(|(_, a)| a.clone()).collect();
    let held = crate::assets::held(&state.db, auth.workspace().uuid(), &hashes).await?;
    Ok(output
        .images
        .iter()
        .filter(|(_, asset)| !held.contains(asset))
        .map(|(node, _)| {
            json!({
                "code": "E007", "severity": "error", "node": node, "related": null,
                "message": "The image's asset is not one this workspace has uploaded.",
            })
        })
        .collect())
}

fn stats_json(output: &charts::Output) -> Value {
    let s = &output.compiled.stats;
    json!({
        "seats": s.seats,
        "rows": s.rows,
        "segments": s.segments,
        "sections": s.sections,
        "floors": s.floors,
        "public_bytes": s.public_bytes,
        "public_br_bytes": output.public_br.as_ref().map(Vec::len),
        "full_bytes": s.full_bytes,
    })
}

const LOCK_CHART: &str = concat!(
    "SELECT c.draft_etag, c.archived_at IS NOT NULL, ",
    lock_columns!(),
    " FROM charts c ",
    lock_joins!(),
    " WHERE c.id = $1::uuid FOR UPDATE OF c"
);

/// A version of this chart compiled from the same document: the same full
/// artefact.
const SAME_VERSION: &str = concat!(
    "SELECT ",
    version_columns!(),
    " FROM chart_versions v WHERE v.chart_id = $1::uuid AND v.full_hash = $2"
);

/// The next number, and the draft itself as the source: the caller has the
/// chart locked and has checked that the draft is the one it compiled.
const INSERT_VERSION: &str = concat!(
    "WITH v AS (
  INSERT INTO chart_versions
    (id, chart_id, number, hash, full_hash, source, public_ccf, public_ccf_br, full_ccf, stats,
     created_by, created_by_key)
  SELECT gen_random_uuid(), c.id,
         coalesce((SELECT max(number) FROM chart_versions WHERE chart_id = c.id), 0) + 1,
         $2, $3, c.draft, $4, $5, $6, $7::jsonb, $8::uuid, $9::uuid
    FROM charts c WHERE c.id = $1::uuid
  RETURNING *
)
SELECT ",
    version_columns!(),
    " FROM v"
);

const SET_PUBLISHED: &str = "UPDATE charts SET published_version_id = $2::uuid WHERE id = $1::uuid";

/// Compile the current draft and publish it as a new immutable version (08
/// §6.2). `If-Match` is optional: with it, the draft must still be that one.
/// A draft that compiles to a version the chart already has publishes that
/// version again (`200`) rather than a duplicate (`201` for a new one).
async fn publish(
    Staff(auth): Staff,
    Path(key): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<(StatusCode, axum::Json<Version>), ApiError> {
    let expected = if_match(&headers)?;
    let actor = auth.actor();
    let (chart_id, draft, etag, is_archived) = draft_for_compile(&state, &auth, &key).await?;
    if is_archived {
        return Err(archived());
    }
    // The lock before the etag, as for a save: a taken-over editor learns who
    // took over, whatever its etag. Checked again once the chart is locked.
    let lock: Option<DraftState> = sqlx::query_as(DRAFT_STATE)
        .bind(auth.workspace().uuid())
        .bind(&key)
        .fetch_optional(&state.db)
        .instrument(cavea_telemetry::db_span("read_lock"))
        .await?;
    if let Some((_, _, a, e, since, until)) = lock
        && let Some(lock) = Lock::live(&auth, a, e, since, until)
        && lock.holder != *actor
    {
        return Err(locked(lock));
    }
    if !matches(expected, etag) {
        return Err(version_conflict(etag));
    }
    let output = charts::compile(draft, true).await?;
    let unknown = unknown_assets(&state, &auth, &output).await?;
    if !unknown.is_empty() {
        return Err(ApiError::new(Code::ChartInvalid).with_detail("findings", unknown));
    }
    let public_br = output.public_br.as_deref().unwrap_or_default();
    let stats = stats_json(&output);
    let compiled = &output.compiled;
    let started = std::time::Instant::now();
    let store = async {
        let mut tx = state.db.begin().await?;
        let (current, is_archived, a, e, since, until): DraftState = sqlx::query_as(LOCK_CHART)
            .bind(&chart_id)
            .fetch_one(&mut *tx)
            .await?;
        if is_archived {
            // Archived while compiling.
            return Err(archived());
        }
        if let Some(lock) = Lock::live(&auth, a, e, since, until)
            && lock.holder != *actor
        {
            // Another took the lock: the draft is theirs to publish.
            return Err(locked(lock));
        }
        if current != etag {
            // Saved while compiling: what was compiled is no longer the draft.
            return Err(version_conflict(current)
                .with_message("The draft changed while it was being published."));
        }
        let same: Option<VersionRow> = sqlx::query_as(SAME_VERSION)
            .bind(&chart_id)
            .bind(compiled.full_hash.as_slice())
            .fetch_optional(&mut *tx)
            .await?;
        let (created, row) = match same {
            Some(row) => (false, row),
            None => {
                let row: VersionRow = sqlx::query_as(INSERT_VERSION)
                    .bind(&chart_id)
                    .bind(compiled.public_hash.as_slice())
                    .bind(compiled.full_hash.as_slice())
                    .bind(&compiled.public)
                    .bind(public_br)
                    .bind(&compiled.full)
                    .bind(stats.to_string())
                    .bind(auth.user())
                    .bind(auth.key_id().map(KeyId::uuid))
                    .fetch_one(&mut *tx)
                    .await?;
                (true, row)
            }
        };
        sqlx::query(SET_PUBLISHED)
            .bind(&chart_id)
            .bind(&row.0)
            .execute(&mut *tx)
            .await?;
        let entry = Entry::by(&auth, &actor, "chart.published")
            .target(&key)
            .detail(json!({ "version": format!("cv_{}", row.0), "created": created }));
        audit::record(&mut tx, entry).await?;
        tx.commit().await?;
        Ok((created, row))
    };
    let (created, row) = store
        .instrument(cavea_telemetry::db_span("publish_chart"))
        .await?;
    let version = Version::new(row)?;
    tracing::info!(
        chart = %key,
        chart_version = %version.id,
        number = version.number,
        hash = %version.hash,
        created,
        seats = compiled.stats.seats,
        public_bytes = compiled.public.len(),
        public_br_bytes = public_br.len(),
        full_bytes = compiled.full.len(),
        compile_ms = output.compile_time.as_secs_f64() * 1e3,
        compress_ms = output.compress_time.as_secs_f64() * 1e3,
        store_ms = started.elapsed().as_secs_f64() * 1e3,
        "chart published"
    );
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, axum::Json(version)))
}

// ---- versions ----

/// Oldest first. `after` is a version id. With each, the publisher's email
/// while they are a member of the workspace.
const VERSIONS: &str = concat!(
    "SELECT ",
    version_columns!(),
    ", u.email FROM chart_versions v JOIN charts c ON c.id = v.chart_id
  LEFT JOIN memberships m ON m.workspace_id = c.workspace_id AND m.user_id = v.created_by
  LEFT JOIN users u ON u.id = m.user_id
 WHERE c.workspace_id = $1::uuid AND c.key = $2
   AND ($3::uuid IS NULL OR v.number >
        (SELECT number FROM chart_versions WHERE id = $3::uuid AND chart_id = c.id))
 ORDER BY v.number
 LIMIT $4"
);

const CHART_EXISTS: &str =
    "SELECT EXISTS (SELECT FROM charts WHERE workspace_id = $1::uuid AND key = $2)";

async fn versions(
    Staff(auth): Staff,
    Path(key): Path<String>,
    Query(page): Query<Page>,
    State(state): State<AppState>,
) -> Result<axum::Json<List<Version>>, ApiError> {
    let limit = page.limit()?;
    let after = match page.after.as_deref() {
        None => None,
        Some(after) => Some(version_uuid(after).ok_or_else(|| {
            ApiError::new(Code::InvalidRequest)
                .with_message("`after` is not a chart version id.")
                .with_detail("reason", "query")
        })?),
    };
    let list = async {
        let rows: Vec<VersionEmailRow> = sqlx::query_as(VERSIONS)
            .bind(auth.workspace().uuid())
            .bind(&key)
            .bind(after)
            .bind(i64::from(limit) + 1)
            .fetch_all(&state.db)
            .await?;
        if rows.is_empty() {
            let exists: bool = sqlx::query_scalar(CHART_EXISTS)
                .bind(auth.workspace().uuid())
                .bind(&key)
                .fetch_one(&state.db)
                .await?;
            if !exists {
                return Err(not_found());
            }
        }
        Ok(rows)
    };
    let rows = list
        .instrument(cavea_telemetry::db_span("list_versions"))
        .await?;
    // Members' emails are for those who may see the members (09 §2).
    let emails = auth.role().is_some_and(|r| r.allows(Permission::Settings));
    let versions = rows
        .into_iter()
        .map(|(v0, v1, v2, v3, v4, v5, v6, email)| {
            let mut version = Version::new((v0, v1, v2, v3, v4, v5, v6))?;
            version.created_by_email = Some(email.filter(|_| emails));
            Ok(version)
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(axum::Json(List::new(versions, limit, |v| v.id.clone())))
}

type VersionEmailRow = (
    String,
    i32,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetThumbnail {
    /// An asset's hash: 32 lowercase hex digits.
    asset: String,
}

const SET_THUMBNAIL: &str = concat!(
    "WITH v AS (
  UPDATE chart_versions v SET thumbnail = $4
    FROM charts c
   WHERE c.workspace_id = $1::uuid AND c.key = $2 AND v.chart_id = c.id AND v.number = $3
     AND EXISTS (SELECT FROM assets WHERE workspace_id = $1::uuid AND hash = $4)
  RETURNING v.*
)
SELECT ",
    version_columns!(),
    " FROM v"
);

/// Whether the chart exists, and its version numbered `$3`.
const VERSION_EXISTS: &str = "SELECT count(c.id) > 0, count(v.id) > 0
  FROM charts c LEFT JOIN chart_versions v ON v.chart_id = c.id AND v.number = $3
 WHERE c.workspace_id = $1::uuid AND c.key = $2";

/// Attach an uploaded image to a version as its thumbnail (09 §3.1, 07 §11
/// step 4). Versions are immutable but for this: the image is not part of
/// what was compiled.
async fn set_thumbnail(
    Staff(auth): Staff,
    Path((key, number)): Path<(String, String)>,
    State(state): State<AppState>,
    Json(body): Json<SetThumbnail>,
) -> Result<axum::Json<Version>, ApiError> {
    let no_version = || {
        ApiError::new(Code::NotFound)
            .with_message("The chart has no version with this number.")
            .with_detail("missing", "version")
    };
    let number: i32 = number.parse().map_err(|_| no_version())?;
    let hash = super::parse_hash(&body.asset)
        .ok_or_else(|| invalid_field("asset", "must be an asset's hash: 32 hex digits"))?;
    let set = async {
        let row: Option<VersionRow> = sqlx::query_as(SET_THUMBNAIL)
            .bind(auth.workspace().uuid())
            .bind(&key)
            .bind(number)
            .bind(hash.as_slice())
            .fetch_optional(&state.db)
            .await?;
        if let Some(row) = row {
            return Ok(row);
        }
        let (chart, version): (bool, bool) = sqlx::query_as(VERSION_EXISTS)
            .bind(auth.workspace().uuid())
            .bind(&key)
            .bind(number)
            .fetch_one(&state.db)
            .await?;
        Err(match (chart, version) {
            (false, _) => not_found(),
            (true, false) => no_version(),
            (true, true) => invalid_field("asset", "is not an image this workspace has uploaded"),
        })
    };
    let row = set
        .instrument(cavea_telemetry::db_span("set_thumbnail"))
        .await?;
    let version = Version::new(row)?;
    tracing::info!(chart = %key, chart_version = %version.id, "thumbnail set");
    Ok(axum::Json(version))
}

// ---- the editing lock (07 §9) ----

/// A lock lives this long unless its holder renews it.
const LOCK_SECS: i32 = 60;

/// The designer's editing lock: who holds it and until when.
#[derive(Debug, Serialize)]
struct Lock {
    /// `usr_…`, or `key_…` for an API key.
    holder: String,
    /// The holder's email, for a caller who may edit charts in the console;
    /// otherwise, or for a key, `null`.
    email: Option<String>,
    since: String,
    expires_at: String,
}

impl Lock {
    /// The lock if it is live, as `auth` may see it.
    fn live(
        auth: &Auth,
        holder: Option<String>,
        email: Option<String>,
        since: Option<String>,
        expires_at: Option<String>,
    ) -> Option<Self> {
        let (holder, since, expires_at) = (holder?, since?, expires_at?);
        let emails = auth
            .role()
            .is_some_and(|r| r.allows(Permission::ChartsWrite));
        Some(Self {
            holder,
            email: email.filter(|_| emails),
            since,
            expires_at,
        })
    }
}

type LockRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// Whether the chart is archived, and its lock as [`LockRow`].
type LockState = (
    bool,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

const READ_LOCK: &str = concat!(
    "SELECT c.archived_at IS NOT NULL, ",
    lock_columns!(),
    " FROM charts c ",
    lock_joins!(),
    " WHERE c.workspace_id = $1::uuid AND c.key = $2"
);

#[derive(Debug, Serialize)]
struct LockReply {
    lock: Option<Lock>,
}

async fn read_lock(
    Staff(auth): Staff,
    Path(key): Path<String>,
    State(state): State<AppState>,
) -> Result<axum::Json<LockReply>, ApiError> {
    let row: Option<LockState> = sqlx::query_as(READ_LOCK)
        .bind(auth.workspace().uuid())
        .bind(&key)
        .fetch_optional(&state.db)
        .instrument(cavea_telemetry::db_span("read_lock"))
        .await?;
    let (_, a, e, since, until) = row.ok_or_else(not_found)?;
    Ok(axum::Json(LockReply {
        lock: Lock::live(&auth, a, e, since, until),
    }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockChart {
    /// Take the lock from another who holds a live one.
    #[serde(default)]
    take_over: bool,
}

/// Take the lock if it is free, expired, the caller's already (a renewal,
/// keeping `since`), or `$4` says take over. An archived chart takes none.
const TAKE_LOCK: &str = concat!(
    "WITH c AS (
  UPDATE charts
     SET lock_actor = $3,
         lock_since = CASE WHEN lock_actor = $3 AND lock_expires_at > now()
                           THEN lock_since ELSE now() END,
         lock_expires_at = now() + $5 * interval '1 second'
   WHERE workspace_id = $1::uuid AND key = $2 AND archived_at IS NULL
     AND (lock_actor IS NULL OR lock_actor = $3 OR lock_expires_at <= now() OR $4)
  RETURNING *
)
SELECT ",
    lock_columns!(),
    " FROM c ",
    lock_joins!()
);

/// Take, renew or take over the editing lock (07 §9). It is coordination
/// between people who may all edit, not a boundary: whoever may save the
/// draft may take it over.
async fn lock(
    Staff(auth): Staff,
    Path(key): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<LockChart>,
) -> Result<axum::Json<Lock>, ApiError> {
    let take_over = body.take_over;
    let actor = auth.actor();
    let take = async {
        // Twice at most: a lock that expires between the two statements is
        // free on the second try.
        for _ in 0..2 {
            let row: Option<LockRow> = sqlx::query_as(TAKE_LOCK)
                .bind(auth.workspace().uuid())
                .bind(&key)
                .bind(&*actor)
                .bind(take_over)
                .bind(LOCK_SECS)
                .fetch_optional(&state.db)
                .await?;
            if let Some((a, e, since, until)) = row {
                return Lock::live(&auth, a, e, since, until)
                    .ok_or_else(|| ApiError::internal("a lock just taken is not live"));
            }
            let row: Option<LockState> = sqlx::query_as(READ_LOCK)
                .bind(auth.workspace().uuid())
                .bind(&key)
                .fetch_optional(&state.db)
                .await?;
            let (is_archived, a, e, since, until) = row.ok_or_else(not_found)?;
            if is_archived {
                return Err(archived());
            }
            if let Some(lock) = Lock::live(&auth, a, e, since, until) {
                return Err(locked(lock));
            }
        }
        Err(ApiError::new(Code::Unavailable).with_message("The lock kept changing; retry."))
    };
    let lock = take
        .instrument(cavea_telemetry::db_span("take_lock"))
        .await?;
    tracing::debug!(chart = %key, since = %lock.since, "chart lock held");
    Ok(axum::Json(lock))
}

const RELEASE_LOCK: &str = "UPDATE charts
   SET lock_actor = NULL, lock_since = NULL, lock_expires_at = NULL
 WHERE workspace_id = $1::uuid AND key = $2 AND lock_actor = $3";

/// Release the caller's lock; another's stays. `204` either way.
async fn unlock(
    Staff(auth): Staff,
    Path(key): Path<String>,
    State(state): State<AppState>,
) -> Result<StatusCode, ApiError> {
    let release = async {
        let done = sqlx::query(RELEASE_LOCK)
            .bind(auth.workspace().uuid())
            .bind(&key)
            .bind(&*auth.actor())
            .execute(&state.db)
            .await?;
        if done.rows_affected() == 0 {
            let exists: bool = sqlx::query_scalar(CHART_EXISTS)
                .bind(auth.workspace().uuid())
                .bind(&key)
                .fetch_one(&state.db)
                .await?;
            if !exists {
                return Err(not_found());
            }
        }
        Ok(StatusCode::NO_CONTENT)
    };
    release
        .instrument(cavea_telemetry::db_span("release_lock"))
        .await
}

fn version_uuid(id: &str) -> Option<&str> {
    let hex = id.strip_prefix("cv_")?;
    (hex.len() == 32 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))).then_some(hex)
}
