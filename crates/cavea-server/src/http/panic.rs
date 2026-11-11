//! A panic in a handler, an extractor or a layer inside this one answers
//! `500 internal_error` instead of dropping the connection. The panic hook
//! (`cavea_telemetry::log_panic`) has already logged it, in the request's
//! span; the request layer around this one adds the request id and counts the
//! `500` under the route. The handler's future is dropped and never polled
//! again; the locks requests share (the chart cache, the rate limiter, the
//! actor registry) are taken with `PoisonError::into_inner`, so a panic while
//! one is held does not stop the next request. A panic inside an event actor
//! is the engine's: it fails that actor's callers with `503 unavailable`, and
//! the next command reloads the event (08 §16).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::extract::Request;
use axum::response::{IntoResponse as _, Response};
use tower::{Layer, Service};

use super::{ApiError, Code};

/// A layer, not a `middleware::from_fn`: it wraps the inner future in place,
/// with no allocation, so it costs nothing a request can measure.
#[derive(Debug, Clone, Copy)]
pub(super) struct CatchPanicLayer;

impl<S> Layer<S> for CatchPanicLayer {
    type Service = CatchPanic<S>;

    fn layer(&self, inner: S) -> CatchPanic<S> {
        CatchPanic(inner)
    }
}

#[derive(Debug, Clone)]
pub(super) struct CatchPanic<S>(S);

impl<S> Service<Request> for CatchPanic<S>
where
    S: Service<Request, Response = Response>,
    S::Future: Unpin,
{
    type Response = Response;
    type Error = S::Error;
    type Future = CatchUnwind<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        CatchUnwind(self.0.call(request))
    }
}

/// The inner future, answering `500` if polling it panics. It is never polled
/// again after that.
#[derive(Debug)]
pub(super) struct CatchUnwind<F>(F);

impl<F, E> Future for CatchUnwind<F>
where
    F: Future<Output = Result<Response, E>> + Unpin,
{
    type Output = Result<Response, E>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let inner = Pin::new(&mut self.0);
        match catch_unwind(AssertUnwindSafe(|| inner.poll(cx))) {
            Ok(poll) => poll,
            Err(_) => Poll::Ready(Ok(ApiError::new(Code::InternalError).into_response())),
        }
    }
}
