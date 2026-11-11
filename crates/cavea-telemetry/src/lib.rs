//! Structured logging for the server: subscriber setup, the HTTP request span
//! layer, and the span names every crate uses (`AGENTS.md` "Logging",
//! `docs/08-server.md` §15).
//!
//! A request is followed end to end through spans:
//!
//! ```text
//! request {request_id, method, route, status, duration_ms, workspace, event,
//!          event_id, commit_seq}
//! ├─ mailbox_wait {op}         (cavea-engine, while the command is queued)
//! ├─ apply {op}                (cavea-engine, on the actor's task)
//! └─ db {statement}            (database calls made by the handler)
//! commit {event_id, seq, commands}
//!                              (cavea-engine; one per batch, follows_from
//!                               every apply span in the batch)
//! ```
//!
//! `cavea-engine` does not depend on this crate; it creates its spans with the
//! same literal names, and a test in `cavea-server` checks that they agree.

mod http;
mod output;

use std::fmt;
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;

use tracing::Subscriber;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;

pub use output::{Output, QUEUE_RECORDS};

pub use http::{
    Completed, REQUEST_ID_HEADER, RequestId, RequestLayer, RequestService, UNMATCHED_ROUTE,
    record_event, record_workspace, request_id,
};

/// Span names. Fields are listed with each.
pub mod span {
    /// One per HTTP request: `request_id`, `method`, `route` (the matched
    /// template), `status`, `duration_ms`, `workspace`, `event`, and
    /// `event_id` and `commit_seq`, recorded by the engine when a command the
    /// request submitted from this span goes into a commit.
    pub const REQUEST: &str = "request";
    /// A command waiting in an event actor's mailbox: `op`.
    pub const MAILBOX_WAIT: &str = "mailbox_wait";
    /// A command applied to event state, a child of the submitting request's
    /// span even though it runs on the actor's task: `op`.
    pub const APPLY: &str = "apply";
    /// One batch commit: `event_id` (32 hex digits), `seq`, `commands`. A
    /// batch serves many requests, so it is not a child of any of them; it
    /// `follows_from` each `apply` span, and each submitting span gets the
    /// same `event_id` and `commit_seq` = `seq`, so log records inside the
    /// commit and the requests' own records share that pair.
    pub const COMMIT: &str = "commit";
    /// One database call: `statement`, a fixed name, never SQL text or values.
    pub const DB: &str = "db";
}

/// A span for one database call, a child of the current span. `statement` is
/// a fixed name such as `"load_event"`.
#[must_use]
pub fn db_span(statement: &'static str) -> tracing::Span {
    tracing::info_span!(span::DB, statement)
}

/// How log records are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFormat {
    /// One JSON object per line, for production.
    #[default]
    Json,
    /// Human-readable lines, for development.
    Text,
}

impl FromStr for LogFormat {
    type Err = InitError;

    fn from_str(s: &str) -> Result<Self, InitError> {
        match s {
            "json" => Ok(Self::Json),
            "text" => Ok(Self::Text),
            _ => Err(InitError::Format),
        }
    }
}

#[derive(Debug)]
pub enum InitError {
    /// The filter is not valid `tracing` directive syntax.
    Filter(tracing_subscriber::filter::ParseError),
    /// The format is neither `json` nor `text`.
    Format,
    /// A global subscriber is already installed.
    AlreadyInitialised,
    /// The log output's thread could not be started.
    Output(std::io::Error),
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Filter(e) => write!(f, "invalid log filter: {e}"),
            Self::Format => f.write_str("log format must be `json` or `text`"),
            Self::AlreadyInitialised => f.write_str("logging is already initialised"),
            Self::Output(e) => write!(f, "cannot start the log output: {e}"),
        }
    }
}

impl std::error::Error for InitError {}

/// Build the subscriber for `filter` (`tracing` directive syntax, such as
/// `info,cavea_server=debug`) and `format`, writing to `writer`. [`init`] uses
/// this with an [`Output`] to stdout; tests use it to capture output. A record
/// the writer fails to take is lost silently: never reported on standard
/// error, whose failure would panic the task that logged.
pub fn subscriber<W>(
    filter: &str,
    format: LogFormat,
    writer: W,
) -> Result<Box<dyn Subscriber + Send + Sync>, InitError>
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    let filter = EnvFilter::try_new(filter).map_err(InitError::Filter)?;
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .log_internal_errors(false);
    Ok(match format {
        // Every enclosing span with its fields, outermost first, so an event
        // inside `apply` still carries the request id.
        LogFormat::Json => Box::new(
            builder
                .json()
                .flatten_event(true)
                .with_current_span(false)
                .with_span_list(true)
                .finish(),
        ),
        LogFormat::Text => Box::new(builder.finish()),
    })
}

/// The process's log output, once [`init`] has run.
static OUTPUT: OnceLock<Output> = OnceLock::new();

/// How long the output is given, at exit, to write what is queued.
pub const FLUSH_AT_EXIT: Duration = Duration::from_millis(250);

/// Log records dropped since start-up because the output was full or failing
/// (`cavea_log_records_dropped_total`); 0 before [`init`].
#[must_use]
pub fn dropped_records() -> u64 {
    OUTPUT.get().map_or(0, Output::dropped)
}

/// Install the process-wide subscriber, writing to stdout through an
/// [`Output`]: logging never waits for stdout and never fails. Call once,
/// first thing in `main`, and keep the guard until exit: dropping it gives the
/// output [`FLUSH_AT_EXIT`] to write what is queued.
pub fn init(filter: &str, format: LogFormat) -> Result<Logging, InitError> {
    let output = Output::spawn(std::io::stdout(), QUEUE_RECORDS).map_err(InitError::Output)?;
    tracing::subscriber::set_global_default(subscriber(filter, format, output.clone())?)
        .map_err(|_| InitError::AlreadyInitialised)?;
    let _ = OUTPUT.set(output.clone());
    Ok(Logging(output))
}

/// Flushes the log output when dropped.
#[derive(Debug)]
#[must_use = "dropping it at once flushes nothing written after"]
pub struct Logging(Output);

impl Drop for Logging {
    fn drop(&mut self) {
        let _ = self.0.flush(FLUSH_AT_EXIT);
    }
}
