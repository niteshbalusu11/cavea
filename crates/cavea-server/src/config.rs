//! Settings from `CAVEA_`-prefixed environment variables (08 §13). Every
//! setting but the database URL has a safe default; a malformed value is an
//! error, never ignored.

use std::fmt;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::str::FromStr;

use cavea_telemetry::LogFormat;

use crate::http::AdminKey;
use crate::http::auth::normalise_origin;
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
    /// `CAVEA_NODE_ID`: this node's name in `events.owner_node`. Defaults to
    /// the host name; two nodes on one database must differ.
    pub node_id: String,
    /// `CAVEA_LEASE_SECS`: how long an event's ownership lasts unless renewed.
    pub lease_secs: NonZeroU32,
    /// `CAVEA_HEARTBEAT_SECS`: how long an idle actor waits before renewing its
    /// lease. Less than the lease.
    pub heartbeat_secs: NonZeroU32,
    /// `CAVEA_ADMIN_KEY`: the operator's key for the workspace endpoints, 32
    /// to 512 printable ASCII characters. There is no default: unset, those
    /// endpoints refuse every request.
    pub admin_key: Option<AdminKey>,
    /// `CAVEA_CORS_ORIGINS`: the browser origins the public endpoints answer
    /// (08 §11): `*` (the default) or a comma-separated list.
    pub cors_origins: CorsOrigins,
    /// `CAVEA_SNAPSHOT_MAX_AGE`: seconds a shared cache may serve the public
    /// snapshot and polling answers as fresh (08 §8.5).
    pub snapshot_max_age: u32,
    /// `CAVEA_RT_MAX_CONNECTIONS`: WebSocket and SSE connections this node
    /// holds open at most (12 §3 T11).
    pub rt_max_connections: NonZeroU32,
    /// `CAVEA_RT_MAX_PER_IP`: WebSocket and SSE connections one client IP may
    /// hold open at once.
    pub rt_max_per_ip: NonZeroU32,
}

impl Config {
    /// Read every setting. Add a new one here, with its default, and to 08 §13.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self {
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
            node_id: match optional("CAVEA_NODE_ID")? {
                Some(id) => id,
                None => hostname().ok_or(ConfigError {
                    var: "CAVEA_NODE_ID",
                    reason: "is required: the host name could not be read".to_owned(),
                })?,
            },
            lease_secs: setting("CAVEA_LEASE_SECS", || NonZeroU32::MIN.saturating_add(14))?,
            heartbeat_secs: setting("CAVEA_HEARTBEAT_SECS", || NonZeroU32::MIN.saturating_add(4))?,
            admin_key: optional("CAVEA_ADMIN_KEY")?,
            cors_origins: setting("CAVEA_CORS_ORIGINS", || CorsOrigins::Any)?,
            snapshot_max_age: setting("CAVEA_SNAPSHOT_MAX_AGE", || 1)?,
            rt_max_connections: setting("CAVEA_RT_MAX_CONNECTIONS", || {
                NonZeroU32::MIN.saturating_add(59_999)
            })?,
            rt_max_per_ip: setting("CAVEA_RT_MAX_PER_IP", || {
                NonZeroU32::MIN.saturating_add(199)
            })?,
        }
        .checked()
    }

    fn checked(self) -> Result<Self, ConfigError> {
        if self.heartbeat_secs >= self.lease_secs {
            return Err(ConfigError {
                var: "CAVEA_HEARTBEAT_SECS",
                reason: "is invalid: must be less than CAVEA_LEASE_SECS".to_owned(),
            });
        }
        if self.node_id.is_empty() {
            return Err(ConfigError {
                var: "CAVEA_NODE_ID",
                reason: "is invalid: must not be empty".to_owned(),
            });
        }
        Ok(self)
    }
}

/// The host name: `HOSTNAME` if set (as in containers), else the kernel's.
fn hostname() -> Option<String> {
    let name = std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/proc/sys/kernel/hostname").ok())?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// Which browser origins may read the public endpoints' responses (08 §11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorsOrigins {
    /// `*`: every origin.
    Any,
    /// These origins only, lowercased, as a browser sends them in `Origin`.
    List(Vec<String>),
}

impl FromStr for CorsOrigins {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.trim() == "*" {
            return Ok(Self::Any);
        }
        let list = s
            .split(',')
            .map(|o| normalise_origin(o.trim()))
            .collect::<Option<Vec<_>>>()
            .filter(|l| !l.is_empty())
            .ok_or("must be * or a comma-separated list of origins such as https://tickets.example.com")?;
        Ok(Self::List(list))
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
