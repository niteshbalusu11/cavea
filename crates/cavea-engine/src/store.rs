//! The storage boundary (08 §4.4, §4.7, §5). A batch is plain data that maps
//! one-to-one onto the statements of the commit transaction.

use std::future::Future;
use std::sync::Arc;

use crate::types::{EventId, HoldId, Oid, Op, Status, Timestamp, TokenHash};

/// A row of `holds`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldRow {
    pub id: HoldId,
    pub token_hash: TokenHash,
    pub expires_at: Timestamp,
    /// `None`: unlimited.
    pub max_objects: Option<u32>,
}

/// A row of `object_state` for an object that is not in the default state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRow {
    pub oid: Oid,
    pub status: Status,
    pub hold_id: Option<HoldId>,
    /// The last committed command that changed the object.
    pub seq: u64,
}

/// A row of `event_log`: one per committed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    pub seq: u64,
    pub op: Op,
    pub actor: Arc<str>,
    pub hold_id: Option<HoldId>,
    /// Objects the command changed.
    pub oids: Vec<Oid>,
    /// The hold's expiry after the command, for `CreateHold` and `ExtendHold`.
    pub expires_at: Option<Timestamp>,
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
    /// Contiguous: `log[i].seq == seq - (log.len() - 1 - i)`.
    pub log: Vec<LogRecord>,
}

/// An event's stored state, read when its actor is loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedEvent {
    pub epoch: u64,
    pub seq: u64,
    pub holds: Vec<HoldRow>,
    pub objects: Vec<ObjectRow>,
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
