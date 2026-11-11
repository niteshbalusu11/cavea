//! `application/problem+json` errors with the stable codes of 08 §6.4.

use std::borrow::Cow;

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use cavea_engine::{Failure, Missing, ObjectFailure, ObjectRef};
use serde_json::{Map, Value, json};

/// A stable error code (08 §6.4). Never rename one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    InvalidRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    ObjectNotFree,
    ObjectNotHeldByToken,
    ObjectStatusConflict,
    VersionConflict,
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
    pub const ALL: [Self; 20] = [
        Self::InvalidRequest,
        Self::Unauthorized,
        Self::Forbidden,
        Self::NotFound,
        Self::ObjectNotFree,
        Self::ObjectNotHeldByToken,
        Self::ObjectStatusConflict,
        Self::VersionConflict,
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

    #[must_use]
    pub fn status(self) -> StatusCode {
        StatusCode::from_u16(self.row().1).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
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

    /// Add one field to `details`.
    #[must_use]
    pub fn with_detail(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.details.insert(key.to_owned(), value.into());
        self
    }

    #[must_use]
    pub fn code(&self) -> Code {
        self.code
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if let Some(cause) = &self.cause {
            tracing::error!(code = self.code.as_str(), error = %cause, "internal error");
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
        if self.code == Code::Overloaded {
            headers.insert(
                header::RETRY_AFTER,
                HeaderValue::from_static(OVERLOADED_RETRY_AFTER),
            );
        }
        response
    }
}

impl From<Failure> for ApiError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::InvalidRequest(reason) => {
                Self::new(Code::InvalidRequest).with_detail("reason", reason.code())
            }
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

/// A JSON request body. Use it instead of `axum::Json` so that a malformed,
/// mistyped or oversized body is a problem+json `invalid_request`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Json<T>(pub T);

impl<T, S> FromRequest<S> for Json<T>
where
    axum::Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, ApiError> {
        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(json_rejection(&rejection)),
        }
    }
}

fn json_rejection(rejection: &JsonRejection) -> ApiError {
    let reason = match rejection {
        JsonRejection::MissingJsonContentType(_) => "content_type",
        JsonRejection::JsonSyntaxError(_) => "json_syntax",
        JsonRejection::JsonDataError(_) => "json_data",
        _ if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => "body_too_large",
        _ => "body_unreadable",
    };
    // axum's text may quote the body: it goes back to the caller, never to the log.
    ApiError::new(Code::InvalidRequest)
        .with_message(rejection.body_text())
        .with_detail("reason", reason)
}
