//! The seat engine: one single-writer actor per event holding seat state in
//! memory, the seat state machine, holds and their expiry, idempotency, and the
//! storage trait that group-commits to PostgreSQL. See `docs/08-server.md` and
//! `docs/02-domain-model.md`.
//!
//! Layers, innermost first:
//!
//! - [`EventState`]: the synchronous state machine. `apply` validates one
//!   command against memory at a given time and mutates it; `take_batch` seals
//!   the changes since the last call. No runtime, clock or I/O.
//! - [`SeatStore`]: loads an event and commits one [`Batch`] atomically.
//!   [`MemoryStore`] implements it with scriptable [`Fault`]s.
//! - [`actor`]: the loop that feeds a bounded mailbox through `EventState` and
//!   commits batches, releasing replies only after their commit.
//! - [`Registry`]: finds or loads the actor for an event, under ownership
//!   taken from the store; drops it when it fails, evicts it when idle, and
//!   stops every actor at shutdown. It also attaches realtime subscribers
//!   ([`Subscription`]) and serves the published snapshot and deltas, which
//!   the actor publishes from committed state only (`feed`, 08 §4.5, §8.3).

mod actor;
mod chart;
mod feed;
mod idempotency;
mod memory;
pub mod metrics;
mod registry;
mod replay;
mod state;
mod store;
mod types;

pub use actor::{ActorConfig, ActorError, ActorHandle, KeepAlive, PendingReply, actor};
pub use chart::{ChartError, ChartView};
pub use feed::{
    BEHIND_SUBSCRIBERS, End, Frame, INTERVAL_MS, MAX_INTERVAL_MS, MAX_WAIT_MS, QUEUE_FRAMES,
    RING_BYTES, RING_MS, Received, STRETCH_SUBSCRIBERS, Subscription, interval_ms,
};
pub use memory::{Fault, MEMORY_LEASE_MS, MemoryCharts, MemoryStore};
pub use registry::{OpenError, Registry, RegistryConfig};
pub use replay::{Difference, HoldField, ObjectField, Rebuilt, Replay, ReplayError, ReplayHold};
pub use state::{EventState, HoldView, InvariantViolation, LoadError};
pub use store::{
    Acquired, Batch, Booking, ChartProvider, HoldRow, IdempotencyRecord, LoadedEvent, LogDetail,
    LogRecord, ObjectRow, SeatStore, StoreError,
};
pub use types::*;
