//! The reconciler (13 §3.3): `cavea-server verify`. For one event, or every
//! event, it replays `event_log` from the beginning through the engine's
//! [`Replay`], which refuses any record that is not a legal transition, and
//! compares the result with `object_state`, `holds` and `events.seq`.
//!
//! It is read-only and safe on a live database: each event is read in one
//! `REPEATABLE READ READ ONLY` transaction, so the log and the state rows come
//! from one snapshot while commits continue; it takes no lease and no lock
//! beyond what any read takes. The log is read in pages, so memory follows the
//! event's state, not the length of its log.
//!
//! Reports name objects, holds and fields; they never carry hold tokens or
//! their hashes, and carry order references and `extra` only when asked
//! (12 §4).

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::Arc;

use cavea_engine::{
    Booking, ChartVersionId, Difference, EventId, HoldField, HoldId, LogRecord, ObjectField,
    ObjectRow, Op, Replay, Status, Timestamp,
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sqlx::postgres::PgPool;
use tracing::Instrument as _;

use crate::config::DatabaseUrl;
use crate::seats::{CHART_CACHE_OBJECTS, Chart, PgCharts};

/// Exit codes: the CLI's (0 ok, 2 usage, 3 other), with 1 for differences.
pub const EXIT_CLEAN: u8 = 0;
pub const EXIT_DIFFERENCES: u8 = 1;
pub const EXIT_USAGE: u8 = 2;
pub const EXIT_COULD_NOT_RUN: u8 = 3;

/// Log rows read per query.
const LOG_PAGE: i64 = 10_000;
/// Findings kept per event; the rest are counted.
pub const MAX_FINDINGS: usize = 1_000;

/// What a report states it did not check, and why.
pub const NOT_CHECKED: &[&str] = &[
    "holds.token_hash: the log does not carry it (08 §4.4)",
    "holds.session_sub, created_by and created_at: written by the server, not by seat commands",
    "idempotency: replies are not in the log",
    "area_alloc: no command allocates areas yet, so every row is a difference",
    "object_state.channel: no command assigns channels yet, so every channel is a difference",
    "hold expiry: the log records database time, not the engine clock that expires holds",
];

const USAGE: &str =
    "usage: cavea-server verify (--workspace KEY --event KEY | --event-id ID | --all) \
[--json] [--include-booking-data]
  Replays each event's log and compares it with its stored state, read-only.
  Reads CAVEA_DATABASE_URL. Exit: 0 clean, 1 differences, 2 usage, 3 could not run.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// The database could not be reached or a query failed.
    Unavailable(String),
    UnknownEvent,
    /// The event's chart version could not be read.
    Chart(String),
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(why) => write!(f, "database unavailable: {why}"),
            Self::UnknownEvent => f.write_str("unknown event"),
            Self::Chart(why) => write!(f, "chart version unreadable: {why}"),
        }
    }
}

impl std::error::Error for VerifyError {}

fn db(e: sqlx::Error) -> VerifyError {
    VerifyError::Unavailable(e.to_string())
}

/// Which events to verify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Key { workspace: String, event: String },
    Id(EventId),
    All,
}

/// One difference or integrity failure. Fields that do not apply are left out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Finding {
    /// `seq` (INV-5), `transition` (a record the replay refuses),
    /// `unreadable_record`, `unknown_object`, `events_seq`, `object`, `hold`,
    /// `unreadable_row`, `channel` or `area_alloc`.
    pub check: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invariant: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_id: Option<u64>,
    /// For `object` and `hold`: `missing` (the log has it, no row does),
    /// `unexpected` (a row the log does not explain), `row_twice`, or the
    /// column that differs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stored: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replayed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The result for one event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventReport {
    /// `events.id` as 32 hex digits.
    pub event_id: String,
    pub workspace: String,
    pub event: String,
    pub chart_version: String,
    pub clean: bool,
    pub log_records: u64,
    /// The last `seq` in the log, 0 for an empty log.
    pub log_last_seq: u64,
    /// The last `seq` the replay applied: the log's last unless a record was
    /// refused.
    pub replayed_to: u64,
    pub events_seq: u64,
    pub object_rows: u64,
    pub hold_rows: u64,
    pub area_alloc_rows: u64,
    /// False when the replay stopped early: state is then not compared.
    pub state_compared: bool,
    pub findings_total: u64,
    /// At most [`MAX_FINDINGS`], in the order found.
    pub findings: Vec<Finding>,
    pub not_checked: &'static [&'static str],
}

impl EventReport {
    fn push(&mut self, finding: Finding) {
        self.findings_total += 1;
        if self.findings.len() < MAX_FINDINGS {
            self.findings.push(finding);
        }
    }
}

/// The whole run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub version: u32,
    pub clean: bool,
    pub events: Vec<EventReport>,
}

/// The events `target` names, in id order.
pub async fn events(pool: &PgPool, target: &Target) -> Result<Vec<EventId>, VerifyError> {
    let ids: Vec<String> = match target {
        Target::All => {
            sqlx::query_scalar("SELECT replace(id::text, '-', '') FROM events ORDER BY id")
                .fetch_all(pool)
                .instrument(cavea_telemetry::db_span("verify_events"))
                .await
                .map_err(db)?
        }
        Target::Key { workspace, event } => sqlx::query_scalar(
            "SELECT replace(e.id::text, '-', '') FROM events e
               JOIN workspaces w ON w.id = e.workspace_id
              WHERE w.key = $1 AND e.key = $2",
        )
        .bind(workspace)
        .bind(event)
        .fetch_all(pool)
        .instrument(cavea_telemetry::db_span("verify_events"))
        .await
        .map_err(db)?,
        Target::Id(id) => vec![format!("{:032x}", id.0)],
    };
    if ids.is_empty() && *target != Target::All {
        return Err(VerifyError::UnknownEvent);
    }
    ids.iter()
        .map(|h| u128::from_str_radix(h, 16).map(EventId))
        .collect::<Result<_, _>>()
        .map_err(|_| VerifyError::Unavailable("unreadable event id".into()))
}

/// One `event_log` row as stored.
type LogRow = (
    i64,
    String,
    i16,
    String,
    Option<i64>,
    Option<String>,
    Vec<i32>,
    Option<String>,
);

/// One booked object in `detail`: `(oid, ticket_type, extra)`.
type DetailObject<'a> = (u32, Option<String>, Option<&'a RawValue>);

/// `event_log.detail` as `PgStore` writes it.
#[derive(Deserialize)]
struct Detail<'a> {
    status: Option<u8>,
    expires_at: Option<u64>,
    max_objects: Option<u32>,
    #[serde(borrow)]
    objects: Option<Vec<DetailObject<'a>>>,
}

fn op(code: i16) -> Option<Op> {
    Some(match code {
        1 => Op::CreateHold,
        2 => Op::Hold,
        3 => Op::Release,
        4 => Op::ExtendHold,
        5 => Op::Book,
        6 => Op::ChangeStatus,
        7 => Op::Expire,
        _ => return None,
    })
}

/// A stored row as the engine wrote it, or why it cannot be.
fn record(seq: u64, row: LogRow) -> Result<LogRecord, String> {
    let (_, _, code, actor, hold_id, order_ref, oids, detail) = row;
    let op = op(code).ok_or_else(|| format!("unknown op {code}"))?;
    let hold_id = hold_id
        .map(|h| u64::try_from(h).map(HoldId))
        .transpose()
        .map_err(|_| "negative hold_id".to_owned())?;
    let detail: Detail<'_> = match detail.as_deref() {
        Some(text) => serde_json::from_str(text).map_err(|e| format!("detail: {e}"))?,
        None => Detail {
            status: None,
            expires_at: None,
            max_objects: None,
            objects: None,
        },
    };
    let status = detail
        .status
        .map(|c| Status::from_code(c).ok_or_else(|| format!("status {c}")))
        .transpose()?;
    let details = detail
        .objects
        .unwrap_or_default()
        .into_iter()
        .map(|(oid, ticket_type, extra)| {
            // `extra` is kept byte for byte, as the engine stored it.
            let extra = extra.map(RawValue::get).filter(|e| *e != "null");
            (oid, ticket_type.map(Arc::from), extra.map(Arc::from))
        })
        .collect();
    Ok(LogRecord {
        seq,
        op,
        actor: actor.into(),
        hold_id,
        // The stored bits of a u32 (08 §5).
        oids: oids.into_iter().map(|o| o as u32).collect(),
        expires_at: detail.expires_at.map(Timestamp),
        max_objects: detail.max_objects,
        order_ref: order_ref.map(Arc::from),
        status,
        details,
    })
}

const LOG: &str = "
SELECT seq, at::text, op, actor, hold_id, order_ref, oids, detail::text
  FROM event_log
 WHERE event_id = $1::uuid AND (seq, at) > ($2, $3::timestamptz)
 ORDER BY seq, at
 LIMIT $4";

type ObjectTuple = (
    i32,
    i16,
    Option<i64>,
    Option<i16>,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
);

/// Verify one event; `Err` only when it could not be verified at all.
pub async fn verify_event(
    pool: &PgPool,
    charts: &PgCharts,
    event: EventId,
    booking_data: bool,
) -> Result<EventReport, VerifyError> {
    let id = format!("{:032x}", event.0);
    let mut tx = pool
        .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .instrument(cavea_telemetry::db_span("verify_begin"))
        .await
        .map_err(db)?;
    let head: Option<(i64, String, String, String)> = sqlx::query_as(
        "SELECT e.seq, replace(e.chart_version_id::text, '-', ''), w.key, e.key
           FROM events e JOIN workspaces w ON w.id = e.workspace_id
          WHERE e.id = $1::uuid",
    )
    .bind(&id)
    .fetch_optional(&mut *tx)
    .instrument(cavea_telemetry::db_span("verify_event"))
    .await
    .map_err(db)?;
    let (events_seq, version, workspace, key) = head.ok_or(VerifyError::UnknownEvent)?;
    let version = u128::from_str_radix(&version, 16)
        .map(ChartVersionId)
        .map_err(|_| VerifyError::Chart("unreadable chart_version_id".into()))?;
    // Chart versions never change, so reading one outside the snapshot is safe.
    let chart = charts.get(version).await.map_err(|e| match e {
        cavea_engine::StoreError::Unavailable(why) => VerifyError::Unavailable(why),
        other => VerifyError::Chart(other.to_string()),
    })?;
    let mut report = EventReport {
        event_id: id.clone(),
        workspace,
        event: key,
        chart_version: format!("cv_{:032x}", version.0),
        clean: false,
        log_records: 0,
        log_last_seq: 0,
        replayed_to: 0,
        events_seq: u64::try_from(events_seq).unwrap_or(0),
        object_rows: 0,
        hold_rows: 0,
        area_alloc_rows: 0,
        state_compared: false,
        findings_total: 0,
        findings: Vec::new(),
        not_checked: NOT_CHECKED,
    };
    let label = |oid: u32| {
        let idx = chart.view.idx_of_oid(oid)?;
        chart.label(idx).map(str::to_owned)
    };

    // The log, in `seq` order, a page at a time.
    let mut replay = Replay::default();
    let mut replaying = true;
    let mut last: Option<u64> = None;
    let mut after = (-1_i64, "-infinity".to_owned());
    loop {
        let rows: Vec<LogRow> = sqlx::query_as(LOG)
            .bind(&id)
            .bind(after.0)
            .bind(&after.1)
            .bind(LOG_PAGE)
            .fetch_all(&mut *tx)
            .instrument(cavea_telemetry::db_span("verify_read_log"))
            .await
            .map_err(db)?;
        let full = rows.len() as i64 == LOG_PAGE;
        if let Some(row) = rows.last() {
            after = (row.0, row.1.clone());
        }
        for row in rows {
            report.log_records += 1;
            let Ok(seq) = u64::try_from(row.0) else {
                report.push(Finding {
                    check: "unreadable_record",
                    detail: Some(format!("negative seq {}", row.0)),
                    ..Finding::default()
                });
                replaying = false;
                continue;
            };
            // INV-5: from 1, no gaps, no repeats.
            let expected = last.map_or(1, |s| s + 1);
            if seq != expected {
                let detail = if Some(seq) == last {
                    "repeated".to_owned()
                } else if seq == expected + 1 {
                    format!("seq {expected} missing")
                } else {
                    format!("seq {expected} to {} missing", seq - 1)
                };
                report.push(Finding {
                    check: "seq",
                    invariant: Some("INV-5"),
                    seq: Some(seq),
                    detail: Some(detail),
                    ..Finding::default()
                });
            }
            last = Some(seq);
            let r = match record(seq, row) {
                Ok(r) => r,
                Err(why) => {
                    report.push(Finding {
                        check: "unreadable_record",
                        seq: Some(seq),
                        detail: Some(why),
                        ..Finding::default()
                    });
                    replaying = false;
                    continue;
                }
            };
            let named: BTreeSet<u32> = r
                .oids
                .iter()
                .chain(r.details.iter().map(|d| &d.0))
                .copied()
                .collect();
            for oid in named {
                if chart.view.idx_of_oid(oid).is_none() {
                    report.push(Finding {
                        check: "unknown_object",
                        invariant: Some("INV-6"),
                        seq: Some(seq),
                        oid: Some(oid),
                        ..Finding::default()
                    });
                }
            }
            if replaying && let Err(e) = replay.apply(&r) {
                replaying = false;
                // A `seq` out of turn is reported above already.
                if e.invariant != "INV-5" {
                    report.push(Finding {
                        check: "transition",
                        invariant: Some(e.invariant),
                        seq: Some(e.seq),
                        detail: Some(e.detail),
                        ..Finding::default()
                    });
                }
            }
        }
        if !full {
            break;
        }
    }
    report.log_last_seq = last.unwrap_or(0);
    report.replayed_to = replay.seq;
    if report.events_seq != report.log_last_seq {
        report.push(Finding {
            check: "events_seq",
            invariant: Some("INV-5"),
            stored: Some(report.events_seq.to_string()),
            replayed: Some(report.log_last_seq.to_string()),
            ..Finding::default()
        });
    }

    // The state rows, from the same snapshot.
    let rows: Vec<ObjectTuple> = sqlx::query_as(
        "SELECT oid, status, hold_id, channel, order_ref, ticket_type, extra::text, seq
           FROM object_state WHERE event_id = $1::uuid",
    )
    .bind(&id)
    .fetch_all(&mut *tx)
    .instrument(cavea_telemetry::db_span("verify_read_objects"))
    .await
    .map_err(db)?;
    report.object_rows = rows.len() as u64;
    let mut objects = Vec::with_capacity(rows.len());
    for (oid, status, hold, channel, order_ref, ticket_type, extra, seq) in rows {
        let oid = oid as u32; // the stored bits of a u32
        if chart.view.idx_of_oid(oid).is_none() {
            report.push(Finding {
                check: "unknown_object",
                invariant: Some("INV-6"),
                oid: Some(oid),
                detail: Some("object_state row".into()),
                ..Finding::default()
            });
        }
        if let Some(channel) = channel {
            report.push(Finding {
                check: "channel",
                oid: Some(oid),
                label: label(oid),
                stored: Some(channel.to_string()),
                ..Finding::default()
            });
        }
        let status = u8::try_from(status).ok().and_then(Status::from_code);
        let (Some(status), Ok(seq), Ok(hold_id)) = (
            status,
            u64::try_from(seq),
            hold.map(|h| u64::try_from(h).map(HoldId)).transpose(),
        ) else {
            report.push(Finding {
                check: "unreadable_row",
                oid: Some(oid),
                label: label(oid),
                ..Finding::default()
            });
            continue;
        };
        objects.push(ObjectRow {
            oid,
            status,
            hold_id,
            booking: order_ref.map(|order_ref| Booking {
                order_ref: order_ref.into(),
                ticket_type: ticket_type.map(Arc::from),
                extra: extra.map(Arc::from),
            }),
            seq,
        });
    }
    let rows: Vec<(i64, i64, Option<i32>)> = sqlx::query_as(
        "SELECT id, (extract(epoch FROM expires_at) * 1000)::int8, max_objects
           FROM holds WHERE event_id = $1::uuid",
    )
    .bind(&id)
    .fetch_all(&mut *tx)
    .instrument(cavea_telemetry::db_span("verify_read_holds"))
    .await
    .map_err(db)?;
    report.hold_rows = rows.len() as u64;
    let mut holds = Vec::with_capacity(rows.len());
    for (hold, expires, max) in rows {
        match (
            u64::try_from(hold),
            u64::try_from(expires),
            max.map(u32::try_from).transpose(),
        ) {
            (Ok(h), Ok(at), Ok(max)) => holds.push((HoldId(h), Timestamp(at), max)),
            _ => report.push(Finding {
                check: "unreadable_row",
                hold_id: u64::try_from(hold).ok(),
                ..Finding::default()
            }),
        }
    }
    let areas: i64 =
        sqlx::query_scalar("SELECT count(*) FROM area_alloc WHERE event_id = $1::uuid")
            .bind(&id)
            .fetch_one(&mut *tx)
            .instrument(cavea_telemetry::db_span("verify_read_areas"))
            .await
            .map_err(db)?;
    report.area_alloc_rows = u64::try_from(areas).unwrap_or(0);
    if areas > 0 {
        report.push(Finding {
            check: "area_alloc",
            stored: Some(areas.to_string()),
            detail: Some("no command allocates areas yet".into()),
            ..Finding::default()
        });
    }
    tx.commit()
        .instrument(cavea_telemetry::db_span("verify_end"))
        .await
        .map_err(db)?;

    // INV-6, only when the whole log replayed.
    if replaying {
        report.state_compared = true;
        // `events.seq` is checked against the log's last `seq` above.
        for d in replay.compare(replay.seq, holds.iter().copied(), &objects) {
            report.push(finding(d, &replay, &objects, &holds, &chart, booking_data));
        }
    }
    report.clean = report.findings_total == 0;
    Ok(report)
}

/// A [`Difference`] with its values. Order references and `extra` are
/// withheld unless `booking_data`.
fn finding(
    d: Difference,
    replay: &Replay,
    objects: &[ObjectRow],
    holds: &[(HoldId, Timestamp, Option<u32>)],
    chart: &Chart,
    booking_data: bool,
) -> Finding {
    let label = |oid: u32| {
        let idx = chart.view.idx_of_oid(oid)?;
        chart.label(idx).map(str::to_owned)
    };
    let object = |oid, field| Finding {
        check: "object",
        oid: Some(oid),
        label: label(oid),
        field: Some(field),
        ..Finding::default()
    };
    let hold = |id: HoldId, field| Finding {
        check: "hold",
        hold_id: Some(id.0),
        field: Some(field),
        ..Finding::default()
    };
    let opt = |v: Option<String>| v.unwrap_or_else(|| "null".into());
    match d {
        Difference::Seq { stored, log } => Finding {
            check: "events_seq",
            invariant: Some("INV-5"),
            stored: Some(stored.to_string()),
            replayed: Some(log.to_string()),
            ..Finding::default()
        },
        Difference::ObjectUnexpected(oid) => {
            let stored = objects.iter().find(|o| o.oid == oid);
            Finding {
                stored: stored.map(|o| o.status.code().to_string()),
                ..object(oid, "unexpected")
            }
        }
        Difference::ObjectMissing(oid) => Finding {
            replayed: replay.objects.get(&oid).map(|o| o.0.code().to_string()),
            ..object(oid, "missing")
        },
        Difference::ObjectRowTwice(oid) => object(oid, "row_twice"),
        Difference::Object(oid, field) => {
            let row = objects.iter().find(|o| o.oid == oid);
            let log = replay.objects.get(&oid);
            let (name, stored, replayed, secret) = match field {
                ObjectField::Status => (
                    "status",
                    row.map(|r| r.status.code().to_string()),
                    log.map(|l| l.0.code().to_string()),
                    false,
                ),
                ObjectField::HoldId => (
                    "hold_id",
                    Some(opt(row.and_then(|r| r.hold_id).map(|h| h.0.to_string()))),
                    Some(opt(log.and_then(|l| l.1).map(|h| h.0.to_string()))),
                    false,
                ),
                ObjectField::OrderRef => (
                    "order_ref",
                    Some(opt(row
                        .and_then(|r| r.booking.as_ref())
                        .map(|b| b.order_ref.to_string()))),
                    Some(opt(log
                        .and_then(|l| l.2.as_ref())
                        .map(|b| b.order_ref.to_string()))),
                    true,
                ),
                ObjectField::TicketType => (
                    "ticket_type",
                    Some(opt(row
                        .and_then(|r| r.booking.as_ref())
                        .and_then(|b| b.ticket_type.as_deref().map(str::to_owned)))),
                    Some(opt(log
                        .and_then(|l| l.2.as_ref())
                        .and_then(|b| b.ticket_type.as_deref().map(str::to_owned)))),
                    false,
                ),
                ObjectField::Extra => (
                    "extra",
                    Some(opt(row
                        .and_then(|r| r.booking.as_ref())
                        .and_then(|b| b.extra.as_deref().map(str::to_owned)))),
                    Some(opt(log
                        .and_then(|l| l.2.as_ref())
                        .and_then(|b| b.extra.as_deref().map(str::to_owned)))),
                    true,
                ),
                ObjectField::Seq => (
                    "seq",
                    row.map(|r| r.seq.to_string()),
                    log.map(|l| l.3.to_string()),
                    false,
                ),
            };
            let show = !secret || booking_data;
            Finding {
                stored: stored.filter(|_| show),
                replayed: replayed.filter(|_| show),
                detail: (!show)
                    .then(|| "values withheld; --include-booking-data shows them".into()),
                ..object(oid, name)
            }
        }
        Difference::HoldUnexpected(id) => hold(id, "unexpected"),
        Difference::HoldMissing(id) => hold(id, "missing"),
        Difference::HoldRowTwice(id) => hold(id, "row_twice"),
        Difference::Hold(id, field) => {
            let log = replay.holds.get(&id);
            let row = holds.iter().find(|h| h.0 == id);
            match field {
                HoldField::ExpiresAt => Finding {
                    stored: row.map(|h| h.1.0.to_string()),
                    replayed: log.map(|h| h.expires_at.0.to_string()),
                    ..hold(id, "expires_at")
                },
                HoldField::MaxObjects => Finding {
                    stored: row.map(|h| opt(h.2.map(|m| m.to_string()))),
                    replayed: log.map(|h| opt(h.max_objects.map(|m| m.to_string()))),
                    ..hold(id, "max_objects")
                },
            }
        }
    }
}

/// The human-readable summary of a report.
#[must_use]
pub fn summary(report: &Report) -> String {
    let mut s = String::new();
    for e in &report.events {
        let verdict = if e.clean {
            "clean".to_owned()
        } else {
            format!("{} difference(s)", e.findings_total)
        };
        // Writing to a String cannot fail.
        let _ = writeln!(
            s,
            "event {}/{} ({}): {verdict}\n  log: {} records, last seq {}, replayed to {}; events.seq {}; \
             object_state {} rows; holds {} rows; area_alloc {} rows{}",
            e.workspace,
            e.event,
            e.event_id,
            e.log_records,
            e.log_last_seq,
            e.replayed_to,
            e.events_seq,
            e.object_rows,
            e.hold_rows,
            e.area_alloc_rows,
            if e.state_compared {
                ""
            } else {
                "; state not compared: the log did not replay to its end"
            },
        );
        for f in &e.findings {
            // `object` and `hold` findings start with the object or hold.
            let mut line = match f.check {
                "object" | "hold" => "  -".to_owned(),
                check => format!("  - {check}"),
            };
            if let Some(i) = f.invariant {
                let _ = write!(line, " [{i}]");
            }
            if let Some(seq) = f.seq {
                let _ = write!(line, " seq {seq}");
            }
            if let Some(oid) = f.oid {
                let _ = write!(line, " object {oid}");
                if let Some(l) = &f.label {
                    let _ = write!(line, " ({l})");
                }
            }
            if let Some(h) = f.hold_id {
                let _ = write!(line, " hold {h}");
            }
            if let Some(field) = f.field {
                let _ = write!(line, " {field}");
            }
            if let Some(v) = &f.stored {
                let _ = write!(line, " stored {v}");
            }
            if let Some(v) = &f.replayed {
                let _ = write!(line, " log {v}");
            }
            if let Some(d) = &f.detail {
                let _ = write!(line, ": {d}");
            }
            let _ = writeln!(s, "{line}");
        }
        if e.findings_total > e.findings.len() as u64 {
            let _ = writeln!(
                s,
                "  … and {} more",
                e.findings_total - e.findings.len() as u64
            );
        }
    }
    let _ = writeln!(
        s,
        "{} event(s) verified: {}",
        report.events.len(),
        if report.clean {
            "clean"
        } else {
            "DIFFERENCES FOUND"
        }
    );
    s
}

/// `cavea-server verify ARGS`: parse, verify, write the report to `out` and
/// problems to `err`, and return the exit code. `database_url` is
/// `CAVEA_DATABASE_URL`; it is never printed.
pub async fn run(
    args: &[String],
    database_url: Option<&str>,
    out: &mut (dyn std::io::Write + Send),
    err: &mut (dyn std::io::Write + Send),
) -> u8 {
    let mut target = None;
    let (mut workspace, mut event) = (None, None);
    let (mut json, mut booking_data) = (false, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--workspace" => workspace = it.next().cloned(),
            "--event" => event = it.next().cloned(),
            "--event-id" => {
                let id = it
                    .next()
                    .map(|s| s.trim_start_matches("ev_").replace('-', ""));
                match id.and_then(|s| {
                    (s.len() == 32)
                        .then(|| u128::from_str_radix(&s, 16).ok())
                        .flatten()
                }) {
                    Some(id) => target = Some(Target::Id(EventId(id))),
                    None => {
                        let _ = writeln!(err, "--event-id takes 32 hex digits or a UUID\n{USAGE}");
                        return EXIT_USAGE;
                    }
                }
            }
            "--all" => target = Some(Target::All),
            "--json" => json = true,
            "--include-booking-data" => booking_data = true,
            "-h" | "--help" => {
                let _ = writeln!(out, "{USAGE}");
                return EXIT_CLEAN;
            }
            other => {
                let _ = writeln!(err, "unknown argument {other:?}\n{USAGE}");
                return EXIT_USAGE;
            }
        }
    }
    match (target.is_some(), workspace, event) {
        (false, Some(workspace), Some(event)) => target = Some(Target::Key { workspace, event }),
        (true, None, None) => {}
        _ => {
            let _ = writeln!(err, "{USAGE}");
            return EXIT_USAGE;
        }
    }
    let Some(target) = target else {
        let _ = writeln!(err, "{USAGE}");
        return EXIT_USAGE;
    };
    let Some(url) = database_url else {
        let _ = writeln!(err, "CAVEA_DATABASE_URL is required");
        return EXIT_COULD_NOT_RUN;
    };
    let Ok(url) = url.parse::<DatabaseUrl>() else {
        let _ = writeln!(
            err,
            "CAVEA_DATABASE_URL is not a valid PostgreSQL connection URL"
        );
        return EXIT_COULD_NOT_RUN;
    };
    let pool = match crate::store::connect(url.options(), 2).await {
        Ok(pool) => pool,
        Err(e) => {
            let _ = writeln!(
                err,
                "cannot connect to PostgreSQL: {}",
                VerifyError::Unavailable(e.to_string())
            );
            return EXIT_COULD_NOT_RUN;
        }
    };
    let charts = PgCharts::new(pool.clone(), CHART_CACHE_OBJECTS);
    let ids = match events(&pool, &target).await {
        Ok(ids) => ids,
        Err(e) => {
            let _ = writeln!(err, "{e}");
            return EXIT_COULD_NOT_RUN;
        }
    };
    let mut report = Report {
        version: 1,
        clean: true,
        events: Vec::with_capacity(ids.len()),
    };
    let mut failed = false;
    for id in ids {
        match verify_event(&pool, &charts, id, booking_data).await {
            Ok(e) => {
                report.clean &= e.clean;
                report.events.push(e);
            }
            Err(e) => {
                let _ = writeln!(err, "event {:032x}: {e}", id.0);
                failed = true;
            }
        }
    }
    pool.close().await;
    let written = if json {
        serde_json::to_writer_pretty(&mut *out, &report)
            .map_err(std::io::Error::from)
            .and_then(|()| writeln!(out))
    } else {
        out.write_all(summary(&report).as_bytes())
    };
    if written.is_err() || failed {
        EXIT_COULD_NOT_RUN
    } else if report.clean {
        EXIT_CLEAN
    } else {
        EXIT_DIFFERENCES
    }
}
