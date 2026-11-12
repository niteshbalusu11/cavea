//! `application/problem+json` errors with the stable codes of 08 §6.4.

use std::borrow::Cow;

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use cavea_engine::{Failure, InvalidReason, Missing, ObjectFailure, ObjectReason, ObjectRef};
use serde_json::{Map, Value, json};

/// A stable error code (08 §6.4). Never rename one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    InvalidRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    AlreadyExists,
    ObjectNotFree,
    ObjectNotHeldByToken,
    ObjectStatusConflict,
    VersionConflict,
    ChartLocked,
    UpgradeConflict,
    IdempotencyMismatch,
    HoldTokenExpired,
    HoldLimitExceeded,
    ChannelNotAllowed,
    OrphanRuleViolated,
    NotEnoughObjects,
    ChartInvalid,
    RateLimited,
    InternalError,
    Overloaded,
    Unavailable,
}

impl Code {
    /// Every code, in table order.
    pub const ALL: [Self; 22] = [
        Self::InvalidRequest,
        Self::Unauthorized,
        Self::Forbidden,
        Self::NotFound,
        Self::AlreadyExists,
        Self::ObjectNotFree,
        Self::ObjectNotHeldByToken,
        Self::ObjectStatusConflict,
        Self::VersionConflict,
        Self::ChartLocked,
        Self::UpgradeConflict,
        Self::IdempotencyMismatch,
        Self::HoldTokenExpired,
        Self::HoldLimitExceeded,
        Self::ChannelNotAllowed,
        Self::OrphanRuleViolated,
        Self::NotEnoughObjects,
        Self::ChartInvalid,
        Self::RateLimited,
        Self::InternalError,
        Self::Overloaded,
        Self::Unavailable,
    ];

    /// `(code, HTTP status, default message)`: the table of 08 §6.4.
    fn row(self) -> (&'static str, u16, &'static str) {
        match self {
            Self::InvalidRequest => ("invalid_request", 400, "The request is malformed."),
            Self::Unauthorized => ("unauthorized", 401, "The API key is missing or invalid."),
            Self::Forbidden => ("forbidden", 403, "The key may not do this."),
            Self::NotFound => ("not_found", 404, "Not found."),
            Self::AlreadyExists => (
                "already_exists",
                409,
                "Something with this key already exists.",
            ),
            Self::ObjectNotFree => ("object_not_free", 409, "At least one object is not free."),
            Self::ObjectNotHeldByToken => (
                "object_not_held_by_token",
                409,
                "At least one object is not held by this hold token.",
            ),
            Self::ObjectStatusConflict => (
                "object_status_conflict",
                409,
                "At least one object's status does not allow this change.",
            ),
            Self::VersionConflict => ("version_conflict", 409, "The draft has changed."),
            Self::ChartLocked => ("chart_locked", 409, "Someone else is editing the chart."),
            Self::UpgradeConflict => (
                "upgrade_conflict",
                409,
                "Objects that are not free are missing from the new chart version.",
            ),
            Self::IdempotencyMismatch => (
                "idempotency_mismatch",
                409,
                "The idempotency key was used with a different request.",
            ),
            Self::HoldTokenExpired => ("hold_token_expired", 410, "The hold token has expired."),
            Self::HoldLimitExceeded => (
                "hold_limit_exceeded",
                422,
                "The hold token's object limit would be exceeded.",
            ),
            Self::ChannelNotAllowed => ("channel_not_allowed", 422, "The channel is not allowed."),
            Self::OrphanRuleViolated => (
                "orphan_rule_violated",
                422,
                "The selection would leave a single seat isolated.",
            ),
            Self::NotEnoughObjects => (
                "not_enough_objects",
                422,
                "Not enough objects are available.",
            ),
            Self::ChartInvalid => ("chart_invalid", 422, "The chart has validation errors."),
            Self::RateLimited => ("rate_limited", 429, "Too many requests."),
            Self::InternalError => ("internal_error", 500, "Internal error."),
            Self::Overloaded => ("overloaded", 503, "The event is busy; retry shortly."),
            Self::Unavailable => ("unavailable", 503, "Storage is unavailable; retry shortly."),
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.row().0
    }

    /// Listed so that clients handle them, but not sent by this version:
    /// their features (upgrading an event, channels, the orphan rule,
    /// best-available) are not built, and an expired hold token is
    /// `not_found` (08 §6.4, D-88).
    #[must_use]
    pub fn reserved(self) -> bool {
        matches!(
            self,
            Self::UpgradeConflict
                | Self::HoldTokenExpired
                | Self::ChannelNotAllowed
                | Self::OrphanRuleViolated
                | Self::NotEnoughObjects
        )
    }

    #[must_use]
    pub fn status(self) -> StatusCode {
        StatusCode::from_u16(self.row().1).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

/// A `details.reason` (08 §6.4): each value the server sends, with the one
/// code it goes with. A handler names a reason only through this type
/// ([`ApiError::from`]), never as text, so the OpenAPI document lists every
/// reason each code can carry (`x-cavea-reasons` on `Problem`, from
/// [`Reason::all`]) and no response can carry one it does not list; the
/// router tests check every problem they receive against that list.
/// Never rename one: clients match on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    // `invalid_request`
    JsonSyntax,
    ContentType,
    JsonData,
    BodyTooLarge,
    BodyUnreadable,
    JsonDepth,
    InvalidField,
    Path,
    Query,
    IdempotencyKey,
    IfMatch,
    Document,
    Archived,
    Unpublished,
    Revoked,
    /// A console session's `/v1` request without `Cavea-Workspace`.
    NoWorkspace,
    LastOwner,
    Upgrade,
    LastEventId,
    AssetType,
    AssetMalformed,
    AssetDimensions,
    /// Confirming or turning off a second factor the user does not have.
    NoSecondFactor,
    /// The engine's own (`InvalidReason`), `too_many_objects` included,
    /// which the server also checks before submitting.
    Engine(InvalidReason),
    // `unauthorized`
    Missing,
    Malformed,
    Invalid,
    SessionToken,
    ConsoleSession,
    /// Sign-in: a wrong email or password, alike.
    Credentials,
    SetupToken,
    Invitation,
    /// Sign-in's second step: the challenge is unknown, used or expired.
    Challenge,
    /// Sign-in's second step: the code is wrong.
    SecondFactor,
    ResetToken,
    // `forbidden`
    KeyKind,
    Origin,
    Session,
    SelfRevocation,
    Csrf,
    /// The session's user is not a member of the workspace named.
    NotAMember,
    Role,
    LicenceExpired,
    InstanceOwner,
    /// A wrong current password, signed in.
    CurrentPassword,
    InvitationEmail,
    /// A wrong code, signed in: confirming or turning off a second factor.
    WrongCode,
    /// Resetting one's own account through the members routes.
    OwnAccount,
    /// The member belongs to a workspace where the caller may not manage them.
    OtherWorkspaces,
    // `already_exists`
    /// Accepting an invitation as new for an email that has an account.
    HasAccount,
    SecondFactorEnabled,
    // `hold_limit_exceeded`
    TokensPerSub,
    // `rate_limited`
    SignInRate,
    Backoff,
    // `overloaded`
    CompileTimeout,
    PasswordHashing,
    AssetMemory,
}

impl Reason {
    /// The server's own reasons, every variant but [`Reason::Engine`];
    /// [`Reason::all`] adds the engine's. A variant left out of this list
    /// is missing from the document, and the router tests fail on the first
    /// response that carries it.
    const OWN: [Self; 56] = [
        Self::JsonSyntax,
        Self::ContentType,
        Self::JsonData,
        Self::BodyTooLarge,
        Self::BodyUnreadable,
        Self::JsonDepth,
        Self::InvalidField,
        Self::Path,
        Self::Query,
        Self::IdempotencyKey,
        Self::IfMatch,
        Self::Document,
        Self::Archived,
        Self::Unpublished,
        Self::Revoked,
        Self::NoWorkspace,
        Self::LastOwner,
        Self::Upgrade,
        Self::LastEventId,
        Self::AssetType,
        Self::AssetMalformed,
        Self::AssetDimensions,
        Self::NoSecondFactor,
        Self::Missing,
        Self::Malformed,
        Self::Invalid,
        Self::SessionToken,
        Self::ConsoleSession,
        Self::Credentials,
        Self::SetupToken,
        Self::Invitation,
        Self::Challenge,
        Self::SecondFactor,
        Self::ResetToken,
        Self::KeyKind,
        Self::Origin,
        Self::Session,
        Self::SelfRevocation,
        Self::Csrf,
        Self::NotAMember,
        Self::Role,
        Self::LicenceExpired,
        Self::InstanceOwner,
        Self::CurrentPassword,
        Self::InvitationEmail,
        Self::WrongCode,
        Self::OwnAccount,
        Self::OtherWorkspaces,
        Self::HasAccount,
        Self::SecondFactorEnabled,
        Self::TokensPerSub,
        Self::SignInRate,
        Self::Backoff,
        Self::CompileTimeout,
        Self::PasswordHashing,
        Self::AssetMemory,
    ];

    /// Every reason, each once.
    pub fn all() -> impl Iterator<Item = Self> {
        Self::OWN
            .into_iter()
            .chain(InvalidReason::ALL.map(Self::Engine))
    }

    /// The code and the text on the wire.
    #[must_use]
    pub fn row(self) -> (Code, &'static str) {
        use Code as C;
        match self {
            Self::JsonSyntax => (C::InvalidRequest, "json_syntax"),
            Self::ContentType => (C::InvalidRequest, "content_type"),
            Self::JsonData => (C::InvalidRequest, "json_data"),
            Self::BodyTooLarge => (C::InvalidRequest, "body_too_large"),
            Self::BodyUnreadable => (C::InvalidRequest, "body_unreadable"),
            Self::JsonDepth => (C::InvalidRequest, "json_depth"),
            Self::InvalidField => (C::InvalidRequest, "invalid_field"),
            Self::Path => (C::InvalidRequest, "path"),
            Self::Query => (C::InvalidRequest, "query"),
            Self::IdempotencyKey => (C::InvalidRequest, "idempotency_key"),
            Self::IfMatch => (C::InvalidRequest, "if_match"),
            Self::Document => (C::InvalidRequest, "document"),
            Self::Archived => (C::InvalidRequest, "archived"),
            Self::Unpublished => (C::InvalidRequest, "unpublished"),
            Self::Revoked => (C::InvalidRequest, "revoked"),
            Self::NoWorkspace => (C::InvalidRequest, "workspace"),
            Self::LastOwner => (C::InvalidRequest, "last_owner"),
            Self::Upgrade => (C::InvalidRequest, "upgrade"),
            Self::LastEventId => (C::InvalidRequest, "last_event_id"),
            Self::AssetType => (C::InvalidRequest, "asset_type"),
            Self::AssetMalformed => (C::InvalidRequest, "asset_malformed"),
            Self::AssetDimensions => (C::InvalidRequest, "asset_dimensions"),
            Self::NoSecondFactor => (C::InvalidRequest, "no_second_factor"),
            Self::Engine(reason) => (C::InvalidRequest, reason.code()),
            Self::Missing => (C::Unauthorized, "missing"),
            Self::Malformed => (C::Unauthorized, "malformed"),
            Self::Invalid => (C::Unauthorized, "invalid"),
            Self::SessionToken => (C::Unauthorized, "session_token"),
            Self::ConsoleSession => (C::Unauthorized, "console_session"),
            Self::Credentials => (C::Unauthorized, "credentials"),
            Self::SetupToken => (C::Unauthorized, "setup_token"),
            Self::Invitation => (C::Unauthorized, "invitation"),
            Self::Challenge => (C::Unauthorized, "challenge"),
            Self::SecondFactor => (C::Unauthorized, "second_factor"),
            Self::ResetToken => (C::Unauthorized, "reset_token"),
            Self::KeyKind => (C::Forbidden, "key_kind"),
            Self::Origin => (C::Forbidden, "origin"),
            Self::Session => (C::Forbidden, "session"),
            Self::SelfRevocation => (C::Forbidden, "self_revocation"),
            Self::Csrf => (C::Forbidden, "csrf"),
            Self::NotAMember => (C::Forbidden, "workspace"),
            Self::Role => (C::Forbidden, "role"),
            Self::LicenceExpired => (C::Forbidden, "licence_expired"),
            Self::InstanceOwner => (C::Forbidden, "instance_owner"),
            Self::CurrentPassword => (C::Forbidden, "credentials"),
            Self::InvitationEmail => (C::Forbidden, "invitation_email"),
            Self::WrongCode => (C::Forbidden, "second_factor"),
            Self::OwnAccount => (C::Forbidden, "own_account"),
            Self::OtherWorkspaces => (C::Forbidden, "other_workspaces"),
            Self::HasAccount => (C::AlreadyExists, "sign_in"),
            Self::SecondFactorEnabled => (C::AlreadyExists, "second_factor"),
            Self::TokensPerSub => (C::HoldLimitExceeded, "tokens_per_sub"),
            Self::SignInRate => (C::RateLimited, "sign_in"),
            Self::Backoff => (C::RateLimited, "backoff"),
            Self::CompileTimeout => (C::Overloaded, "compile_timeout"),
            Self::PasswordHashing => (C::Overloaded, "password_hashing"),
            Self::AssetMemory => (C::Overloaded, "asset_memory"),
        }
    }

    #[must_use]
    pub fn code(self) -> Code {
        self.row().0
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.row().1
    }

    /// Per code, its reasons, sorted and each once: the document's
    /// `x-cavea-reasons`.
    #[must_use]
    pub fn by_code() -> Map<String, Value> {
        let mut by_code: std::collections::BTreeMap<&str, std::collections::BTreeSet<&str>> =
            std::collections::BTreeMap::new();
        for reason in Self::all() {
            by_code
                .entry(reason.code().as_str())
                .or_default()
                .insert(reason.as_str());
        }
        by_code
            .into_iter()
            .map(|(code, reasons)| (code.to_owned(), reasons.into_iter().collect()))
            .collect()
    }

    /// Per code, the reasons of `details.objects[].reason` (the engine's
    /// `ObjectReason`): the document's `x-cavea-object-reasons`.
    #[must_use]
    pub fn objects_by_code() -> Map<String, Value> {
        let mut by_code: Map<String, Value> = Map::new();
        for reason in ObjectReason::ALL {
            let code = ApiError::from(reason.failure(Vec::new())).code().as_str();
            let entry = by_code.entry(code).or_insert_with(|| json!([]));
            if let Some(list) = entry.as_array_mut() {
                list.push(reason.code().into());
            }
        }
        by_code
    }
}

/// Seconds a caller should wait after `503 overloaded`.
const OVERLOADED_RETRY_AFTER: &str = "1";

/// An error response. Renders as
/// `{"code", "status", "message", "details", "request_id"}` (08 §6.4).
#[derive(Debug)]
pub struct ApiError {
    code: Code,
    message: Cow<'static, str>,
    details: Map<String, Value>,
    /// Seconds for `Retry-After`, beyond `overloaded`'s default.
    retry_after: Option<u64>,
    /// Logged, never sent: the cause of an internal error.
    cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl ApiError {
    /// An error with the code's default message and empty details.
    #[must_use]
    pub fn new(code: Code) -> Self {
        Self {
            code,
            message: Cow::Borrowed(code.row().2),
            details: Map::new(),
            retry_after: None,
            cause: None,
        }
    }

    /// A `500 internal_error` with a generic body. `cause` is logged with the
    /// request id when the response is rendered; it must not contain secrets
    /// or request bodies.
    #[must_use]
    pub fn internal(cause: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self {
            cause: Some(cause.into()),
            ..Self::new(Code::InternalError)
        }
    }

    #[must_use]
    pub fn with_message(mut self, message: impl Into<Cow<'static, str>>) -> Self {
        self.message = message.into();
        self
    }

    /// Add one field to `details`; never `reason`, which only a [`Reason`]
    /// sets.
    #[must_use]
    pub fn with_detail(mut self, key: &str, value: impl Into<Value>) -> Self {
        debug_assert_ne!(key, "reason", "a reason is set only by `Reason`");
        self.details.insert(key.to_owned(), value.into());
        self
    }

    /// Send `Retry-After: <seconds>`.
    #[must_use]
    pub fn with_retry_after(mut self, seconds: u64) -> Self {
        self.retry_after = Some(seconds);
        self
    }

    #[must_use]
    pub fn code(&self) -> Code {
        self.code
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match &self.cause {
            Some(cause) if self.code == Code::InternalError => {
                tracing::error!(code = self.code.as_str(), error = %cause, "internal error");
            }
            Some(cause) => {
                tracing::warn!(code = self.code.as_str(), error = %cause, "request failed");
            }
            None => {}
        }
        let body = json!({
            "code": self.code.as_str(),
            "status": self.code.status().as_u16(),
            "message": self.message,
            "details": self.details,
            "request_id": cavea_telemetry::request_id().map(|id| id.to_string()),
        });
        let mut response = (self.code.status(), body.to_string()).into_response();
        let headers = response.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        if let Some(seconds) = self.retry_after {
            headers.insert(header::RETRY_AFTER, HeaderValue::from(seconds));
        } else if self.code == Code::Overloaded {
            headers.insert(
                header::RETRY_AFTER,
                HeaderValue::from_static(OVERLOADED_RETRY_AFTER),
            );
        }
        response
    }
}

/// The reason's code with its default message, and `details.reason`.
impl From<Reason> for ApiError {
    fn from(reason: Reason) -> Self {
        let mut error = Self::new(reason.code());
        error
            .details
            .insert("reason".to_owned(), reason.as_str().into());
        error
    }
}

impl From<Failure> for ApiError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::InvalidRequest(reason) => Self::from(Reason::Engine(reason)),
            Failure::Forbidden => Self::new(Code::Forbidden),
            Failure::NotFound(Missing::HoldToken) => {
                Self::new(Code::NotFound).with_detail("missing", "hold_token")
            }
            Failure::NotFound(Missing::Objects(objects)) => Self::new(Code::NotFound)
                .with_detail("missing", "objects")
                .with_detail(
                    "objects",
                    objects.iter().map(object_ref).collect::<Vec<_>>(),
                ),
            Failure::ObjectNotFree(objects) => {
                Self::new(Code::ObjectNotFree).with_detail("objects", object_failures(&objects))
            }
            Failure::ObjectNotHeldByToken(objects) => Self::new(Code::ObjectNotHeldByToken)
                .with_detail("objects", object_failures(&objects)),
            Failure::ObjectStatusConflict(objects) => Self::new(Code::ObjectStatusConflict)
                .with_detail("objects", object_failures(&objects)),
            Failure::IdempotencyMismatch => Self::new(Code::IdempotencyMismatch),
            Failure::HoldTokenExpired => Self::new(Code::HoldTokenExpired),
            Failure::HoldLimitExceeded { limit, requested } => Self::new(Code::HoldLimitExceeded)
                .with_detail("limit", limit)
                .with_detail("requested", requested),
            Failure::Overloaded => Self::new(Code::Overloaded),
            Failure::Unavailable => Self::new(Code::Unavailable),
        }
    }
}

/// A database error: `503 unavailable` when PostgreSQL cannot be reached, is
/// restarting or shutting down, or the pool is exhausted (08 §12, §16), else
/// `500`. A handler that expects a particular error, such as a unique
/// violation, checks for it before using `?`.
impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        if unavailable(&error) {
            Self {
                cause: Some(error.into()),
                ..Self::new(Code::Unavailable)
            }
        } else {
            Self::internal(error)
        }
    }
}

/// The database is down or going down, not wrong: the connection broke or
/// could not be made (a handshake that is not PostgreSQL's, such as a port
/// proxy's while the database restarts, is a protocol error), or PostgreSQL
/// refused or ended it while starting, stopping or crashing.
fn unavailable(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::PoolTimedOut
        | sqlx::Error::PoolClosed
        | sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::Protocol(_)
        | sqlx::Error::WorkerCrashed => true,
        // Class 08 connection exceptions; admin_shutdown, crash_shutdown,
        // cannot_connect_now; too_many_connections.
        sqlx::Error::Database(e) => e.code().is_some_and(|c| {
            c.starts_with("08") || matches!(&*c, "57P01" | "57P02" | "57P03" | "53300")
        }),
        _ => false,
    }
}

fn object_ref(object: &ObjectRef) -> Value {
    match object {
        ObjectRef::Label(label) => json!({ "label": &**label }),
        ObjectRef::Idx(idx) => json!({ "idx": idx }),
    }
}

fn object_failures(objects: &[ObjectFailure]) -> Value {
    objects
        .iter()
        .map(|o| json!({ "idx": o.idx, "reason": o.reason.code() }))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    #[derive(Debug)]
    struct Db(&'static str);

    impl std::fmt::Display for Db {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }

    impl std::error::Error for Db {}

    impl sqlx::error::DatabaseError for Db {
        fn message(&self) -> &str {
            self.0
        }
        fn code(&self) -> Option<Cow<'_, str>> {
            Some(Cow::Borrowed(self.0))
        }
        fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
            self
        }
        fn kind(&self) -> sqlx::error::ErrorKind {
            sqlx::error::ErrorKind::Other
        }
    }

    /// Each reason once per code, in the form clients match on, and the
    /// engine's list complete (the match below has no wildcard, so a new
    /// engine reason fails to compile until it is placed).
    #[test]
    fn reasons_are_declared_once() {
        let mut seen = std::collections::BTreeSet::new();
        for reason in Reason::all() {
            let row = (reason.code().as_str(), reason.as_str());
            assert!(seen.insert(row), "{reason:?} is listed twice");
            let text = reason.as_str();
            assert!(
                text.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{text}"
            );
        }
        let position = |r: InvalidReason| match r {
            InvalidReason::NoObjects => 0,
            InvalidReason::TooManyObjects => 1,
            InvalidReason::TtlOutOfRange => 2,
            InvalidReason::ZeroObjectLimit => 3,
            InvalidReason::TokenAlreadyExists => 4,
            InvalidReason::OrderRefLength => 5,
            InvalidReason::TicketTypeLength => 6,
            InvalidReason::ExtraTooLarge => 7,
            InvalidReason::DetailMismatch => 8,
            InvalidReason::BadStatus => 9,
            InvalidReason::IdempotencyKeyLength => 10,
        };
        for (i, reason) in InvalidReason::ALL.into_iter().enumerate() {
            assert_eq!(position(reason), i);
        }
        let position = |r: ObjectReason| match r {
            ObjectReason::NotFree => 0,
            ObjectReason::NotHeldByToken => 1,
            ObjectReason::NotBooked => 2,
            ObjectReason::NotBlocked => 3,
        };
        for (i, reason) in ObjectReason::ALL.into_iter().enumerate() {
            assert_eq!(position(reason), i);
        }
    }

    /// Seen during a PostgreSQL restart under load (13 §7.3): a port proxy's
    /// answer to the TLS request, and PostgreSQL ending connections as it
    /// stops, were answered `500`, which clients do not retry.
    #[test]
    fn a_database_going_down_is_unavailable() {
        let code = |e: sqlx::Error| ApiError::from(e).code();
        for e in [
            sqlx::Error::Protocol("unexpected response from SSLRequest: 0x00".into()),
            sqlx::Error::Io(std::io::ErrorKind::ConnectionReset.into()),
            sqlx::Error::PoolTimedOut,
            sqlx::Error::WorkerCrashed,
            sqlx::Error::Database(Box::new(Db("57P01"))),
            sqlx::Error::Database(Box::new(Db("57P03"))),
            sqlx::Error::Database(Box::new(Db("08006"))),
        ] {
            assert_eq!(code(e), Code::Unavailable);
        }
        for e in [
            sqlx::Error::RowNotFound,
            sqlx::Error::Database(Box::new(Db("23505"))),
            sqlx::Error::ColumnNotFound("x".into()),
        ] {
            assert_eq!(code(e), Code::InternalError);
        }
    }
}
