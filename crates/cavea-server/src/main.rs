//! `cavea-server`: read configuration, start logging, migrate the database,
//! serve until Ctrl-C or SIGTERM, then stop within 20 seconds (08 §14,
//! `cavea_server::serve`).

use std::sync::Arc;

use anyhow::Context as _;
use cavea_server::config::Config;
use cavea_server::http::AppState;
use cavea_server::realtime::{Realtime, Timing};
use cavea_server::seats::{Seats, ServerClock};
use cavea_server::store;
use tokio::net::TcpListener;

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
    let realtime = Realtime::new(
        config.rt_max_connections.get() as usize,
        config.rt_max_per_ip.get(),
        Timing::default(),
    );
    let state = AppState {
        config: Arc::new(config),
        db,
        seats,
        realtime,
    };
    cavea_server::serve(listener, state, shutdown_signal()).await?;
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
