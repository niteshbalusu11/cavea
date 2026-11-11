//! The storage boundary (08 §4.4, §4.7, §5). A batch is plain data that maps
//! one-to-one onto the statements of the commit transaction.

use std::future::Future;
use std::sync::Arc;

use crate::types::{EventId, HoldId, Oid, Op, Reply, Status, Timestamp, TokenHash};

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
    /// For `Book`.
    pub order_ref: Option<Arc<str>>,
    /// The status set, for `ChangeStatus` (`detail`).
    pub status: Option<Status>,
}

impl std::fmt::Debug for LogRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogRecord")
            .field("seq", &self.seq)
            .field("op", &self.op)
            .field("actor", &self.actor)
            .field("hold_id", &self.hold_id)
            .field("oids", &self.oids)
            .field("expires_at", &self.expires_at)
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
    /// Contiguous: `log[i].seq == seq - (log.len() - 1 - i)`. Empty only when
    /// the batch carries nothing but idempotency records of refusals; `seq` is
    /// then unchanged.
    pub log: Vec<LogRecord>,
    /// Statement 8: one per keyed command applied, refusals included, unique by
    /// key. Kept by the store for 24 hours.
    pub idempotency: Vec<IdempotencyRecord>,
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
    UnknownEvent,
    /// The batch does not follow the stored state (for example a `seq` gap).
    Inconsistent(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(why) => write!(f, "store unavailable: {why}"),
            Self::Fenced => f.write_str("fencing check failed: event owned by another node"),
            Self::UnknownEvent => f.write_str("unknown event"),
            Self::Inconsistent(why) => write!(f, "batch inconsistent with stored state: {why}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// Where an actor loads its event and commits its batches.
pub trait SeatStore: Send + Sync + 'static {
    fn load_event(
        &self,
        event: EventId,
    ) -> impl Future<Output = Result<LoadedEvent, StoreError>> + Send;

    /// Commit one batch as one atomic unit (08 §4.4).
    fn commit_batch(&self, batch: Batch) -> impl Future<Output = Result<(), StoreError>> + Send;
}
