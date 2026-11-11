//! The PostgreSQL seat store (08 §3, §4.4, §4.7, §5): the connection pool,
//! embedded migrations, and [`PgStore`], the production [`SeatStore`].
//!
//! Values cross the boundary as plain integers, bytes and text: event ids as
//! 32 hex digits cast to `uuid`, times as Unix milliseconds converted in SQL,
//! JSON as text cast to `json`. That keeps `sqlx`'s type integrations
//! (and a date-time crate) out of the build.

mod migrations;
mod reply;

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use cavea_engine::{
    Batch, Booking, EventId, HoldId, HoldRow, IdempotencyRecord, LoadedEvent, LogRecord, ObjectRow,
    SeatStore, Status, StoreError, Timestamp, TokenHash,
};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use tracing::Instrument as _;

pub use migrations::{MigrationError, migrate};

/// The longest a commit may take, from waiting for a connection to the reply,
/// before it fails as `unavailable`. Well under the engine's one-minute
/// idempotency retention (08 §6.1), which assumes commits time out within it.
pub const COMMIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Open the connection pool: `size` connections at most, the first one now so
/// that an unreachable database stops startup.
pub async fn connect(options: PgConnectOptions, size: u32) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(size)
        .acquire_timeout(COMMIT_TIMEOUT)
        .connect_with(options)
        .instrument(cavea_telemetry::db_span("connect"))
        .await
}

/// [`SeatStore`] on PostgreSQL. Cheap to clone.
#[derive(Debug, Clone)]
pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl SeatStore for PgStore {
    async fn load_event(&self, event: EventId) -> Result<LoadedEvent, StoreError> {
        load(&self.pool, event)
            .instrument(cavea_telemetry::db_span("load_event"))
            .await
    }

    async fn commit_batch(&self, batch: Batch) -> Result<(), StoreError> {
        let params = Params::new(&batch)?;
        let commit = commit(&self.pool, &params).instrument(cavea_telemetry::db_span("commit"));
        // On a timeout the statement may still commit on the server. The
        // actor stops and the event is reloaded (08 §4.6); a batch landing
        // after that reload fails nothing, because the reloaded actor's next
        // commit then fails its `seq` check and reloads again.
        tokio::time::timeout(COMMIT_TIMEOUT, commit)
            .await
            .unwrap_or_else(|_| {
                Err(StoreError::Unavailable(
                    "commit timed out; whether it was written is unknown".into(),
                ))
            })
    }
}

/// The commit (08 §4.4) as one statement, so one round trip and its own
/// transaction. Every write is gated on the fence (statement 1): if the event
/// is owned at another epoch, its `seq` is not the one the batch continues, or
/// a hold the batch extends or deletes is missing, nothing is written and the
/// statement returns 0. Any error aborts the whole statement.
const COMMIT: &str = "
WITH fence AS (                                                 -- 1
  UPDATE events SET seq = $4
   WHERE id = $1::uuid AND owner_epoch = $2 AND seq = $3
     AND (SELECT count(*) FROM holds
           WHERE event_id = $1::uuid AND id = ANY($9::int8[] || $11::int8[]))
         = cardinality($9::int8[]) + cardinality($11::int8[])
  RETURNING workspace_id
), holds_created AS (                                           -- 2
  INSERT INTO holds (event_id, id, token_hash, expires_at, max_objects)
  SELECT $1::uuid, u.id, u.token_hash,
         to_timestamp(0) + u.expires_ms * interval '1 millisecond', u.max_objects
    FROM unnest($5::int8[], $6::bytea[], $7::int8[], $8::int4[])
         AS u(id, token_hash, expires_ms, max_objects)
   WHERE EXISTS (SELECT FROM fence)
), holds_extended AS (
  UPDATE holds SET expires_at = to_timestamp(0) + u.expires_ms * interval '1 millisecond'
    FROM unnest($9::int8[], $10::int8[]) AS u(id, expires_ms)
   WHERE holds.event_id = $1::uuid AND holds.id = u.id AND EXISTS (SELECT FROM fence)
), holds_deleted AS (
  DELETE FROM holds
   WHERE event_id = $1::uuid AND id = ANY($11::int8[]) AND EXISTS (SELECT FROM fence)
), objects_set AS (                                             -- 3
  -- `channel` has no producer yet (AssignChannel); left as it is.
  INSERT INTO object_state (event_id, oid, status, hold_id, order_ref, ticket_type, extra, seq)
  SELECT $1::uuid, u.oid, u.status, u.hold_id, u.order_ref, u.ticket_type, u.extra::json, u.seq
    FROM unnest($12::int4[], $13::int2[], $14::int8[], $15::text[], $16::text[], $17::text[],
                $18::int8[])
         AS u(oid, status, hold_id, order_ref, ticket_type, extra, seq)
   WHERE EXISTS (SELECT FROM fence)
  ON CONFLICT (event_id, oid) DO UPDATE SET
    status = EXCLUDED.status, hold_id = EXCLUDED.hold_id, order_ref = EXCLUDED.order_ref,
    ticket_type = EXCLUDED.ticket_type, extra = EXCLUDED.extra, seq = EXCLUDED.seq
), objects_freed AS (                                           -- 4
  DELETE FROM object_state
   WHERE event_id = $1::uuid AND oid = ANY($19::int4[]) AND EXISTS (SELECT FROM fence)
), log AS (                                                     -- 6
  -- 5, area allocations, has no producer yet.
  INSERT INTO event_log (event_id, seq, op, actor, hold_id, order_ref, oids, detail)
  SELECT $1::uuid, u.seq, u.op, u.actor, u.hold_id, u.order_ref,
         ($25::int4[])[u.lo:u.hi], u.detail::json
    FROM unnest($20::int8[], $21::int2[], $22::text[], $23::int8[], $24::text[], $26::int4[],
                $27::int4[], $28::text[])
         AS u(seq, op, actor, hold_id, order_ref, lo, hi, detail)
   WHERE EXISTS (SELECT FROM fence)
), idempotency AS (                                             -- 8
  -- 7, webhook outbox rows, has no producer yet.
  -- A live record is never overwritten: a key another event used is kept.
  INSERT INTO idempotency (workspace_id, event_id, key, request_hash, status, response, created_at)
  SELECT (SELECT workspace_id FROM fence), $1::uuid, u.key, u.request_hash, u.status,
         u.response::json, to_timestamp(0) + u.created_ms * interval '1 millisecond'
    FROM unnest($29::text[], $30::bytea[], $31::int4[], $32::text[], $33::int8[])
         AS u(key, request_hash, status, response, created_ms)
   WHERE EXISTS (SELECT FROM fence)
  ON CONFLICT (workspace_id, key) DO UPDATE SET
    event_id = EXCLUDED.event_id, request_hash = EXCLUDED.request_hash,
    status = EXCLUDED.status, response = EXCLUDED.response, created_at = EXCLUDED.created_at
   WHERE idempotency.created_at <= EXCLUDED.created_at - interval '24 hours'
)
SELECT count(*) FROM fence";

/// Why the fence matched nothing. Read only after a refused commit.
const DIAGNOSE: &str = "
SELECT owner_epoch, seq FROM events WHERE id = $1::uuid";

async fn commit(pool: &PgPool, p: &Params) -> Result<(), StoreError> {
    let written: i64 = sqlx::query_scalar(COMMIT)
        .bind(&p.event)
        .bind(p.epoch)
        .bind(p.prev_seq)
        .bind(p.seq)
        .bind(&p.hold_ids)
        .bind(&p.hold_tokens)
        .bind(&p.hold_expires)
        .bind(&p.hold_max)
        .bind(&p.extended_ids)
        .bind(&p.extended_expires)
        .bind(&p.deleted_ids)
        .bind(&p.set_oids)
        .bind(&p.set_status)
        .bind(&p.set_hold)
        .bind(&p.set_order_ref)
        .bind(&p.set_ticket_type)
        .bind(&p.set_extra)
        .bind(&p.set_seq)
        .bind(&p.freed_oids)
        .bind(&p.log_seq)
        .bind(&p.log_op)
        .bind(&p.log_actor)
        .bind(&p.log_hold)
        .bind(&p.log_order_ref)
        .bind(&p.log_oids)
        .bind(&p.log_lo)
        .bind(&p.log_hi)
        .bind(&p.log_detail)
        .bind(&p.key)
        .bind(&p.key_hash)
        .bind(&p.key_status)
        .bind(&p.key_response)
        .bind(&p.key_created)
        .fetch_one(pool)
        .await
        .map_err(store_error)?;
    if written == 1 {
        return Ok(());
    }
    let stored: Option<(i64, i64)> = sqlx::query_as(DIAGNOSE)
        .bind(&p.event)
        .fetch_optional(pool)
        .await
        .map_err(store_error)?;
    Err(match stored {
        None => StoreError::UnknownEvent,
        Some((epoch, _)) if epoch != p.epoch => StoreError::Fenced,
        Some((_, seq)) if seq != p.prev_seq => {
            StoreError::Inconsistent("log does not continue the stored seq".into())
        }
        Some(_) => StoreError::Inconsistent("extended or deleted an unknown hold".into()),
    })
}

/// The commit's parameters: the batch as one array per column.
#[derive(Default)]
struct Params {
    event: String,
    epoch: i64,
    /// The stored `seq` the batch continues.
    prev_seq: i64,
    seq: i64,
    hold_ids: Vec<i64>,
    hold_tokens: Vec<Vec<u8>>,
    hold_expires: Vec<i64>,
    hold_max: Vec<Option<i32>>,
    extended_ids: Vec<i64>,
    extended_expires: Vec<i64>,
    deleted_ids: Vec<i64>,
    set_oids: Vec<i32>,
    set_status: Vec<i16>,
    set_hold: Vec<Option<i64>>,
    set_order_ref: Vec<Option<String>>,
    set_ticket_type: Vec<Option<String>>,
    set_extra: Vec<Option<String>>,
    set_seq: Vec<i64>,
    freed_oids: Vec<i32>,
    log_seq: Vec<i64>,
    log_op: Vec<i16>,
    log_actor: Vec<String>,
    log_hold: Vec<Option<i64>>,
    log_order_ref: Vec<Option<String>>,
    /// Every log row's `oids`, concatenated; row i is `log_oids[lo..=hi]`,
    /// 1-based (a two-dimensional array must be rectangular).
    log_oids: Vec<i32>,
    log_lo: Vec<i32>,
    log_hi: Vec<i32>,
    log_detail: Vec<Option<String>>,
    key: Vec<String>,
    key_hash: Vec<Vec<u8>>,
    key_status: Vec<i32>,
    key_response: Vec<String>,
    key_created: Vec<i64>,
}

impl Params {
    /// Check what needs no database (INV-5: the log is contiguous and ends at
    /// `seq`) and convert. The stored `seq` is checked by the fence.
    fn new(b: &Batch) -> Result<Self, StoreError> {
        let len = b.log.len() as u64;
        let prev = b.seq.checked_sub(len).ok_or_else(seq_error)?;
        let contiguous = b.log.iter().zip(prev + 1..).all(|(r, s)| r.seq == s);
        if !contiguous || (b.log.is_empty() && b.idempotency.is_empty()) {
            return Err(seq_error());
        }
        let mut p = Self {
            event: format!("{:032x}", b.event.0),
            epoch: int(b.epoch)?,
            prev_seq: int(prev)?,
            seq: int(b.seq)?,
            ..Self::default()
        };
        for h in &b.holds_created {
            p.hold_ids.push(int(h.id.0)?);
            p.hold_tokens.push(h.token_hash.0.to_vec());
            p.hold_expires.push(int(h.expires_at.0)?);
            p.hold_max.push(
                h.max_objects
                    .map(i32::try_from)
                    .transpose()
                    .map_err(range)?,
            );
        }
        for (id, expires_at) in &b.holds_extended {
            p.extended_ids.push(int(id.0)?);
            p.extended_expires.push(int(expires_at.0)?);
        }
        for id in &b.holds_deleted {
            p.deleted_ids.push(int(id.0)?);
        }
        for o in &b.objects_set {
            p.set_oids.push(oid(o.oid));
            p.set_status.push(i16::from(o.status.code()));
            p.set_hold.push(o.hold_id.map(|h| int(h.0)).transpose()?);
            let booking = o.booking.as_ref();
            p.set_order_ref
                .push(booking.map(|b| b.order_ref.to_string()));
            p.set_ticket_type
                .push(booking.and_then(|b| b.ticket_type.as_deref().map(str::to_owned)));
            p.set_extra
                .push(booking.and_then(|b| b.extra.as_deref().map(str::to_owned)));
            p.set_seq.push(int(o.seq)?);
        }
        p.freed_oids = b.objects_freed.iter().map(|&o| oid(o)).collect();
        for r in &b.log {
            p.log_seq.push(int(r.seq)?);
            p.log_op.push(i16::from(r.op as u8));
            p.log_actor.push(r.actor.to_string());
            p.log_hold.push(r.hold_id.map(|h| int(h.0)).transpose()?);
            p.log_order_ref
                .push(r.order_ref.as_deref().map(str::to_owned));
            p.log_lo
                .push(i32::try_from(p.log_oids.len() + 1).map_err(range)?);
            p.log_oids.extend(r.oids.iter().map(|&o| oid(o)));
            p.log_hi
                .push(i32::try_from(p.log_oids.len()).map_err(range)?);
            p.log_detail.push(detail(r));
        }
        for r in &b.idempotency {
            p.key.push(r.key.to_string());
            p.key_hash.push(r.fingerprint.to_vec());
            p.key_status.push(reply::status(&r.reply));
            p.key_response.push(reply::encode(&r.reply));
            p.key_created.push(int(r.created_at.0)?);
        }
        Ok(p)
    }
}

/// `event_log.detail`: what replaying the log needs beyond the columns. Built
/// as text so that `extra`, already JSON, is embedded as it is.
fn detail(r: &LogRecord) -> Option<String> {
    let mut fields = Vec::new();
    if let Some(status) = r.status {
        fields.push(format!("\"status\":{}", status.code()));
    }
    if let Some(at) = r.expires_at {
        fields.push(format!("\"expires_at\":{}", at.0));
    }
    if !r.details.is_empty() {
        let mut objects = String::from("\"objects\":[");
        for (n, (oid, ticket_type, extra)) in r.details.iter().enumerate() {
            let ticket_type = ticket_type.as_deref().map_or_else(
                || "null".to_owned(),
                |t| serde_json::Value::from(t).to_string(),
            );
            let sep = if n == 0 { "" } else { "," };
            // Writing to a String cannot fail.
            let _ = write!(
                objects,
                "{sep}[{oid},{ticket_type},{}]",
                extra.as_deref().unwrap_or("null")
            );
        }
        objects.push(']');
        fields.push(objects);
    }
    (!fields.is_empty()).then(|| format!("{{{}}}", fields.join(",")))
}

/// Read the event's stored state from one snapshot (08 §4.7 step 3). Taking
/// ownership (step 1) is not done here yet: the epoch is read as stored.
async fn load(pool: &PgPool, event: EventId) -> Result<LoadedEvent, StoreError> {
    let id = format!("{:032x}", event.0);
    let mut tx = pool
        .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .await
        .map_err(store_error)?;
    let (epoch, seq): (i64, i64) =
        sqlx::query_as("SELECT owner_epoch, seq FROM events WHERE id = $1::uuid")
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_error)?
            .ok_or(StoreError::UnknownEvent)?;

    let holds: Vec<(i64, Vec<u8>, i64, Option<i32>)> = sqlx::query_as(
        "SELECT id, token_hash, (extract(epoch FROM expires_at) * 1000)::int8, max_objects
           FROM holds WHERE event_id = $1::uuid",
    )
    .bind(&id)
    .fetch_all(&mut *tx)
    .await
    .map_err(store_error)?;
    let holds = holds
        .into_iter()
        .map(|(id, token, expires, max)| {
            Ok(HoldRow {
                id: HoldId(unsigned(id)?),
                token_hash: TokenHash(token.try_into().map_err(|_| corrupt("token_hash"))?),
                expires_at: Timestamp(unsigned(expires)?),
                max_objects: max
                    .map(u32::try_from)
                    .transpose()
                    .map_err(|_| corrupt("max_objects"))?,
            })
        })
        .collect::<Result<_, StoreError>>()?;

    type ObjectTuple = (
        i32,
        i16,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
    );
    let objects: Vec<ObjectTuple> = sqlx::query_as(
        "SELECT oid, status, hold_id, order_ref, ticket_type, extra::text, seq
           FROM object_state WHERE event_id = $1::uuid",
    )
    .bind(&id)
    .fetch_all(&mut *tx)
    .await
    .map_err(store_error)?;
    let objects = objects
        .into_iter()
        .map(|(oid, status, hold, order_ref, ticket_type, extra, seq)| {
            let status = u8::try_from(status)
                .ok()
                .and_then(Status::from_code)
                .ok_or_else(|| corrupt("status"))?;
            Ok(ObjectRow {
                oid: oid as u32, // the stored bits of a u32
                status,
                hold_id: hold.map(unsigned).transpose()?.map(HoldId),
                booking: order_ref.map(|order_ref| Booking {
                    order_ref: order_ref.into(),
                    ticket_type: ticket_type.map(Arc::from),
                    extra: extra.map(Arc::from),
                }),
                seq: unsigned(seq)?,
            })
        })
        .collect::<Result<_, StoreError>>()?;

    // Records older than 24 hours by the database clock are left out; the
    // engine also ignores any its own clock finds expired.
    let records: Vec<(String, Vec<u8>, String, i64)> = sqlx::query_as(
        "SELECT key, request_hash, response::text, (extract(epoch FROM created_at) * 1000)::int8
           FROM idempotency
          WHERE event_id = $1::uuid AND created_at > now() - interval '24 hours'",
    )
    .bind(&id)
    .fetch_all(&mut *tx)
    .await
    .map_err(store_error)?;
    let idempotency = records
        .into_iter()
        .map(|(key, hash, response, created)| {
            Ok(IdempotencyRecord {
                key: key.into(),
                fingerprint: hash.try_into().map_err(|_| corrupt("request_hash"))?,
                created_at: Timestamp(unsigned(created)?),
                reply: reply::decode(&response).ok_or_else(|| corrupt("response"))?,
            })
        })
        .collect::<Result<_, StoreError>>()?;
    tx.commit().await.map_err(store_error)?;

    Ok(LoadedEvent {
        epoch: unsigned(epoch)?,
        seq: unsigned(seq)?,
        holds,
        objects,
        idempotency,
    })
}

/// A rejected statement (cardinality, data or integrity violation) wrote
/// nothing and will be rejected again; anything else, such as a lost
/// connection or a pool timeout, leaves the outcome unknown.
fn store_error(e: sqlx::Error) -> StoreError {
    if let sqlx::Error::Database(db) = &e
        && let Some(code) = db.code()
        && matches!(code.get(..2), Some("21" | "22" | "23"))
    {
        // The message names the constraint or type, never the values.
        return StoreError::Inconsistent(db.message().to_owned());
    }
    StoreError::Unavailable(e.to_string())
}

fn seq_error() -> StoreError {
    StoreError::Inconsistent("log does not continue the stored seq".into())
}

fn range(_: std::num::TryFromIntError) -> StoreError {
    StoreError::Inconsistent("value out of the column's range".into())
}

fn corrupt(column: &str) -> StoreError {
    StoreError::Inconsistent(format!("unreadable stored {column}"))
}

fn int(v: u64) -> Result<i64, StoreError> {
    i64::try_from(v).map_err(range)
}

fn unsigned(v: i64) -> Result<u64, StoreError> {
    u64::try_from(v).map_err(range)
}

/// `oid` is a u32 kept in an `int` column bit for bit (08 §5).
fn oid(oid: u32) -> i32 {
    oid as i32
}
