//! Settings from `CAVEA_`-prefixed environment variables (08 §13). Every
//! setting but the database URL has a safe default; a malformed value is an
//! error, never ignored.

use std::fmt;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::str::FromStr;

use cavea_telemetry::LogFormat;
use sqlx::ConnectOptions as _;
use sqlx::postgres::PgConnectOptions;

#[derive(Debug, Clone)]
pub struct Config {
    /// `CAVEA_LISTEN`: the socket address to bind.
    pub listen: SocketAddr,
    /// `CAVEA_LOG`: a `tracing` filter such as `info,cavea_server=debug`.
    /// Checked when logging starts.
    pub log: String,
    /// `CAVEA_LOG_FORMAT`: `json` (production) or `text` (development).
    pub log_format: LogFormat,
    /// `CAVEA_DATABASE_URL`: the PostgreSQL connection URL. Required.
    pub database_url: DatabaseUrl,
    /// `CAVEA_DB_POOL`: the most connections the pool opens.
    pub db_pool: NonZeroU32,
}

impl Config {
    /// Read every setting. Add a new one here, with its default, and to 08 §13.
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            listen: setting("CAVEA_LISTEN", || SocketAddr::from(([0, 0, 0, 0], 8080)))?,
            log: setting("CAVEA_LOG", || "info".to_owned())?,
            log_format: setting("CAVEA_LOG_FORMAT", LogFormat::default)?,
            database_url: optional("CAVEA_DATABASE_URL")?.ok_or(ConfigError {
                var: "CAVEA_DATABASE_URL",
                reason: "is required: set it to a PostgreSQL URL such as \
                         postgres://user:password@host:5432/cavea"
                    .to_owned(),
            })?,
            db_pool: setting("CAVEA_DB_POOL", || NonZeroU32::MIN.saturating_add(15))?,
        })
    }
}

/// A parsed PostgreSQL connection URL. It holds the password, so neither it
/// nor anything derived from it is ever printed.
#[derive(Clone)]
pub struct DatabaseUrl(PgConnectOptions);

impl DatabaseUrl {
    /// Connection options with statement logging off: SQL text stays out of
    /// the logs, and database calls are traced through `db` spans instead.
    #[must_use]
    pub fn options(&self) -> PgConnectOptions {
        self.0.clone().disable_statement_logging()
    }
}

impl FromStr for DatabaseUrl {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // The parser's message may quote the URL; it is replaced, not passed on.
        s.parse()
            .map(Self)
            .map_err(|_| "not a valid PostgreSQL connection URL")
    }
}

impl fmt::Debug for DatabaseUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DatabaseUrl(..)")
    }
}

/// A setting that is missing or cannot be used. The value is not repeated:
/// some settings are secrets.
#[derive(Debug)]
pub struct ConfigError {
    pub var: &'static str,
    /// Reads after the variable's name: "is invalid: …" or "is required: …".
    pub reason: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.var, self.reason)
    }
}

impl std::error::Error for ConfigError {}

fn setting<T>(var: &'static str, default: impl FnOnce() -> T) -> Result<T, ConfigError>
where
    T: FromStr<Err: fmt::Display>,
{
    Ok(optional(var)?.unwrap_or_else(default))
}

fn optional<T>(var: &'static str) -> Result<Option<T>, ConfigError>
where
    T: FromStr<Err: fmt::Display>,
{
    let Some(raw) = std::env::var_os(var) else {
        return Ok(None);
    };
    let error = |reason: String| ConfigError {
        var,
        reason: format!("is invalid: {reason}"),
    };
    raw.to_str()
        .ok_or_else(|| error("not valid UTF-8".to_owned()))?
        .parse()
        .map(Some)
        .map_err(|e: T::Err| error(e.to_string()))
}
