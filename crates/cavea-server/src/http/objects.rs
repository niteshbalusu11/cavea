//! `GET /v1/events/{event}/objects` (08 §6.2): the status of given labels, or
//! a page of objects in `idx` order, optionally of one status. Secret key.
//!
//! Read from the committed rows of `object_state` and `holds`, like the
//! event's counts: exactly what was acknowledged, without loading the event's
//! actor (the engine has no read command). Free objects have no row, so a page
//! walks the chart's `idx` order and asks PostgreSQL only for the rows of the
//! objects it looks at: one query of at most [`WINDOW`] objects per page.

use std::collections::HashMap;

use axum::Json as JsonBody;
use axum::extract::State;
use axum::response::{IntoResponse as _, Response};
use cavea_engine::{Idx, ObjectRef, Timestamp};
use serde::Serialize;
use serde_json::value::RawValue;
use tracing::Instrument as _;

use super::extract::{Page, Path, Query, check_object_count};
use super::openapi::{Needs, Op, Routes};
use super::seats::{Find, chart, rfc3339, target};
use super::staff::Staff;
use super::{ApiError, AppState, Code};
use crate::console::Permission;
use crate::seats::Chart;

pub(super) fn routes() -> Routes {
    let string = || serde_json::json!({ "type": "string" });
    Routes::new().add(
        Op::get(
            "/v1/events/{event}/objects",
            "listObjects",
            "Objects' status",
        )
        .about(
            "By label (`label`, repeated, at most 1,000), or a page in `idx` order \
(`status`, `limit`, `after`); not both. Read from committed state.",
        )
        .needs(Needs::Staff(Permission::EventsRead))
        .query(
            "label",
            serde_json::json!({ "type": "array", "items": string(), "maxItems": 1_000 }),
            "Objects' labels: `label=A&label=B`.",
        )
        .query(
            "status",
            string(),
            "`free`, `held`, `booked`, `blocked` or a custom code, 4 to 15.",
        )
        .query(
            "limit",
            serde_json::json!({ "type": "integer", "minimum": 1, "maximum": 100 }),
            "Objects per page.",
        )
        .query("after", string(), "The `next` of the previous page.")
        .reply(
            200,
            "Objects",
            "`{data, unknown}` by label, or `{data, next}` for a page",
        ),
        objects,
    )
}

/// The most objects one page looks at. A page of a rare status may hold fewer
/// than `limit` objects, or none, with a `next` to continue from.
const WINDOW: usize = 10_000;

/// A status filter: a domain status code (02 §4.1).
fn status_code(wire: &str) -> Option<i16> {
    match wire {
        "free" => Some(0),
        "held" => Some(1),
        "booked" => Some(2),
        "blocked" => Some(3),
        code => code.parse().ok().filter(|c| (4..=15).contains(c)),
    }
}

/// An object as a manager sees it. `seq` and the per-status fields are
/// present only for an object that is not free; the hold token, and its hash,
/// are never shown.
#[derive(Debug, Serialize)]
struct Object<'a> {
    label: &'a str,
    oid: u32,
    /// `"free"`, `"held"`, `"booked"`, `"blocked"`, or a custom code.
    status: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    seq: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    order_ref: Option<String>,
    /// Booked objects only; `null` when none was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    ticket_type: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extra: Option<Option<Box<RawValue>>>,
}

/// Typed, not `json!`: `extra` is returned as stored, byte for byte.
#[derive(Serialize)]
struct ByLabel<'a> {
    data: Vec<Object<'a>>,
    /// Labels the event's chart does not have, each once.
    unknown: Vec<String>,
}

#[derive(Serialize)]
struct Listed<'a> {
    data: Vec<Object<'a>>,
    next: Option<&'a str>,
}

/// A committed `object_state` row, with its hold's expiry.
struct Row {
    status: i16,
    order_ref: Option<String>,
    ticket_type: Option<String>,
    extra: Option<String>,
    seq: i64,
    expires_ms: Option<i64>,
}

type RowTuple = (
    i32,
    i16,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    Option<i64>,
);

/// The rows of the given objects (`oid`s, stored bit for bit as `int`).
const ROWS: &str = "SELECT o.oid, o.status, o.order_ref, o.ticket_type, o.extra::text, o.seq,
       (extract(epoch FROM h.expires_at) * 1000)::int8
  FROM object_state o
  JOIN events e ON e.id = o.event_id AND e.workspace_id = $3::uuid
  LEFT JOIN holds h ON h.event_id = o.event_id AND h.id = o.hold_id
 WHERE o.event_id = $1::uuid AND o.oid = ANY($2::int4[])";

fn query_error(message: &'static str) -> ApiError {
    ApiError::new(Code::InvalidRequest)
        .with_message(message)
        .with_detail("reason", "query")
}

/// `label` (repeated), or `status`, `limit` and `after`.
#[derive(Default)]
struct Params {
    labels: Vec<String>,
    status: Option<String>,
    limit: Option<String>,
    after: Option<String>,
}

impl Params {
    fn parse(pairs: Vec<(String, String)>) -> Result<Self, ApiError> {
        let mut params = Self::default();
        for (name, value) in pairs {
            let slot = match name.as_str() {
                "label" => {
                    params.labels.push(value);
                    continue;
                }
                "status" => &mut params.status,
                "limit" => &mut params.limit,
                "after" => &mut params.after,
                _ => return Err(query_error("Unknown query parameter.")),
            };
            if slot.replace(value).is_some() {
                return Err(query_error("A query parameter is repeated."));
            }
        }
        let paged = params.status.is_some() || params.limit.is_some() || params.after.is_some();
        if paged && !params.labels.is_empty() {
            return Err(query_error(
                "`label` cannot be combined with `status`, `limit` or `after`.",
            ));
        }
        Ok(params)
    }

    fn limit(&self) -> Result<usize, ApiError> {
        let Some(limit) = &self.limit else {
            return Ok(Page::DEFAULT_LIMIT as usize);
        };
        limit
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=Page::MAX_LIMIT).contains(n))
            .map(|n| n as usize)
            .ok_or_else(|| query_error("`limit` must be 1 to 100."))
    }
}

async fn objects(
    Staff(auth): Staff,
    Path(event): Path<String>,
    Query(pairs): Query<Vec<(String, String)>>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let params = Params::parse(pairs)?;
    let filter = match params.status.as_deref() {
        None => None,
        Some(s) => Some(status_code(s).ok_or_else(|| {
            query_error("`status` must be free, held, booked, blocked or a custom code, 4 to 15.")
        })?),
    };
    let limit = params.limit()?;
    check_object_count(params.labels.len())?;
    let target = target(&state, &auth, Find::Key(&event)).await?;
    let chart = chart(&state, target.version).await?;
    let read = Read {
        state: &state,
        chart: &chart,
        event: format!("{:032x}", target.id.0),
        workspace: auth.workspace().uuid(),
    };
    if !params.labels.is_empty() {
        return read.labels(&params.labels).await;
    }
    let start = match params.after.as_deref() {
        None => 0,
        Some(after) => {
            let idx = chart
                .view
                .resolve(&ObjectRef::Label(after.into()))
                .ok_or_else(|| query_error("`after` is not a label of the event's chart."))?;
            idx as usize + 1
        }
    };
    read.page(start, filter, limit).await
}

struct Read<'a> {
    state: &'a AppState,
    chart: &'a Chart,
    event: String,
    workspace: &'a str,
}

impl Read<'_> {
    /// The committed rows of the objects at `idxs`, by `oid`.
    async fn rows(&self, idxs: &[Idx]) -> Result<HashMap<i32, Row>, ApiError> {
        let oids = idxs
            .iter()
            .map(|&idx| self.oid(idx).map(u32::cast_signed))
            .collect::<Result<Vec<_>, _>>()?;
        let rows: Vec<RowTuple> = sqlx::query_as(ROWS)
            .bind(&self.event)
            .bind(oids)
            .bind(self.workspace)
            .fetch_all(&self.state.db)
            .instrument(cavea_telemetry::db_span("read_objects"))
            .await?;
        Ok(rows
            .into_iter()
            .map(
                |(oid, status, order_ref, ticket_type, extra, seq, expires_ms)| {
                    let row = Row {
                        status,
                        order_ref,
                        ticket_type,
                        extra,
                        seq,
                        expires_ms,
                    };
                    (oid, row)
                },
            )
            .collect())
    }

    fn oid(&self, idx: Idx) -> Result<u32, ApiError> {
        self.chart
            .view
            .oid(idx)
            .ok_or_else(|| ApiError::internal("an object of the chart has no oid"))
    }

    fn object(&self, idx: Idx, row: Option<&Row>) -> Result<Object<'_>, ApiError> {
        let label = self
            .chart
            .label(idx)
            .ok_or_else(|| ApiError::internal("an object of the chart has no label"))?;
        let mut object = Object {
            label,
            oid: self.oid(idx)?,
            status: "free".into(),
            seq: None,
            expires_at: None,
            order_ref: None,
            ticket_type: None,
            extra: None,
        };
        // A free object may have a row, for its channel: it is still free.
        let Some(row) = row.filter(|r| r.status != 0) else {
            return Ok(object);
        };
        object.seq = Some(row.seq);
        object.status = match row.status {
            1 => "held".into(),
            2 => "booked".into(),
            3 => "blocked".into(),
            code => code.into(),
        };
        if row.status == 1 {
            let ms = row.expires_ms.and_then(|ms| u64::try_from(ms).ok());
            object.expires_at = ms.map(|ms| rfc3339(Timestamp(ms)));
        }
        if row.status == 2 {
            object.order_ref.clone_from(&row.order_ref);
            object.ticket_type = Some(row.ticket_type.clone());
            let extra = row.extra.clone().map(RawValue::from_string).transpose();
            object.extra = Some(extra.map_err(ApiError::internal)?);
        }
        Ok(object)
    }

    /// Each label's object in `idx` order, and the labels the chart lacks.
    async fn labels(&self, labels: &[String]) -> Result<Response, ApiError> {
        let mut idxs = Vec::new();
        let mut unknown = Vec::new();
        for label in labels {
            match self
                .chart
                .view
                .resolve(&ObjectRef::Label(label.as_str().into()))
            {
                Some(idx) => idxs.push(idx),
                None if !unknown.contains(label) => unknown.push(label.clone()),
                None => {}
            }
        }
        idxs.sort_unstable();
        idxs.dedup();
        let rows = self.rows(&idxs).await?;
        let data = idxs
            .iter()
            .map(|&idx| self.object(idx, rows.get(&self.oid(idx)?.cast_signed())))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(JsonBody(ByLabel { data, unknown }).into_response())
    }

    /// Up to `limit` objects matching `filter` from `idx` `start`, looking at
    /// no more than [`WINDOW`] objects. `next` is the label of the last
    /// object looked at, while the chart goes on.
    async fn page(
        &self,
        start: usize,
        filter: Option<i16>,
        limit: usize,
    ) -> Result<Response, ApiError> {
        let len = self.chart.view.len();
        // Without a filter every object matches: look at one page's worth.
        let window = if filter.is_none() { limit } else { WINDOW };
        let end = len.min(start.saturating_add(window));
        let idxs = (start..end)
            .map(Idx::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(ApiError::internal)?;
        let rows = self.rows(&idxs).await?;
        let mut data = Vec::new();
        let mut last = None;
        for &idx in &idxs {
            last = Some(idx);
            let row = rows.get(&self.oid(idx)?.cast_signed());
            let status = row.map_or(0, |r| r.status);
            if filter.is_none_or(|f| f == status) {
                data.push(self.object(idx, row)?);
                if data.len() == limit {
                    break;
                }
            }
        }
        let next = last
            .filter(|&idx| (idx as usize) + 1 < len)
            .and_then(|idx| self.chart.label(idx));
        Ok(JsonBody(Listed { data, next }).into_response())
    }
}
