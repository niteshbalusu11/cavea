//! The self-hosted Cavea server: one binary holding per-event seat state, with
//! the REST API, realtime hub, webhooks and console hosting. PostgreSQL is its
//! only required dependency. See `docs/08-server.md`; modules follow its §3.

pub mod charts;
pub mod config;
pub mod http;
pub mod limiter;
pub mod metrics;
pub mod net;
pub mod realtime;
pub mod seats;
pub mod store;
pub mod verify;

use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::time::Instant;

/// From the signal to exit (08 §14).
pub const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(20);
/// How long requests in flight may take to finish after the signal; the rest
/// of the deadline is for the actors to commit and release their leases.
pub const DRAIN: Duration = Duration::from_secs(10);

/// Serve `state` on `listener` until `signal` resolves, then stop within
/// [`SHUTDOWN_DEADLINE`] (08 §14): every realtime connection is sent CLOSE 3
/// and closed, so that none holds the drain open; the listener closes and
/// requests in flight get up to [`DRAIN`] to finish, their commands included;
/// then every actor commits what it accepted and releases its lease, and only
/// then does the server wait, until the deadline, for connections that have
/// not closed yet (a stalled SSE response, whose client does not read).
pub async fn serve(
    listener: TcpListener,
    state: http::AppState,
    signal: impl Future<Output = ()>,
) -> std::io::Result<()> {
    let seats = state.seats.clone();
    let realtime = state.realtime.clone();
    let probe = tokio::spawn(state.health.clone().watch(state.db.clone()));
    // In the background, so that the server answers at once; a request for an
    // event not loaded yet loads it itself.
    let preload = tokio::spawn({
        let seats = seats.clone();
        async move {
            if let Err(error) = seats.preload().await {
                tracing::warn!(%error, "cannot list events with holds; they load on their next request");
            }
        }
    });
    let (stop, stopping) = tokio::sync::watch::channel(false);
    let app = http::router(state).into_make_service_with_connect_info::<SocketAddr>();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        let mut stopping = stopping;
        let _ = stopping.wait_for(|&s| s).await;
    });
    let mut server = std::pin::pin!(server.into_future());
    tokio::select! {
        result = &mut server => result?,
        () = signal => {
            let deadline = Instant::now() + SHUTDOWN_DEADLINE;
            realtime.shutdown();
            stop.send_replace(true);
            if tokio::time::timeout(DRAIN, &mut server).await.is_err() {
                tracing::warn!("requests still in flight after the drain period; closing them");
            }
            preload.abort();
            probe.abort();
            if tokio::time::timeout_at(deadline, seats.shutdown()).await.is_err() {
                tracing::error!("actors did not stop within the shutdown deadline");
            }
            // After the actors: a stalled SSE response keeps its slot until
            // its client reads, which must not take the actors' time.
            if tokio::time::timeout_at(deadline, realtime.closed()).await.is_err() {
                tracing::warn!(open = realtime.connections(), "realtime connections still open");
            }
        }
    }
    tracing::info!("stopped");
    Ok(())
}
