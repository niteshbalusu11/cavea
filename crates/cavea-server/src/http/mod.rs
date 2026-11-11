//! The router, application state, credentials, extractors and problem+json
//! errors (08 §3, §6, §11).

/// SQL that formats a `timestamptz` column as RFC 3339 UTC to the second:
/// `rfc3339!("k", ".created_at")`.
macro_rules! rfc3339 {
    ($($column:literal),+) => {
        concat!("to_char(", $($column,)+ " AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')")
    };
}

pub mod auth;
#[macro_use]
mod charts;
mod error;
mod events;
pub mod extract;
mod keys;
mod public;
mod seats;
mod workspaces;

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::get;
use cavea_telemetry::RequestLayer;
use serde::Serialize;
use sqlx::PgPool;

use crate::config::Config;
use crate::seats::Seats;

pub use auth::{AdminKey, AdminKeyAuth, Auth, PublishableKey, SecretKey};
pub use error::{ApiError, Code};
pub use extract::{Json, Path, Query};

/// Shared with every handler through `State<AppState>`. Cheap to clone.
#[derive(Debug, Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    /// The PostgreSQL pool (`CAVEA_DB_POOL` connections). Wrap each call in
    /// `cavea_telemetry::db_span`.
    pub db: PgPool,
    /// The actor registry and chart cache: seat commands go through it.
    pub seats: Seats,
}

/// The whole HTTP surface.
pub fn router(state: AppState) -> Router {
    finish(routes(), state)
}

fn routes() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .merge(workspaces::routes())
        .merge(keys::routes())
        .merge(charts::routes())
        .merge(events::routes())
        .merge(seats::routes())
        .merge(public::routes())
}

/// Add the problem+json fallbacks, the default body limit, CORS for the public
/// routes and the request span layer. The layers only wrap routes added before them, so every route goes
/// in before this.
fn finish(routes: Router<AppState>, state: AppState) -> Router {
    routes
        .fallback(|| async { ApiError::new(Code::NotFound).with_message("No such endpoint.") })
        .method_not_allowed_fallback(|| async {
            ApiError::new(Code::NotFound).with_message("No such endpoint for this method.")
        })
        .layer(DefaultBodyLimit::max(extract::DEFAULT_BODY_LIMIT))
        .layer(middleware::from_fn_with_state(state.clone(), public::cors))
        .layer(RequestLayer::new().quiet(&["/healthz"]))
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

/// A page of a list (08 §6.1): `{"data": [..], "next": "<cursor>"}`. `next` is
/// the id of the last item when more may follow, else `null`; pass it back as
/// `?after=`.
#[derive(Debug, Serialize)]
pub struct List<T> {
    data: Vec<T>,
    next: Option<String>,
}

impl<T> List<T> {
    /// `items` was fetched with `limit + 1` rows: the extra row only says that
    /// another page exists.
    fn new(mut items: Vec<T>, limit: u32, id: impl Fn(&T) -> String) -> Self {
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        let next = if items.len() > limit {
            items.truncate(limit);
            items.last().map(id)
        } else {
            None
        };
        Self { data: items, next }
    }
}

#[cfg(test)]
mod tests;
