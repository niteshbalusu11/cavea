//! The self-hosted Cavea server: one binary holding per-event seat state, with
//! the REST API, realtime hub, webhooks and console hosting. PostgreSQL is its
//! only required dependency. See `docs/08-server.md`; modules follow its §3.

pub mod config;
pub mod http;
