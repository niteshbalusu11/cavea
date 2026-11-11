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

mod actor;
mod chart;
mod memory;
mod state;
mod store;
mod types;

pub use actor::{ActorConfig, ActorError, ActorHandle, PendingReply, actor};
pub use chart::{ChartError, ChartView};
pub use memory::{Fault, MemoryStore};
pub use state::{EventState, HoldView, InvariantViolation, LoadError};
pub use store::{Batch, HoldRow, LoadedEvent, LogRecord, ObjectRow, SeatStore, StoreError};
pub use types::*;
