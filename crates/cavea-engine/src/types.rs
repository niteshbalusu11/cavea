//! Domain types: identifiers, statuses, commands, replies and failure reasons
//! (`docs/02-domain-model.md` §3 to §5, `docs/08-server.md` §4.2 and §6.4).

use std::fmt;
use std::sync::Arc;

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
pub trait Clock: Send + 'static {
    fn now(&self) -> Timestamp;
}

/// A clock that moves only when told to.
#[derive(Debug, Clone, Default)]
pub struct ManualClock(Arc<std::sync::atomic::AtomicU64>);

impl ManualClock {
    #[must_use]
    pub fn new(start: Timestamp) -> Self {
        Self(Arc::new(start.0.into()))
    }
    pub fn set(&self, t: Timestamp) {
        self.0.store(t.0, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn advance_ms(&self, ms: u64) {
        self.0.fetch_add(ms, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp(self.0.load(std::sync::atomic::Ordering::SeqCst))
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
}

/// A command and the identity recorded in the log (`event_log.actor`).
#[derive(Debug, Clone)]
pub struct Request {
    /// Key id, user id or `system`.
    pub actor: Arc<str>,
    pub command: Command,
}

/// The commands this slice executes (08 §4.2). Authorisation (which key may
/// issue which command, the per-key object cap) is the caller's job.
#[derive(Debug, Clone)]
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
}

impl Command {
    #[must_use]
    pub fn op(&self) -> Op {
        match self {
            Self::CreateHold { .. } => Op::CreateHold,
            Self::Hold { .. } => Op::Hold,
            Self::Release { .. } => Op::Release,
            Self::ExtendHold { .. } => Op::ExtendHold,
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
}

impl Op {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::CreateHold => "create_hold",
            Self::Hold => "hold",
            Self::Release => "release",
            Self::ExtendHold => "extend_hold",
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
}

impl ObjectReason {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::NotFree => "not_free",
            Self::NotHeldByToken => "not_held_by_token",
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
}

/// A refused command. Each variant is exactly one stable code of 08 §6.4. A
/// refused command changed nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    InvalidRequest(InvalidReason),
    NotFound(Missing),
    ObjectNotFree(Vec<ObjectFailure>),
    ObjectNotHeldByToken(Vec<ObjectFailure>),
    HoldTokenExpired,
    HoldLimitExceeded { limit: u32, requested: u64 },
    Overloaded,
    Unavailable,
}

impl Failure {
    /// The stable error code (08 §6.4).
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "invalid_request",
            Self::NotFound(_) => "not_found",
            Self::ObjectNotFree(_) => "object_not_free",
            Self::ObjectNotHeldByToken(_) => "object_not_held_by_token",
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
            Self::NotFound(_) => 404,
            Self::ObjectNotFree(_) | Self::ObjectNotHeldByToken(_) => 409,
            Self::HoldTokenExpired => 410,
            Self::HoldLimitExceeded { .. } => 422,
            Self::Overloaded | Self::Unavailable => 503,
        }
    }
}
