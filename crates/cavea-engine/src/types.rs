//! Domain types: identifiers, statuses, commands, replies and failure reasons
//! (`docs/02-domain-model.md` §3 to §5, `docs/08-server.md` §4.2 and §6.4).

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use tokio::sync::watch;

/// Dense per-chart-version object index, 0 to N−1 (02 §3).
pub type Idx = u32;
/// Chart-stable object identifier (02 §3).
pub type Oid = u32;

/// The event's database id (`events.id`, a UUID).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventId(pub u128);

/// A hold's id: the `seq` of the `CreateHold` command that made it (08 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HoldId(pub u64);

/// SHA-256 of a hold token. The engine never sees the token itself (02 §5).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenHash(pub [u8; 32]);

impl fmt::Debug for TokenHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print the hash: logs must not let anyone correlate a token.
        f.write_str("TokenHash(..)")
    }
}

/// Milliseconds since the Unix epoch, as read from the injected [`Clock`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub u64);

impl Timestamp {
    #[must_use]
    pub fn plus_secs(self, secs: u32) -> Self {
        Self(self.0.saturating_add(u64::from(secs) * 1000))
    }
}

/// The engine's only source of time. Production passes the server clock; tests
/// and the simulation harness pass [`ManualClock`].
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> Timestamp;
    /// Resolves once `now() >= t`. The actor waits on it for the next hold
    /// expiry, so expiries follow this clock, never the wall clock.
    fn sleep_until(&self, t: Timestamp) -> impl Future<Output = ()> + Send;
}

/// A clock that moves only when told to.
#[derive(Debug, Clone)]
pub struct ManualClock(Arc<watch::Sender<u64>>);

impl ManualClock {
    #[must_use]
    pub fn new(start: Timestamp) -> Self {
        Self(Arc::new(watch::Sender::new(start.0)))
    }
    pub fn set(&self, t: Timestamp) {
        self.0.send_replace(t.0);
    }
    pub fn advance_ms(&self, ms: u64) {
        self.0.send_modify(|now| *now = now.saturating_add(ms));
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new(Timestamp(0))
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp(*self.0.borrow())
    }

    fn sleep_until(&self, t: Timestamp) -> impl Future<Output = ()> + Send {
        let mut rx = self.0.subscribe();
        async move {
            // The sender lives as long as `self`; an error cannot happen while
            // anyone can still advance the clock.
            let _ = rx.wait_for(|&now| now >= t.0).await;
        }
    }
}

/// A domain status code, 0 to 15 (02 §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Status(u8);

impl Status {
    pub const FREE: Self = Self(0);
    pub const HELD: Self = Self(1);
    pub const BOOKED: Self = Self(2);
    pub const BLOCKED: Self = Self(3);

    /// Any code from 0 to 15; custom statuses are 4 to 15.
    #[must_use]
    pub fn from_code(code: u8) -> Option<Self> {
        (code <= 15).then_some(Self(code))
    }

    #[must_use]
    pub fn code(self) -> u8 {
        self.0
    }
}

/// How a request names an object (08 §6.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectRef {
    Label(Box<str>),
    Idx(Idx),
}

/// Bounds from 02 §5 and §14.
pub mod limits {
    /// Shortest hold lifetime, in seconds.
    pub const MIN_HOLD_TTL_SECS: u32 = 60;
    /// Longest hold lifetime, in seconds.
    pub const MAX_HOLD_TTL_SECS: u32 = 7_200;
    /// Most objects one command may name.
    pub const MAX_OBJECTS_PER_COMMAND: usize = 1_000;
    /// Longest order reference, in characters (02 §6).
    pub const MAX_ORDER_REF_CHARS: usize = 128;
    /// Longest ticket type key, in characters (02 §6).
    pub const MAX_TICKET_TYPE_CHARS: usize = 128;
    /// Largest `extra` per object, in bytes (02 §14).
    pub const MAX_EXTRA_BYTES: usize = 2_048;
    /// Longest idempotency key, in characters (08 §6.1).
    pub const MAX_IDEMPOTENCY_KEY_CHARS: usize = 255;
    /// How long an idempotency record matches, in milliseconds (08 §6.1).
    pub const IDEMPOTENCY_TTL_MS: u64 = 24 * 60 * 60 * 1000;
    /// Most idempotency records one event keeps in memory (08 §6.1).
    pub const IDEMPOTENCY_WINDOW: usize = 50_000;
    /// A record younger than this is never dropped from memory to make room; a
    /// full window of such records refuses new keys with `overloaded`.
    pub const IDEMPOTENCY_MIN_RETENTION_MS: u64 = 60_000;
}

/// Who issued a command, for the "Who may issue it" column of 02 §4.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authority {
    /// A publishable key: create hold tokens, hold and release with a token.
    Publishable,
    /// A secret key or a signed-in console user: every command.
    Secret,
}

/// An `Idempotency-Key` and the caller's fingerprint of the request it came
/// with (08 §6.1). The engine compares fingerprints; it never computes one.
#[derive(Clone, PartialEq, Eq)]
pub struct Idempotency {
    pub key: Arc<str>,
    pub fingerprint: [u8; 32],
}

impl fmt::Debug for Idempotency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Idempotency(..)")
    }
}

/// A command and the identity recorded in the log (`event_log.actor`).
#[derive(Debug, Clone)]
pub struct Request {
    /// Key id, user id or `system`.
    pub actor: Arc<str>,
    pub authority: Authority,
    pub command: Command,
    pub idempotency: Option<Idempotency>,
}

/// Ticket type and `extra` for one object of a booking (02 §6).
#[derive(Clone, PartialEq, Eq)]
pub struct BookDetail {
    pub object: ObjectRef,
    pub ticket_type: Option<Arc<str>>,
    /// A JSON object as text. The caller checks that it is JSON; the engine
    /// bounds its size and stores it as given.
    pub extra: Option<Arc<str>>,
}

impl fmt::Debug for BookDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BookDetail")
            .field("object", &self.object)
            .field("ticket_type", &self.ticket_type)
            .finish_non_exhaustive()
    }
}

/// The commands the engine executes (08 §4.2). The engine enforces which
/// [`Authority`] may issue each one (02 §4.3); key configuration (the per-key
/// object cap, the workspace's longest hold, channels) is the caller's job.
#[derive(Clone)]
pub enum Command {
    /// Register a new hold token. `max_objects: None` means unlimited.
    CreateHold {
        token: TokenHash,
        ttl_secs: u32,
        max_objects: Option<u32>,
    },
    /// Free → held under the token.
    Hold {
        token: TokenHash,
        objects: Vec<ObjectRef>,
    },
    /// Held → free for objects held by the token; an empty list releases all
    /// of the token's objects. The token stays valid.
    Release {
        token: TokenHash,
        objects: Vec<ObjectRef>,
    },
    /// Set the token's expiry to `now + ttl_secs`.
    ExtendHold { token: TokenHash, ttl_secs: u32 },
    /// Held → booked with a token (every named object, or all of the token's
    /// objects if none are named), or free → booked without one (08 §7.2).
    Book {
        token: Option<TokenHash>,
        objects: Vec<ObjectRef>,
        order_ref: Arc<str>,
        /// At most one per object, each naming an object being booked.
        details: Vec<BookDetail>,
    },
    /// Booked → free, without a token. Every named object must be booked.
    ReleaseBooked { objects: Vec<ObjectRef> },
    /// Free → blocked or a custom status, or back to free (08 §7.2).
    ChangeStatus {
        objects: Vec<ObjectRef>,
        status: Status,
    },
}

impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Only the operation: commands carry order references and `extra`.
        write!(f, "Command::{}", self.op().name())
    }
}

impl Command {
    #[must_use]
    pub fn op(&self) -> Op {
        match self {
            Self::CreateHold { .. } => Op::CreateHold,
            Self::Hold { .. } => Op::Hold,
            Self::Release { .. } | Self::ReleaseBooked { .. } => Op::Release,
            Self::ExtendHold { .. } => Op::ExtendHold,
            Self::Book { .. } => Op::Book,
            Self::ChangeStatus { .. } => Op::ChangeStatus,
        }
    }

    /// The "Who may issue it" column of 02 §4.3.
    #[must_use]
    pub fn permits(&self, authority: Authority) -> bool {
        match self {
            Self::CreateHold { .. } | Self::Hold { .. } | Self::Release { .. } => true,
            Self::ExtendHold { .. }
            | Self::Book { .. }
            | Self::ReleaseBooked { .. }
            | Self::ChangeStatus { .. } => authority == Authority::Secret,
        }
    }
}

/// Stable operation codes for `event_log.op`. Never renumber.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Op {
    CreateHold = 1,
    Hold = 2,
    Release = 3,
    ExtendHold = 4,
    Book = 5,
    ChangeStatus = 6,
    /// Internal: a hold reached its expiry (08 §7.3).
    Expire = 7,
}

impl Op {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::CreateHold => "create_hold",
            Self::Hold => "hold",
            Self::Release => "release",
            Self::ExtendHold => "extend_hold",
            Self::Book => "book",
            Self::ChangeStatus => "change_status",
            Self::Expire => "expire",
        }
    }
}

/// A successful reply. `seq` is the command's own sequence number, or, for a
/// command that changed nothing, the sequence number of the state it saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    HoldCreated {
        seq: u64,
        hold: HoldId,
        expires_at: Timestamp,
    },
    /// Every named object (deduplicated, ascending `idx`), including ones the
    /// token already held.
    Held {
        seq: u64,
        expires_at: Timestamp,
        objects: Vec<Idx>,
    },
    /// The objects that were released, ascending `idx`.
    Released {
        seq: u64,
        objects: Vec<Idx>,
    },
    Extended {
        seq: u64,
        expires_at: Timestamp,
    },
    /// The objects booked, ascending `idx`.
    Booked {
        seq: u64,
        objects: Vec<Idx>,
    },
    /// Every named object (deduplicated, ascending `idx`), including ones
    /// already in the requested status.
    StatusChanged {
        seq: u64,
        objects: Vec<Idx>,
    },
}

pub type Reply = Result<Outcome, Failure>;

/// One object that failed its condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectFailure {
    pub idx: Idx,
    pub reason: ObjectReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectReason {
    NotFree,
    NotHeldByToken,
    NotBooked,
    /// Neither blocked nor a custom status.
    NotBlocked,
}

impl ObjectReason {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::NotFree => "not_free",
            Self::NotHeldByToken => "not_held_by_token",
            Self::NotBooked => "not_booked",
            Self::NotBlocked => "not_blocked",
        }
    }

    /// The refusal listing `objects`, all of which failed for this reason.
    #[must_use]
    pub fn failure(self, objects: Vec<ObjectFailure>) -> Failure {
        match self {
            Self::NotFree => Failure::ObjectNotFree(objects),
            Self::NotHeldByToken => Failure::ObjectNotHeldByToken(objects),
            Self::NotBooked | Self::NotBlocked => Failure::ObjectStatusConflict(objects),
        }
    }
}

/// What a `not_found` failure could not find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Missing {
    HoldToken,
    /// Every reference that did not resolve, in request order.
    Objects(Vec<ObjectRef>),
}

/// Why a request was malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidReason {
    NoObjects,
    TooManyObjects,
    TtlOutOfRange,
    ZeroObjectLimit,
    TokenAlreadyExists,
    OrderRefLength,
    TicketTypeLength,
    ExtraTooLarge,
    /// A booking detail names an object that is not being booked, or one
    /// object twice.
    DetailMismatch,
    /// Change-status may only set free, blocked or a custom status.
    BadStatus,
    IdempotencyKeyLength,
}

impl InvalidReason {
    /// `details.reason` of `invalid_request` (08 §6.4).
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::NoObjects => "no_objects",
            Self::TooManyObjects => "too_many_objects",
            Self::TtlOutOfRange => "ttl_out_of_range",
            Self::ZeroObjectLimit => "zero_object_limit",
            Self::TokenAlreadyExists => "token_already_exists",
            Self::OrderRefLength => "order_ref_length",
            Self::TicketTypeLength => "ticket_type_length",
            Self::ExtraTooLarge => "extra_too_large",
            Self::DetailMismatch => "detail_mismatch",
            Self::BadStatus => "bad_status",
            Self::IdempotencyKeyLength => "idempotency_key_length",
        }
    }
}

/// A refused command. Each variant is exactly one stable code of 08 §6.4. A
/// refused command changed nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    InvalidRequest(InvalidReason),
    /// The request's [`Authority`] may not issue this command.
    Forbidden,
    NotFound(Missing),
    ObjectNotFree(Vec<ObjectFailure>),
    ObjectNotHeldByToken(Vec<ObjectFailure>),
    /// An object's status does not allow the change (not booked for a release
    /// without a token, not blocked for a change back to free).
    ObjectStatusConflict(Vec<ObjectFailure>),
    IdempotencyMismatch,
    HoldTokenExpired,
    HoldLimitExceeded {
        limit: u32,
        requested: u64,
    },
    Overloaded,
    Unavailable,
}

impl Failure {
    /// The stable error code (08 §6.4).
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "invalid_request",
            Self::Forbidden => "forbidden",
            Self::NotFound(_) => "not_found",
            Self::ObjectNotFree(_) => "object_not_free",
            Self::ObjectNotHeldByToken(_) => "object_not_held_by_token",
            Self::ObjectStatusConflict(_) => "object_status_conflict",
            Self::IdempotencyMismatch => "idempotency_mismatch",
            Self::HoldTokenExpired => "hold_token_expired",
            Self::HoldLimitExceeded { .. } => "hold_limit_exceeded",
            Self::Overloaded => "overloaded",
            Self::Unavailable => "unavailable",
        }
    }

    #[must_use]
    pub fn http_status(&self) -> u16 {
        match self {
            Self::InvalidRequest(_) => 400,
            Self::Forbidden => 403,
            Self::NotFound(_) => 404,
            Self::ObjectNotFree(_)
            | Self::ObjectNotHeldByToken(_)
            | Self::ObjectStatusConflict(_)
            | Self::IdempotencyMismatch => 409,
            Self::HoldTokenExpired => 410,
            Self::HoldLimitExceeded { .. } => 422,
            Self::Overloaded | Self::Unavailable => 503,
        }
    }
}
