//! Liveness, readiness and metrics (08 §6.2, §15).
//!
//! - `GET /healthz`: `ok` while the process serves HTTP.
//! - `GET /readyz`: `ready`, or `503` with the reason, while the database is
//!   unreachable or the server is shutting down. Migrations run before the
//!   server listens, so a server that answers has applied them; preloading
//!   does not delay readiness (08 §4.7). It reads two flags: probing it costs
//!   no query and never reaches an actor.
//! - `GET /metrics`: Prometheus text, for the admin key only.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use sqlx::PgPool;
use tracing::{Instrument as _, info, warn};

use super::{AdminKeyAuth, AppState};

/// How often the database is probed for readiness.
pub const PROBE_EVERY: Duration = Duration::from_secs(1);
/// A probe that takes longer than this fails.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
}

async fn healthz() -> &'static str {
    "ok"
}

async fn readyz(State(state): State<AppState>) -> (StatusCode, &'static str) {
    if state.realtime.is_stopping() {
        (StatusCode::SERVICE_UNAVAILABLE, "shutting down")
    } else if !state.health.database() {
        (StatusCode::SERVICE_UNAVAILABLE, "database unreachable")
    } else {
        (StatusCode::OK, "ready")
    }
}

async fn metrics(_: AdminKeyAuth, State(state): State<AppState>) -> Response {
    let mut response = crate::metrics::render(&state).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// What readiness knows of the database. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Health {
    database: Arc<AtomicBool>,
}

impl Default for Health {
    /// Reachable: the server has just migrated and connected.
    fn default() -> Self {
        Self {
            database: Arc::new(AtomicBool::new(true)),
        }
    }
}

impl Health {
    /// Whether the last probe reached the database.
    #[must_use]
    pub fn database(&self) -> bool {
        self.database.load(Ordering::Relaxed)
    }

    /// Probe the database every [`PROBE_EVERY`] with one `SELECT 1` through
    /// the pool, which fails after [`PROBE_TIMEOUT`]; runs until dropped.
    pub async fn watch(self, db: PgPool) {
        loop {
            tokio::time::sleep(PROBE_EVERY).await;
            let probe = sqlx::query("SELECT 1")
                .execute(&db)
                .instrument(cavea_telemetry::db_span("probe"));
            let reached = matches!(tokio::time::timeout(PROBE_TIMEOUT, probe).await, Ok(Ok(_)));
            if self.database.swap(reached, Ordering::Relaxed) != reached {
                if reached {
                    info!("database reachable again; ready");
                } else {
                    warn!("database unreachable; not ready");
                }
            }
        }
    }
}
