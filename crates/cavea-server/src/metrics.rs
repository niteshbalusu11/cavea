//! The server's metrics (08 §15): what the engine records, the HTTP requests
//! by route and status, the realtime hub, the rate limiter, the database pool
//! and the log output, rendered as Prometheus text for `GET /metrics`.
//!
//! Every label is bounded: a route is the matched template (or
//! `<unmatched>`), a status an HTTP status, an op or a result one of the
//! engine's, a transport or a budget one of two. No key, token, event or
//! address is ever a label.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, PoisonError, RwLock};
use std::time::Duration;

use cavea_engine::metrics::{Exposition, Histogram, METRICS};

use crate::http::AppState;
use crate::limiter::Budget;
use crate::realtime::Transport;

/// One route's requests: counts per status (100 to 599) and durations.
struct Route {
    statuses: [AtomicU64; 500],
    duration: Histogram,
}

impl Route {
    fn new() -> Self {
        Self {
            statuses: [const { AtomicU64::new(0) }; 500],
            duration: Histogram::seconds(),
        }
    }

    fn record(&self, status: u16, elapsed: Duration) {
        if let Some(count) = self.statuses.get(usize::from(status).wrapping_sub(100)) {
            count.fetch_add(1, Ordering::Relaxed);
        }
        self.duration.observe_duration(elapsed);
    }
}

/// Routes as they are first requested; at most the router's templates.
static ROUTES: LazyLock<RwLock<HashMap<Box<str>, Route>>> = LazyLock::new(RwLock::default);

/// Count one request: the request layer's callback
/// ([`cavea_telemetry::RequestLayer::on_completed`]).
pub fn record_request(route: &str, status: u16, elapsed: Duration) {
    {
        let routes = ROUTES.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(r) = routes.get(route) {
            r.record(status, elapsed);
            return;
        }
    }
    let mut routes = ROUTES.write().unwrap_or_else(PoisonError::into_inner);
    routes
        .entry(route.into())
        .or_insert_with(Route::new)
        .record(status, elapsed);
}

/// Every metric, as Prometheus text (format 0.0.4).
#[must_use]
pub fn render(state: &AppState) -> String {
    let mut out = Exposition::new();
    METRICS.render(&mut out, state.seats.registry.load());
    {
        let routes = ROUTES.read().unwrap_or_else(PoisonError::into_inner);
        let mut sorted: Vec<_> = routes.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        out.family(
            "cavea_http_requests_total",
            "counter",
            "HTTP requests by matched route and status.",
        );
        for (route, r) in &sorted {
            for (status, count) in (100u16..).zip(&r.statuses) {
                let n = count.load(Ordering::Relaxed);
                if n > 0 {
                    let status = status.to_string();
                    out.sample(
                        "cavea_http_requests_total",
                        &[("route", route), ("status", &status)],
                        n,
                    );
                }
            }
        }
        out.family(
            "cavea_http_duration_seconds",
            "histogram",
            "HTTP request durations by matched route, until the response starts.",
        );
        for (route, r) in &sorted {
            out.histogram(
                "cavea_http_duration_seconds",
                &[("route", route)],
                &r.duration,
            );
        }
    }
    let realtime = &state.realtime;
    out.gauge(
        "cavea_rt_connections",
        "Open realtime connections, subscribed or not.",
        realtime.connections(),
    );
    out.family(
        "cavea_rt_subscribers",
        "gauge",
        "Subscribed realtime connections by transport.",
    );
    for transport in [Transport::WebSocket, Transport::Sse] {
        out.sample(
            "cavea_rt_subscribers",
            &[("transport", transport.as_str())],
            realtime.subscribers(transport),
        );
    }
    out.counter(
        "cavea_rt_frames_total",
        "Frames written to realtime connections.",
        realtime.frames(),
    );
    out.counter(
        "cavea_rt_resyncs_total",
        "RESYNCs sent to subscribers that fell behind.",
        realtime.resyncs(),
    );
    out.family(
        "cavea_rate_limited_total",
        "counter",
        "Public requests refused by the rate limiter, by budget.",
    );
    for (budget, name) in [(Budget::Holds, "holds"), (Budget::Reads, "reads")] {
        out.sample(
            "cavea_rate_limited_total",
            &[("budget", name)],
            state.limiter.refused(budget),
        );
    }
    let (open, idle) = (state.db.size(), state.db.num_idle());
    out.gauge(
        "cavea_db_pool_in_use",
        "Database connections in use.",
        (open as usize).saturating_sub(idle),
    );
    out.gauge(
        "cavea_db_pool_connections",
        "Database connections open, in use or idle.",
        open,
    );
    out.counter(
        "cavea_log_records_dropped_total",
        "Log records dropped because the log output was full or failing.",
        cavea_telemetry::dropped_records(),
    );
    out.into_string()
}
