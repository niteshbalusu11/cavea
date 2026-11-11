//! The route table and the OpenAPI 3.1 document built from it (08 §6.1,
//! `DECISIONS.md` D-46).
//!
//! Every route is registered through [`Routes::add`] with an [`Op`]: the
//! router takes the method and path from it, and [`document`] describes
//! exactly the operations registered, so a route cannot exist without its
//! entry or the reverse. The component schemas (`schemas.rs`) are written by
//! hand; the router tests validate every response they receive, and every
//! request body the server accepted, against them (`check.rs`), so a field
//! added, removed or retyped in a handler fails the tests until the document
//! says so. Every error code of 08 §6.4 comes from [`Code::ALL`].
//!
//! The document is committed as `crates/cavea-server/openapi.json`, which a
//! test keeps current, and served at `GET /v1/openapi.json`.

use std::sync::OnceLock;

use axum::Router;
use axum::body::Bytes;
use axum::handler::Handler;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodFilter, MethodRouter, on};
use serde_json::{Map, Value, json};

use super::{AppState, Code};

#[cfg(test)]
pub(crate) mod check;
mod schemas;

/// The credential an operation takes (08 §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Needs {
    /// None: a public file named by its URL, the health probes, this document.
    Nothing,
    Admin,
    Secret,
    /// A console session alone: the `/console/api` routes of a signed-in user.
    Console,
    Publishable,
    /// A publishable key in the `key` query parameter (D-31).
    QueryKey,
}

/// One operation: a method on a path, and what the document says of it.
#[derive(Debug, Clone)]
pub struct Op {
    pub method: Method,
    /// axum's syntax, which is OpenAPI's: `/v1/charts/{chart}`.
    pub path: &'static str,
    pub id: &'static str,
    summary: &'static str,
    description: Option<&'static str>,
    pub needs: Needs,
    /// Takes an optional `Cavea-Session` (08 §11).
    session: bool,
    /// Takes the hold token in `Cavea-Hold-Token`.
    hold_token: bool,
    /// Answers without the database, whatever the request.
    no_storage: bool,
    idempotent: bool,
    /// `If-Match`, and whether it is required.
    if_match: Option<bool>,
    params: Vec<Value>,
    body: Option<Value>,
    responses: Vec<(u16, Value)>,
    errors: Vec<u16>,
}

impl Op {
    fn new(method: Method, path: &'static str, id: &'static str, summary: &'static str) -> Self {
        Self {
            method,
            path,
            id,
            summary,
            description: None,
            needs: Needs::Nothing,
            session: false,
            hold_token: false,
            no_storage: false,
            idempotent: false,
            if_match: None,
            params: Vec::new(),
            body: None,
            responses: Vec::new(),
            errors: Vec::new(),
        }
    }

    pub(super) fn get(path: &'static str, id: &'static str, summary: &'static str) -> Self {
        Self::new(Method::GET, path, id, summary)
    }

    pub(super) fn post(path: &'static str, id: &'static str, summary: &'static str) -> Self {
        Self::new(Method::POST, path, id, summary)
    }

    pub(super) fn put(path: &'static str, id: &'static str, summary: &'static str) -> Self {
        Self::new(Method::PUT, path, id, summary)
    }

    pub(super) fn patch(path: &'static str, id: &'static str, summary: &'static str) -> Self {
        Self::new(Method::PATCH, path, id, summary)
    }

    pub(super) fn delete(path: &'static str, id: &'static str, summary: &'static str) -> Self {
        Self::new(Method::DELETE, path, id, summary)
    }

    pub(super) fn about(mut self, description: &'static str) -> Self {
        self.description = Some(description);
        self
    }

    pub(super) fn needs(mut self, needs: Needs) -> Self {
        self.needs = needs;
        self
    }

    pub(super) fn session(mut self) -> Self {
        self.session = true;
        self
    }

    pub(super) fn hold_token(mut self) -> Self {
        self.hold_token = true;
        self
    }

    /// Never reaches the database, so never answers `503`.
    pub(super) fn no_storage(mut self) -> Self {
        self.no_storage = true;
        self
    }

    pub(super) fn idempotent(mut self) -> Self {
        self.idempotent = true;
        self
    }

    pub(super) fn if_match(mut self, required: bool) -> Self {
        self.if_match = Some(required);
        self
    }

    /// `?limit=&after=` (08 §6.1).
    pub(super) fn page(mut self) -> Self {
        self.params.push(component("parameters", "Limit"));
        self.params.push(component("parameters", "After"));
        self
    }

    /// A query parameter; an array is repeated (`?label=A&label=B`).
    pub(super) fn query(mut self, name: &str, schema: Value, description: &str) -> Self {
        let mut param = json!({
            "name": name, "in": "query", "required": false,
            "description": description, "schema": schema,
        });
        if param["schema"]["type"] == "array" {
            param["style"] = "form".into();
            param["explode"] = true.into();
        }
        self.params.push(param);
        self
    }

    /// A JSON body of the named component schema.
    pub(super) fn body(mut self, schema: &str) -> Self {
        self.body = Some(json!({
            "required": true,
            "content": { "application/json": { "schema": schema_ref(schema) } },
        }));
        self
    }

    /// A body of another form: a request body object.
    pub(super) fn raw_body(mut self, body: Value) -> Self {
        self.body = Some(body);
        self
    }

    /// A JSON response of the named component schema.
    pub(super) fn reply(self, status: u16, schema: &str, description: &str) -> Self {
        let content = json!({ "application/json": { "schema": schema_ref(schema) } });
        self.response(status, description, Some(content))
    }

    /// A response with the given content (media types), or none.
    pub(super) fn response(
        mut self,
        status: u16,
        description: &str,
        content: Option<Value>,
    ) -> Self {
        let mut response = json!({ "description": description });
        if let Some(content) = content {
            response["content"] = content;
        }
        self.responses.push((status, response));
        self
    }

    /// The last response added carries `ETag`.
    pub(super) fn etag(mut self) -> Self {
        if let Some((_, response)) = self.responses.last_mut() {
            response["headers"] = json!({ "ETag": component("headers", "ETag") });
        }
        self
    }

    /// The last response added carries `Set-Cookie` (the console session).
    pub(super) fn sets_cookie(mut self) -> Self {
        if let Some((_, response)) = self.responses.last_mut() {
            response["headers"] = json!({ "Set-Cookie": component("headers", "SetCookie") });
        }
        self
    }

    /// Error statuses beyond those [`Op::error_statuses`] derives.
    pub(super) fn errors(mut self, statuses: &[u16]) -> Self {
        self.errors.extend_from_slice(statuses);
        self
    }

    /// Whether the operation takes a `Cavea-Session`.
    #[must_use]
    pub fn takes_session(&self) -> bool {
        self.session
    }

    fn path_params(&self) -> impl Iterator<Item = &'static str> {
        self.path
            .split('/')
            .filter_map(|s| s.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
    }

    /// The problem+json statuses: 500 always; 503 unless the operation never
    /// reaches the database (a credential is looked up there, 08 §12); 401
    /// and 403 with a credential; 400 with any input; 404 with a path
    /// parameter; 409 with `Idempotency-Key` (`idempotency_mismatch`); 429
    /// for publishable keys (08 §12); and what the operation adds.
    fn error_statuses(&self) -> Vec<u16> {
        let mut statuses = vec![500];
        if !self.no_storage {
            statuses.push(503);
        }
        if self.idempotent {
            statuses.push(409);
        }
        if self.needs != Needs::Nothing {
            statuses.extend([401, 403]);
        }
        let path_params = self.path_params().next().is_some();
        if path_params || self.body.is_some() || !self.params.is_empty() || self.idempotent {
            statuses.push(400);
        }
        if path_params {
            statuses.push(404);
        }
        if matches!(self.needs, Needs::Publishable | Needs::QueryKey) {
            statuses.push(429);
        }
        statuses.extend(&self.errors);
        statuses.sort_unstable();
        statuses.dedup();
        statuses
    }

    fn security(&self) -> Value {
        let scheme = match self.needs {
            Needs::Nothing => return json!([]),
            Needs::Admin => "adminKey",
            Needs::Secret => "secretKey",
            Needs::Console => return json!([{ "consoleSession": [] }]),
            Needs::Publishable => "publishableKey",
            Needs::QueryKey => "publishableKeyQuery",
        };
        if self.hold_token {
            json!([{ scheme: [], "holdToken": [] }])
        } else if self.session {
            json!([{ scheme: [], "sessionToken": [] }, { scheme: [] }])
        } else {
            json!([{ scheme: [] }])
        }
    }

    fn to_json(&self) -> Value {
        let mut params: Vec<Value> = self
            .path_params()
            .map(|name| component("parameters", &path_param_name(name)))
            .collect();
        let safe = matches!(self.method, Method::GET | Method::HEAD);
        if self.needs == Needs::Console && !safe {
            params.push(component("parameters", "CsrfToken"));
        }
        if self.idempotent {
            params.push(component("parameters", "IdempotencyKey"));
        }
        match self.if_match {
            Some(true) => params.push(component("parameters", "IfMatchRequired")),
            Some(false) => params.push(component("parameters", "IfMatch")),
            None => {}
        }
        params.extend(self.params.iter().cloned());
        let mut responses = Map::new();
        for (status, response) in &self.responses {
            let mut response = response.clone();
            let headers = response["headers"].as_object().cloned().unwrap_or_default();
            let mut all = Map::new();
            all.insert("Cavea-Request-Id".into(), component("headers", "RequestId"));
            all.extend(headers);
            response["headers"] = Value::Object(all);
            responses.insert(status.to_string(), response);
        }
        for status in self.error_statuses() {
            if let Some(name) = problem_response(status) {
                responses
                    .entry(status.to_string())
                    .or_insert_with(|| component("responses", name));
            }
        }
        let mut op = json!({
            "operationId": self.id,
            "summary": self.summary,
            "tags": [tag(self.path)],
            "security": self.security(),
            "responses": responses,
        });
        if let Some(description) = self.description {
            op["description"] = description.into();
        }
        if !params.is_empty() {
            op["parameters"] = params.into();
        }
        if let Some(body) = &self.body {
            op["requestBody"] = body.clone();
        }
        op
    }
}

/// The router and the operations registered on it.
pub(super) struct Routes {
    router: Router<AppState>,
    ops: Vec<Op>,
}

impl Routes {
    pub(super) fn new() -> Self {
        Self {
            router: Router::new(),
            ops: Vec::new(),
        }
    }

    /// Register `handler` for `op`'s method and path.
    pub(super) fn add<H, T>(self, op: Op, handler: H) -> Self
    where
        H: Handler<T, AppState>,
        T: 'static,
    {
        self.layered(op, handler, |route| route)
    }

    /// [`Routes::add`], with `layer` applied to the route, such as a body
    /// limit.
    pub(super) fn layered<H, T>(
        mut self,
        op: Op,
        handler: H,
        layer: impl FnOnce(MethodRouter<AppState>) -> MethodRouter<AppState>,
    ) -> Self
    where
        H: Handler<T, AppState>,
        T: 'static,
    {
        // Every method the table uses has a filter; GET also answers HEAD.
        let filter = MethodFilter::try_from(op.method.clone()).unwrap_or(MethodFilter::GET);
        self.router = self.router.route(op.path, layer(on(filter, handler)));
        self.ops.push(op);
        self
    }

    pub(super) fn merge(mut self, other: Self) -> Self {
        self.router = self.router.merge(other.router);
        self.ops.extend(other.ops);
        self
    }

    pub(super) fn into_router(self) -> Router<AppState> {
        self.router
    }

    pub(super) fn ops(&self) -> &[Op] {
        &self.ops
    }
}

fn component(kind: &str, name: &str) -> Value {
    json!({ "$ref": format!("#/components/{kind}/{name}") })
}

fn schema_ref(name: &str) -> Value {
    component("schemas", name)
}

/// The parameter component of a path parameter: `{chart}` is `ChartKey`.
fn path_param_name(name: &str) -> String {
    let mut chars = name.chars();
    let first = chars.next().map(|c| c.to_ascii_uppercase());
    format!("{}{}Path", first.unwrap_or_default(), chars.as_str())
}

/// Path parameters: name, description.
const PATH_PARAMS: [(&str, &str); 7] = [
    ("workspace", "A workspace id: `ws_` and 32 hex digits."),
    ("id", "An API key id: `key_` and 32 hex digits."),
    ("chart", "The customer's chart key (1 to 128 characters)."),
    ("event", "The customer's event key (1 to 128 characters)."),
    ("token", "A hold token: `ht_` and 64 lowercase hex digits."),
    (
        "file",
        "A public chart artefact: its content hash, 32 lowercase hex digits, and `.ccf`.",
    ),
    ("hash", "An asset's content hash: 32 lowercase hex digits."),
];

/// The operation's tag: its resource.
fn tag(path: &str) -> &str {
    if path.starts_with("/v1/public/") {
        return "public";
    }
    if path.starts_with("/console/api/") {
        return "console";
    }
    match path.strip_prefix("/v1/") {
        Some(rest) if !rest.ends_with(".json") => rest.split('/').next().unwrap_or(rest),
        _ => "system",
    }
}

/// The components.responses entry of a problem+json status.
fn problem_response(status: u16) -> Option<&'static str> {
    Some(match status {
        400 => "BadRequest",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "NotFound",
        409 => "Conflict",
        410 => "Gone",
        422 => "UnprocessableContent",
        429 => "TooManyRequests",
        500 => "InternalError",
        503 => "ServiceUnavailable",
        _ => return None,
    })
}

const INFO: &str = "The Cavea server's REST API (`docs/08-server.md` §6). JSON bodies with \
`snake_case` fields; times are RFC 3339 UTC. Every response carries `Cavea-Request-Id`; a caller may \
send its own (1 to 64 characters of `A-Z a-z 0-9 . _ -`). Errors are `application/problem+json` \
with a stable `code` (08 §6.4): match on `code`, not on `message`. Lists are \
`{\"data\": [..], \"next\": <cursor or null>}`, oldest first; pass `next` back as `after`. The \
WebSocket protocol of `GET /v1/public/rt` is 08 §8, which OpenAPI cannot describe. The \
`/console/api` routes take the console session cookie (09 §10); their non-safe methods send \
`Cavea-Csrf-Token`.";

/// The whole document.
#[must_use]
pub fn document() -> Value {
    let routes = super::routes();
    let mut paths = Map::new();
    for op in routes.ops() {
        let item = paths
            .entry(op.path)
            .or_insert_with(|| Value::Object(Map::new()));
        item[op.method.as_str().to_ascii_lowercase()] = op.to_json();
    }
    let mut parameters = Map::new();
    let used: Vec<&str> = routes.ops().iter().flat_map(Op::path_params).collect();
    let statuses: Vec<u16> = routes.ops().iter().flat_map(Op::error_statuses).collect();
    for (name, description) in PATH_PARAMS.into_iter().filter(|(n, _)| used.contains(n)) {
        parameters.insert(
            path_param_name(name),
            json!({
                "name": name, "in": "path", "required": true,
                "description": description, "schema": { "type": "string" },
            }),
        );
    }
    parameters.extend(
        json!({
            "Limit": {
                "name": "limit", "in": "query", "required": false,
                "description": "Items per page.",
                "schema": { "type": "integer", "minimum": 1, "maximum": 100 },
            },
            "After": {
                "name": "after", "in": "query", "required": false,
                "description": "The `next` of the previous page.",
                "schema": { "type": "string" },
            },
            "IdempotencyKey": {
                "name": "Idempotency-Key", "in": "header", "required": false,
                "description": "1 to 255 printable ASCII characters, kept 24 hours (08 §6.1). The same \
key with another request is `409 idempotency_mismatch`.",
                "schema": { "type": "string", "minLength": 1, "maxLength": 255 },
            },
            "IfMatch": {
                "name": "If-Match", "in": "header", "required": false,
                "description": "The draft's etag as `draft_etag` gives it, or `*`.",
                "schema": { "type": "string" },
            },
            "CsrfToken": {
                "name": "Cavea-Csrf-Token", "in": "header", "required": false,
                "description": "Required: the session's `csrf_token` from `GET /console/api/me` (or \
sign-in).",
                "schema": { "type": "string" },
            },
            "IfMatchRequired": {
                "name": "If-Match", "in": "header", "required": true,
                "description": "The draft's etag as `draft_etag` gives it, or `*` to overwrite \
whatever is there.",
                "schema": { "type": "string" },
            },
        })
        .as_object()
        .cloned()
        .unwrap_or_default(),
    );
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Cavea server API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": INFO,
        },
        "paths": paths,
        "components": {
            "schemas": schemas::all(),
            "parameters": parameters,
            "headers": {
                "RequestId": {
                    "description": "The request's id: `req_` and 32 hex digits, or the caller's own.",
                    "schema": { "type": "string" },
                },
                "RetryAfter": {
                    "description": "Seconds to wait before retrying.",
                    "schema": { "type": "integer", "minimum": 1 },
                },
                "ETag": {
                    "description": "The representation's entity tag.",
                    "schema": { "type": "string" },
                },
                "SetCookie": {
                    "description": "The console session: `__Host-cavea_console=cs_…; Path=/; \
    Max-Age=604800; HttpOnly; SameSite=Lax; Secure` (without `__Host-` and `Secure` when \
    `CAVEA_CONSOLE_COOKIE_SECURE=false`); `Max-Age=0` clears it.",
                    "schema": { "type": "string" },
                },
            },
            "responses": problem_responses(&statuses),
            "securitySchemes": {
                "adminKey": {
                    "type": "http", "scheme": "bearer",
                    "description": "The operator's `CAVEA_ADMIN_KEY` (08 §11).",
                },
                "secretKey": {
                    "type": "http", "scheme": "bearer", "bearerFormat": "sk_…",
                    "description": "A workspace's secret key, for the customer's backend.",
                },
                "publishableKey": {
                    "type": "http", "scheme": "bearer", "bearerFormat": "pk_…",
                    "description": "A workspace's publishable key, for the buyer's browser.",
                },
                "publishableKeyQuery": {
                    "type": "apiKey", "in": "query", "name": "key",
                    "description": "A publishable key in the URL: the realtime routes (D-31).",
                },
                "sessionToken": {
                    "type": "apiKey", "in": "header", "name": "Cavea-Session",
                    "description": "A session token signed by the customer's backend (08 §11). \
    Required when the event sets `require_session`; verified whenever sent.",
                },
                "holdToken": {
                    "type": "apiKey", "in": "header", "name": "Cavea-Hold-Token",
                    "description": "The buyer's hold token.",
                },
                "consoleSession": {
                    "type": "apiKey", "in": "cookie", "name": "__Host-cavea_console",
                    "description": "A staff member's console session (09 §2), set by \
    `POST /console/api/session` or `/console/api/setup`; `cavea_console` when \
    `CAVEA_CONSOLE_COOKIE_SECURE=false`. Not the buyer's session token. With it, a non-safe method \
    sends `Cavea-Csrf-Token`.",
                },
            },
        },
    })
}

/// One response per problem status an operation uses, listing its codes (08
/// §6.4). Every code is in the `Problem` schema's `code` enum regardless.
fn problem_responses(used: &[u16]) -> Map<String, Value> {
    let mut responses = Map::new();
    for code in Code::ALL {
        let status = code.status().as_u16();
        let Some(name) = problem_response(status).filter(|_| used.contains(&status)) else {
            continue;
        };
        let entry = responses.entry(name).or_insert_with(|| {
            let mut headers = json!({ "Cavea-Request-Id": component("headers", "RequestId") });
            if matches!(status, 429 | 503) {
                headers["Retry-After"] = component("headers", "RetryAfter");
            }
            json!({
                "description": "",
                "headers": headers,
                "content": {
                    "application/problem+json": { "schema": schema_ref("Problem") },
                },
            })
        });
        let description = entry["description"].as_str().unwrap_or_default();
        let joined = if description.is_empty() {
            format!("`{}`", code.as_str())
        } else {
            format!("{description}, `{}`", code.as_str())
        };
        entry["description"] = joined.into();
    }
    responses
}

/// The document as served and committed: pretty JSON and a final newline.
#[must_use]
pub fn text() -> String {
    let mut text = serde_json::to_string_pretty(&document()).unwrap_or_default();
    text.push('\n');
    text
}

/// `GET /v1/openapi.json`: built once, then served from memory. No
/// credential: it describes the API, nothing of any workspace.
pub(super) async fn serve(headers: HeaderMap) -> Response {
    static SERVED: OnceLock<(Bytes, HeaderValue)> = OnceLock::new();
    let (body, etag) = SERVED.get_or_init(|| {
        let text = text();
        let mut etag = String::from("\"");
        super::auth::push_hex(&mut etag, &super::auth::sha256(&text)[..16]);
        etag.push('"');
        let etag = HeaderValue::try_from(etag).unwrap_or(HeaderValue::from_static("\"\""));
        (Bytes::from(text), etag)
    });
    let (listed, any) = super::public::none_match(&headers, etag);
    let mut response = if listed || any {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let mut response = body.clone().into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        response
    };
    let headers = response.headers_mut();
    headers.insert(header::ETAG, etag.clone());
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    response
}
