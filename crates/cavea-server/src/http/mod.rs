//! The router, application state and problem+json errors (08 §3, §6).

mod error;

use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use cavea_telemetry::RequestLayer;

use crate::config::Config;

pub use error::{ApiError, Code, Json};

/// Shared with every handler through `State<AppState>`. Cheap to clone. Later
/// slices add the actor registry and the database pool here.
#[derive(Debug, Clone)]
pub struct AppState {
    pub config: Arc<Config>,
}

/// The whole HTTP surface.
pub fn router(state: AppState) -> Router {
    finish(Router::new().route("/healthz", get(healthz)), state)
}

/// Add the problem+json fallbacks and the request span layer. The layer only
/// wraps routes added before it, so every route goes in before this.
fn finish(routes: Router<AppState>, state: AppState) -> Router {
    routes
        .fallback(|| async { ApiError::new(Code::NotFound).with_message("No such endpoint.") })
        .method_not_allowed_fallback(|| async {
            ApiError::new(Code::NotFound).with_message("No such endpoint for this method.")
        })
        .layer(RequestLayer::new().quiet(&["/healthz"]))
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

#[cfg(test)]
mod tests;
