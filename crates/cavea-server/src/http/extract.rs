//! Extractors whose rejections are problem+json, and the request limits of
//! 12 §3 T19.

use axum::body::Bytes;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use serde::de::DeserializeOwned;

use super::{ApiError, Code};

/// The body limit of every route unless it sets its own: room for a command
/// naming [`MAX_OBJECTS_PER_COMMAND`] labels with a ticket type each. A route
/// that needs more, such as chart upload, adds
/// `.layer(axum::extract::DefaultBodyLimit::max(n))` to its method router.
pub const DEFAULT_BODY_LIMIT: usize = 256 * 1024;

/// The deepest nesting of arrays and objects a JSON body may have.
pub const MAX_JSON_DEPTH: usize = 32;

/// The most objects one command may name (02 §14). The engine refuses more
/// with `invalid_request` and `too_many_objects`; [`check_object_count`] gives
/// the same answer before a command is submitted.
pub use cavea_engine::limits::MAX_OBJECTS_PER_COMMAND;

/// Refuse a request naming more than [`MAX_OBJECTS_PER_COMMAND`] objects, as the
/// engine would.
pub fn check_object_count(count: usize) -> Result<(), ApiError> {
    if count > MAX_OBJECTS_PER_COMMAND {
        return Err(ApiError::new(Code::InvalidRequest)
            .with_message("Too many objects in one request.")
            .with_detail("reason", "too_many_objects")
            .with_detail("limit", MAX_OBJECTS_PER_COMMAND));
    }
    Ok(())
}

/// A JSON request body. Use it instead of `axum::Json` so that a malformed,
/// mistyped, oversized or too deeply nested body is a problem+json
/// `invalid_request`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Json<T>(pub T);

impl<T, S> FromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, ApiError> {
        let RawJson(bytes) = RawJson::from_request(request, state).await?;
        match axum::Json::<T>::from_bytes(&bytes) {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(json_rejection(&rejection)),
        }
    }
}

/// A JSON request body as bytes, not yet parsed: the checks of [`Json`]
/// (content type, the route's size limit, nesting depth) without the parse.
#[derive(Debug, Clone, Default)]
pub struct RawJson(pub Bytes);

impl<S> FromRequest<S> for RawJson
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, ApiError> {
        if !json_content_type(&request) {
            return Err(invalid(
                "content_type",
                "Expected `Content-Type: application/json`.",
            ));
        }
        // Reads at most the route's body limit.
        let bytes = Bytes::from_request(request, state).await.map_err(|r| {
            if r.status() == StatusCode::PAYLOAD_TOO_LARGE {
                invalid("body_too_large", "The body is over the size limit.")
            } else {
                invalid("body_unreadable", "The body could not be read.")
            }
        })?;
        if too_deep(&bytes) {
            return Err(invalid(
                "json_depth",
                "The body nests arrays and objects too deeply.",
            )
            .with_detail("limit", MAX_JSON_DEPTH));
        }
        Ok(Self(bytes))
    }
}

pub(super) fn invalid(reason: &'static str, message: &'static str) -> ApiError {
    ApiError::new(Code::InvalidRequest)
        .with_message(message)
        .with_detail("reason", reason)
}

/// `application/json` or `application/*+json`, with any parameters.
fn json_content_type(request: &Request) -> bool {
    let Some(value) = request.headers().get(header::CONTENT_TYPE) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    let essence = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    essence
        .strip_prefix("application/")
        .is_some_and(|sub| sub == "json" || sub.ends_with("+json"))
}

/// Whether arrays and objects nest deeper than [`MAX_JSON_DEPTH`]: one pass,
/// no allocation, before anything is parsed. Brackets inside strings do not
/// count; anything else malformed is left to the parser.
fn too_deep(bytes: &[u8]) -> bool {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for &b in bytes {
        if in_string {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                if depth > MAX_JSON_DEPTH {
                    return true;
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

fn json_rejection(rejection: &JsonRejection) -> ApiError {
    let reason = match rejection {
        JsonRejection::JsonSyntaxError(_) => "json_syntax",
        JsonRejection::JsonDataError(_) => "json_data",
        _ => "body_unreadable",
    };
    // axum's text may quote the body: it goes back to the caller, never to the log.
    ApiError::new(Code::InvalidRequest)
        .with_message(rejection.body_text())
        .with_detail("reason", reason)
}

/// Path parameters. Use it instead of `axum::extract::Path` so that a
/// parameter of the wrong type is a problem+json `invalid_request` with
/// `reason: "path"`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Path<T>(pub T);

impl<T, S> FromRequestParts<S> for Path<T>
where
    axum::extract::Path<T>: FromRequestParts<S, Rejection = PathRejection>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        // No identifier holds U+0000, which PostgreSQL text cannot hold
        // either: one in the path names nothing (08 §6.4).
        if parts.uri.path().contains("%00") {
            return Err(ApiError::new(Code::NotFound));
        }
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::new(Code::InvalidRequest)
                .with_message(rejection.body_text())
                .with_detail("reason", "path")),
        }
    }
}

/// The query string. Use it instead of `axum::extract::Query` so that a
/// malformed or mistyped query is a problem+json `invalid_request` with
/// `reason: "query"`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Query<T>(pub T);

impl<T, S> FromRequestParts<S> for Query<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        // U+0000 is in no key, label or cursor, and PostgreSQL text cannot
        // hold it.
        if parts.uri.query().is_some_and(|q| q.contains("%00")) {
            return Err(ApiError::new(Code::InvalidRequest)
                .with_message("The query holds U+0000.")
                .with_detail("reason", "query"));
        }
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(Self(value)),
            Err(rejection) => Err(query_rejection(&rejection)),
        }
    }
}

fn query_rejection(rejection: &QueryRejection) -> ApiError {
    ApiError::new(Code::InvalidRequest)
        .with_message(rejection.body_text())
        .with_detail("reason", "query")
}

/// A secret in a request or response body: a password, a setup, invitation
/// or hold token, a new key, a link holding a token. Its `Debug` shows
/// nothing, so a body logged with `?body` cannot carry one (12 §4, SEC-009);
/// on the wire it is the plain string.
#[derive(Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::ops::Deref for Secret {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// `?limit=&after=` of a list endpoint (08 §6.1).
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    limit: Option<u32>,
    /// The `next` of the previous page: the id of its last item.
    pub after: Option<String>,
}

impl Page {
    pub const DEFAULT_LIMIT: u32 = 100;
    pub const MAX_LIMIT: u32 = 100;

    /// A page from a list's own query, which has more parameters.
    #[must_use]
    pub fn new(limit: Option<u32>, after: Option<String>) -> Self {
        Self { limit, after }
    }

    /// The page size, 1 to [`Self::MAX_LIMIT`].
    pub fn limit(&self) -> Result<u32, ApiError> {
        match self.limit.unwrap_or(Self::DEFAULT_LIMIT) {
            n @ 1..=Self::MAX_LIMIT => Ok(n),
            _ => Err(ApiError::new(Code::InvalidRequest)
                .with_message("`limit` must be 1 to 100.")
                .with_detail("reason", "query")),
        }
    }
}

/// The longest name or customer key, in characters (02 §3).
pub const MAX_TEXT_CHARS: usize = 128;

/// Check a required text field such as a name or key: 1 to
/// [`MAX_TEXT_CHARS`] characters, no U+0000 (which PostgreSQL text cannot
/// hold). Refused as `invalid_request` with `reason: "invalid_field"` and the
/// field's name.
pub fn check_text(field: &'static str, value: &str) -> Result<(), ApiError> {
    if value.is_empty() || value.chars().count() > MAX_TEXT_CHARS || value.contains('\0') {
        return Err(invalid_field(
            field,
            "must be 1 to 128 characters, without U+0000",
        ));
    }
    Ok(())
}

/// `invalid_request` for one field of the body.
pub fn invalid_field(field: &'static str, problem: &'static str) -> ApiError {
    ApiError::new(Code::InvalidRequest)
        .with_message(format!("`{field}` {problem}."))
        .with_detail("reason", "invalid_field")
        .with_detail("field", field)
}
