//! `cavea-server`: read configuration, start logging, migrate the database,
//! serve until Ctrl-C or SIGTERM, then stop within 20 seconds (08 §14).

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use cavea_server::config::Config;
use cavea_server::http::{self, AppState};
use cavea_server::seats::{Seats, ServerClock};
use cavea_server::store;
use tokio::net::TcpListener;
use tokio::time::Instant;

/// From the signal to exit (08 §14).
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(20);
/// How long requests in flight may take to finish after the signal; the rest
/// of the deadline is for the actors to commit and release their leases.
const DRAIN: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    cavea_telemetry::init(&config.log, config.log_format).context("CAVEA_LOG is invalid")?;

    // Never print the URL: it carries the password.
    let options = config.database_url.options();
    store::migrate(&options).await?;
    let db = store::connect(options, config.db_pool.get())
        .await
        .context("cannot connect to PostgreSQL (CAVEA_DATABASE_URL)")?;

    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("cannot bind {}", config.listen))?;
    tracing::info!(addr = %listener.local_addr()?, version = env!("CARGO_PKG_VERSION"), "listening");
    if config.admin_key.is_none() {
        tracing::warn!("CAVEA_ADMIN_KEY is not set: the workspace endpoints refuse every request");
    }
    let seats = Seats::new(db.clone(), &config, ServerClock::System);
    let state = AppState {
        config: Arc::new(config),
        db,
        seats: seats.clone(),
    };
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
    let server = axum::serve(listener, http::router(state)).with_graceful_shutdown(async move {
        let mut stopping = stopping;
        let _ = stopping.wait_for(|&s| s).await;
    });
    let mut server = std::pin::pin!(server.into_future());
    tokio::select! {
        result = &mut server => result?,
        () = shutdown_signal() => {
            let deadline = Instant::now() + SHUTDOWN_DEADLINE;
            // Stop accepting connections and let requests in flight finish,
            // their commands included, before the actors stop.
            stop.send_replace(true);
            if tokio::time::timeout(DRAIN, &mut server).await.is_err() {
                tracing::warn!("requests still in flight after the drain period; closing them");
            }
            preload.abort();
            if tokio::time::timeout_at(deadline, seats.shutdown()).await.is_err() {
                tracing::error!("actors did not stop within the shutdown deadline");
            }
        }
    }
    tracing::info!("stopped");
    Ok(())
}

/// Resolves on Ctrl-C or, on Unix, SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "cannot listen for Ctrl-C");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::error!(%error, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutting down");
}
