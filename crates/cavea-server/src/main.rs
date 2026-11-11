//! The self-hosted Cavea server: one binary holding per-event seat state, with
//! the REST API, realtime hub, webhooks and console hosting. PostgreSQL is its only
//! required dependency. Configured through `CAVEA_`-prefixed environment
//! variables. See `docs/08-server.md`.

use std::net::SocketAddr;

use anyhow::Context as _;
use axum::{Router, routing::get};
use tokio::net::TcpListener;

/// Server settings, read from `CAVEA_`-prefixed environment variables
/// (`docs/08-server.md` §13). Every setting has a safe default.
struct Config {
    /// `CAVEA_LISTEN`: the socket address to bind.
    listen: SocketAddr,
    /// `CAVEA_LOG`: a `tracing` filter directive such as `info` or `cavea_server=debug`.
    log: String,
}

impl Config {
    fn from_env() -> anyhow::Result<Self> {
        let listen = match std::env::var("CAVEA_LISTEN") {
            Ok(value) => value
                .parse()
                .with_context(|| format!("CAVEA_LISTEN is not a socket address: {value:?}"))?,
            Err(_) => SocketAddr::from(([0, 0, 0, 0], 8080)),
        };
        let log = std::env::var("CAVEA_LOG").unwrap_or_else(|_| "info".to_owned());
        Ok(Self { listen, log })
    }
}

fn app() -> Router {
    Router::new().route("/healthz", get(|| async { "ok" }))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_new(&config.log)
                .with_context(|| format!("CAVEA_LOG is not a valid filter: {:?}", config.log))?,
        )
        .init();

    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("cannot bind {}", config.listen))?;
    tracing::info!(addr = %listener.local_addr()?, version = env!("CARGO_PKG_VERSION"), "listening");
    axum::serve(listener, app())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
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

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt as _;

    #[tokio::test]
    async fn healthz_responds_ok() {
        let response = super::app()
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body = axum::body::to_bytes(response.into_body(), 64)
            .await
            .unwrap();
        assert_eq!(&body[..], b"ok");
    }
}
