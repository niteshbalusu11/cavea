//! The storage boundary (08 §4.4, §4.7, §5). A batch is plain data that maps
//! one-to-one onto the statements of the commit transaction.

use std::future::Future;
use std::sync::Arc;

use crate::chart::ChartView;
use crate::types::{ChartVersionId, EventId, HoldId, Oid, Op, Reply, Status, Timestamp, TokenHash};

/// A row of `holds`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldRow {
    pub id: HoldId,
    pub token_hash: TokenHash,
    pub expires_at: Timestamp,
    /// `None`: unlimited.
    pub max_objects: Option<u32>,
}

/// The `order_ref`, `ticket_type` and `extra` columns of a booked object.
#[derive(Clone, PartialEq, Eq)]
pub struct Booking {
    pub order_ref: Arc<str>,
    pub ticket_type: Option<Arc<str>>,
    /// Opaque JSON text, at most 2 KB.
    pub extra: Option<Arc<str>>,
}

impl std::fmt::Debug for Booking {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Order references and `extra` are customer data: never printed.
        f.debug_struct("Booking")
            .field("ticket_type", &self.ticket_type)
            .finish_non_exhaustive()
    }
}

/// A row of `object_state` for an object that is not in the default state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRow {
    pub oid: Oid,
    pub status: Status,
    pub hold_id: Option<HoldId>,
    /// Present exactly when `status` is booked.
    pub booking: Option<Booking>,
    /// The last committed command that changed the object.
    pub seq: u64,
}

/// A row of `idempotency` (08 §6.1): the reply a keyed command produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyRecord {
    pub key: Arc<str>,
    pub fingerprint: [u8; 32],
    /// The engine clock when the command was applied.
    pub created_at: Timestamp,
    /// The store encodes it so that loading decodes the same value.
    pub reply: Reply,
}

/// A row of `event_log`: one per committed command. Debug output leaves out
/// the order reference.
#[derive(Clone, PartialEq, Eq)]
pub struct LogRecord {
    pub seq: u64,
    pub op: Op,
    pub actor: Arc<str>,
    pub hold_id: Option<HoldId>,
    /// Objects the command changed.
    pub oids: Vec<Oid>,
    /// The hold's expiry after the command, for `CreateHold` and `ExtendHold`.
    pub expires_at: Option<Timestamp>,
    /// For `CreateHold`: the hold's object limit, `None` for unlimited, so
    /// that replaying the log rebuilds `holds` (the token hash aside).
    pub max_objects: Option<u32>,
    /// For `Book`.
    pub order_ref: Option<Arc<str>>,
    /// The status set, for `ChangeStatus` (`detail`).
    pub status: Option<Status>,
    /// For `Book`: `(oid, ticket_type, extra)` of each object that has either
    /// (`detail`), so replaying the log rebuilds `object_state`.
    pub details: Vec<LogDetail>,
}

/// `(oid, ticket_type, extra)` of one booked object.
pub type LogDetail = (Oid, Option<Arc<str>>, Option<Arc<str>>);

impl std::fmt::Debug for LogRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogRecord")
            .field("seq", &self.seq)
            .field("op", &self.op)
            .field("actor", &self.actor)
            .field("hold_id", &self.hold_id)
            .field("oids", &self.oids)
            .field("expires_at", &self.expires_at)
            .field("max_objects", &self.max_objects)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// One commit: the net effect of every command in it, as of its last command.
/// Rows within each list are unique and in ascending id order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub event: EventId,
    /// Fence: the commit must fail if `events.owner_epoch` differs.
    pub epoch: u64,
    /// `events.seq` after this batch; equal to the last log record's `seq`.
    pub seq: u64,
    pub holds_created: Vec<HoldRow>,
    /// Existing holds with a new expiry.
    pub holds_extended: Vec<(HoldId, Timestamp)>,
    pub holds_deleted: Vec<HoldId>,
    /// Upserts into `object_state`.
    pub objects_set: Vec<ObjectRow>,
    /// Objects that returned to the default state: deleted from `object_state`.
    pub objects_freed: Vec<Oid>,
    /// By status code: how many more objects have it after the batch than
    /// before (negative for fewer), so that the store keeps each event's
    /// count per status with its rows (`events.counts`). Sums to 0.
    pub counts: [i32; 16],
    /// Contiguous: `log[i].seq == seq - (log.len() - 1 - i)`. Empty only when
    /// the batch carries nothing but idempotency records of refusals; `seq` is
    /// then unchanged.
    pub log: Vec<LogRecord>,
    /// Statement 8: one per keyed command applied, refusals included, unique by
    /// key. Kept by the store for 24 hours.
    pub idempotency: Vec<IdempotencyRecord>,
}

/// Ownership of an event, taken before its state is read (08 §4.7 step 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Acquired {
    /// The new `owner_epoch`: every later commit and renewal is fenced on it.
    pub epoch: u64,
    /// The chart version the event is pinned to.
    pub chart_version: ChartVersionId,
}

/// An event's stored state, read when its actor is loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedEvent {
    pub epoch: u64,
    pub seq: u64,
    pub holds: Vec<HoldRow>,
    pub objects: Vec<ObjectRow>,
    /// The event's idempotency records younger than 24 hours. Older ones are
    /// ignored if present.
    pub idempotency: Vec<IdempotencyRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The database could not be reached or the transaction failed. Whether the
    /// batch was written is unknown.
    Unavailable(String),
    /// The fencing update matched no row: another node owns the event.
    Fenced,
    /// Ownership was refused: another node holds a live lease on the event.
    Leased,
    UnknownEvent,
    /// The batch does not follow the stored state (for example a `seq` gap).
    Inconsistent(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(why) => write!(f, "store unavailable: {why}"),
            Self::Fenced => f.write_str("fencing check failed: event owned by another node"),
            Self::Leased => f.write_str("event leased by another node"),
            Self::UnknownEvent => f.write_str("unknown event"),
            Self::Inconsistent(why) => write!(f, "batch inconsistent with stored state: {why}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// Where an actor loads its event and commits its batches. Each store speaks
/// for one node (`owner_node`), and judges leases by its own clock: the
/// database's, never the node's (08 §4.8).
pub trait SeatStore: Send + Sync + 'static {
    /// Take ownership of the event (08 §4.7 step 1): raise `owner_epoch`, set
    /// the owner to this node and the lease to the store's lease period.
    /// Refused with [`StoreError::Leased`] while another node holds a live
    /// lease. Taking it again from this node always succeeds and fences this
    /// node's earlier actor, including a commit of it that lands late.
    fn acquire(&self, event: EventId) -> impl Future<Output = Result<Acquired, StoreError>> + Send;

    /// Read the event's stored state in one snapshot. Takes no ownership.
    fn load_event(
        &self,
        event: EventId,
    ) -> impl Future<Output = Result<LoadedEvent, StoreError>> + Send;

    /// Commit one batch as one atomic unit (08 §4.4), fenced on `batch.epoch`,
    /// and renew the lease.
    fn commit_batch(&self, batch: Batch) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Renew the lease held at `epoch`; [`StoreError::Fenced`] if it was lost.
    fn heartbeat(
        &self,
        event: EventId,
        epoch: u64,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// End the lease held at `epoch`, so that any node may take the event at
    /// once. [`StoreError::Fenced`] if it was already lost.
    fn release(
        &self,
        event: EventId,
        epoch: u64,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
}

/// Where a loading actor gets its chart (08 §4.7 step 2). An implementation
/// caches one [`ChartView`] per version: every event on a version shares it.
pub trait ChartProvider: Send + Sync + 'static {
    fn chart(
        &self,
        version: ChartVersionId,
    ) -> impl Future<Output = Result<Arc<ChartView>, StoreError>> + Send;
}
