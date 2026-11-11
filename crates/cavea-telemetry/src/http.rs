//! The request span layer: a request id, one span per request, one log record
//! when it completes, and the `Cavea-Request-Id` response header.

use std::collections::hash_map::RandomState;
use std::fmt;
use std::future::Future;
use std::hash::BuildHasher as _;
use std::pin::Pin;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Instant;

use axum::extract::MatchedPath;
use axum::http::{HeaderName, HeaderValue, Request, Response};
use tower::{Layer, Service};
use tracing::field::Empty;
use tracing::{Instrument as _, Span, debug, info, info_span};

pub const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("cavea-request-id");

/// The `route` of a request that matched no route. Raw paths are never logged:
/// they carry customer keys.
pub const UNMATCHED_ROUTE: &str = "<unmatched>";

/// Longest inbound request id the server adopts.
const MAX_INBOUND_LEN: usize = 64;

/// A request id: `req_` and 32 lowercase hex digits when generated, or the
/// caller's own id when it passes [`RequestId::from_inbound`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestId(Box<str>);

impl RequestId {
    /// A new id. 128 bits from a SipHash keyed randomly once per process over
    /// a counter: unique in practice across nodes and restarts, and it reveals
    /// nothing about request volume.
    #[must_use]
    pub fn generate() -> Self {
        static KEYS: OnceLock<RandomState> = OnceLock::new();
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let keys = KEYS.get_or_init(RandomState::new);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let hi = keys.hash_one((n, 0u8));
        let lo = keys.hash_one((n, 1u8));
        Self(format!("req_{hi:016x}{lo:016x}").into())
    }

    /// The caller's id, if it is 1 to 64 characters of `A-Z a-z 0-9 . _ -`
    /// (08 §6.1). Anything else is ignored, so it cannot inject into logs.
    #[must_use]
    pub fn from_inbound(value: &HeaderValue) -> Option<Self> {
        let bytes = value.as_bytes();
        let valid = (1..=MAX_INBOUND_LEN).contains(&bytes.len())
            && bytes
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        valid
            .then(|| value.to_str().ok())
            .flatten()
            .map(|s| Self(s.into()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

struct Scope {
    id: RequestId,
    span: Span,
}

tokio::task_local! {
    static SCOPE: Scope;
}

/// The id of the request being handled, or `None` outside one (or on a task
/// the handler spawned).
#[must_use]
pub fn request_id() -> Option<RequestId> {
    SCOPE.try_with(|s| s.id.clone()).ok()
}

/// Record the workspace on the current request's span, wherever the caller is
/// in its span tree. Does nothing outside a request.
pub fn record_workspace(workspace: &str) {
    let _ = SCOPE.try_with(|s| {
        s.span.record("workspace", workspace);
    });
}

/// Record the event key on the current request's span. Does nothing outside a
/// request.
pub fn record_event(event: &str) {
    let _ = SCOPE.try_with(|s| {
        s.span.record("event", event);
    });
}

/// Wraps every request in a [`span::REQUEST`](crate::span::REQUEST) span. Add
/// it with `Router::layer` after every route and fallback, so that the matched
/// route is known.
#[derive(Debug, Clone, Copy, Default)]
pub struct RequestLayer {
    quiet: &'static [&'static str],
}

impl RequestLayer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Routes whose successful completion is logged at `debug` rather than
    /// `info`, such as health checks.
    #[must_use]
    pub fn quiet(self, routes: &'static [&'static str]) -> Self {
        Self { quiet: routes }
    }
}

impl<S> Layer<S> for RequestLayer {
    type Service = RequestService<S>;

    fn layer(&self, inner: S) -> RequestService<S> {
        RequestService {
            inner,
            quiet: self.quiet,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RequestService<S> {
    inner: S,
    quiet: &'static [&'static str],
}

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

impl<S, B, R> Service<Request<B>> for RequestService<S>
where
    S: Service<Request<B>, Response = Response<R>>,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    R: Send + 'static,
{
    type Response = Response<R>;
    type Error = S::Error;
    type Future = BoxFuture<Result<Response<R>, S::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<B>) -> Self::Future {
        let start = Instant::now();
        let id = request
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(RequestId::from_inbound)
            .unwrap_or_else(RequestId::generate);
        let route = request
            .extensions()
            .get::<MatchedPath>()
            .map_or(UNMATCHED_ROUTE, MatchedPath::as_str);
        let quiet = self.quiet.contains(&route);
        let span = info_span!(
            crate::span::REQUEST,
            request_id = %id,
            method = %request.method(),
            route,
            status = Empty,
            duration_ms = Empty,
            event_id = Empty,
            commit_seq = Empty,
            workspace = Empty,
            event = Empty,
        );
        let header = HeaderValue::from_str(id.as_str()).ok();
        let scope = Scope {
            id,
            span: span.clone(),
        };
        let response = SCOPE.scope(scope, span.in_scope(|| self.inner.call(request)));
        let completion = span.clone();
        Box::pin(
            async move {
                let mut result = response.await;
                let mut quiet = quiet;
                if let Ok(response) = &mut result {
                    quiet &= response.status().is_success();
                    completion.record("status", response.status().as_u16());
                    if let Some(header) = header {
                        response.headers_mut().insert(REQUEST_ID_HEADER, header);
                    }
                }
                // Milliseconds to the microsecond.
                let micros = start.elapsed().as_micros();
                completion.record("duration_ms", micros as f64 / 1e3);
                if quiet {
                    debug!("request completed");
                } else {
                    info!("request completed");
                }
                result
            }
            .instrument(span),
        )
    }
}
