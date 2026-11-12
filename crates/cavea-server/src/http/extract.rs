//! Extractors whose rejections are problem+json, and the request limits of
//! 12 §3 T19.

use axum::body::Bytes;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use serde::de::DeserializeOwned;

use super::Reason;
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
        return Err(
            ApiError::from(Reason::Engine(cavea_engine::InvalidReason::TooManyObjects))
                .with_message("Too many objects in one request.")
                .with_detail("limit", MAX_OBJECTS_PER_COMMAND),
        );
    }
    Ok(())
}

/// A JSON request body, which must be an object. Use it instead of
/// `axum::Json` so that a malformed, mistyped, oversized or too deeply nested
/// body is a problem+json `invalid_request`. A struct nested in the body
/// takes [`object`] or [`optional_object`].
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
        // Every JSON body of the API is an object (08 §6.4). serde's derived
        // `Deserialize` also takes a struct from an array, its fields by
        // position, so anything else is refused before `T` is parsed: valid
        // JSON as `json_data`, the rest as `json_syntax`.
        if bytes.iter().find(|b| !b.is_ascii_whitespace()) != Some(&b'{') {
            return Err(
                if serde_json::from_slice::<serde::de::IgnoredAny>(&bytes).is_ok() {
                    invalid(Reason::JsonData, "The body must be a JSON object.")
                } else {
                    invalid(Reason::JsonSyntax, "The body is not JSON.")
                },
            );
        }
        match axum::Json::<T>::from_bytes(&bytes) {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(json_rejection(&rejection)),
        }
    }
}

/// A JSON request body as bytes, not yet parsed: the checks of [`Json`]
/// (content type, the route's size limit, nesting depth) without the parse.
/// Whatever parses it checks that it is an object.
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
                Reason::ContentType,
                "Expected `Content-Type: application/json`.",
            ));
        }
        // Reads at most the route's body limit.
        let bytes = Bytes::from_request(request, state).await.map_err(|r| {
            if r.status() == StatusCode::PAYLOAD_TOO_LARGE {
                invalid(Reason::BodyTooLarge, "The body is over the size limit.")
            } else {
                invalid(Reason::BodyUnreadable, "The body could not be read.")
            }
        })?;
        if too_deep(&bytes) {
            return Err(invalid(
                Reason::JsonDepth,
                "The body nests arrays and objects too deeply.",
            )
            .with_detail("limit", MAX_JSON_DEPTH));
        }
        Ok(Self(bytes))
    }
}

/// `invalid_request` with `reason` and a message.
pub(super) fn invalid(reason: Reason, message: &'static str) -> ApiError {
    ApiError::from(reason).with_message(message)
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
        JsonRejection::JsonSyntaxError(_) => Reason::JsonSyntax,
        JsonRejection::JsonDataError(_) => Reason::JsonData,
        _ => Reason::BodyUnreadable,
    };
    // axum's text may quote the body: it goes back to the caller, never to the log.
    ApiError::from(reason).with_message(rejection.body_text())
}

/// A struct nested in a request body, as a field's
/// `#[serde(deserialize_with = "object")]`: it must be a JSON object, where
/// serde's derived `Deserialize` would also take an array, its fields by
/// position, which the document does not allow (08 §6.4).
pub fn object<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    struct Fields<T>(std::marker::PhantomData<T>);

    impl<'de, T: serde::Deserialize<'de>> serde::de::Visitor<'de> for Fields<T> {
        type Value = T;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("an object")
        }

        fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<T, A::Error> {
            T::deserialize(serde::de::value::MapAccessDeserializer::new(map))
        }
    }

    deserializer.deserialize_map(Fields(std::marker::PhantomData))
}

/// [`object`] for an optional field: `null` is `None`.
pub fn optional_object<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    struct Object<T>(T);

    impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for Object<T> {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            object(deserializer).map(Self)
        }
    }

    let value: Option<Object<T>> = serde::Deserialize::deserialize(deserializer)?;
    Ok(value.map(|o| o.0))
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
            Err(rejection) => Err(ApiError::from(Reason::Path).with_message(rejection.body_text())),
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
            return Err(ApiError::from(Reason::Query).with_message("The query holds U+0000."));
        }
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(Self(value)),
            Err(rejection) => Err(query_rejection(&rejection)),
        }
    }
}

fn query_rejection(rejection: &QueryRejection) -> ApiError {
    ApiError::from(Reason::Query).with_message(rejection.body_text())
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
            _ => Err(ApiError::from(Reason::Query).with_message("`limit` must be 1 to 100.")),
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
    ApiError::from(Reason::InvalidField)
        .with_message(format!("`{field}` {problem}."))
        .with_detail("field", field)
}
