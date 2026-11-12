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
use crate::net::TrustedProxies;
use sqlx::ConnectOptions as _;
use sqlx::postgres::PgConnectOptions;

/// The highest `CAVEA_ASSET_MAX_BYTES`: an upload is held in memory whole
/// before it is stored.
const MAX_ASSET_BYTES: u32 = 256 << 20;

/// The highest `CAVEA_DB_WAIT_SECS`, an hour: the wait is bounded, and a
/// platform that should wait longer restarts the server instead.
const MAX_DB_WAIT_SECS: u32 = 3_600;

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
    /// `CAVEA_DB_WAIT_SECS`: how long the start waits for PostgreSQL to accept
    /// its first connection, at most [`MAX_DB_WAIT_SECS`]; 0 tries once.
    pub db_wait_secs: u32,
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
    /// `CAVEA_TRUSTED_PROXIES`: proxies whose `X-Forwarded-For` names the
    /// client (08 §12). None by default: the socket's peer is the client.
    pub trusted_proxies: TrustedProxies,
    /// `CAVEA_RATE_LIMIT_HOLDS`: hold-token creations, holds and releases per
    /// minute per publishable key and client address; 0 is unlimited.
    pub rate_limit_holds: u32,
    /// `CAVEA_RATE_LIMIT_READS`: other public requests per minute per
    /// publishable key and client address; 0 is unlimited.
    pub rate_limit_reads: u32,
    /// `CAVEA_ASSET_MAX_BYTES`: the largest image `POST /v1/assets` takes.
    pub asset_max_bytes: NonZeroU32,
    /// `CAVEA_PUBLIC_URL`: the server's external base URL, such as
    /// `https://seats.example.com`, without a trailing slash. The setup link
    /// starts with it.
    pub public_url: Option<PublicUrl>,
    /// `CAVEA_CONSOLE_COOKIE_SECURE`: whether the console session cookie is
    /// `Secure` and `__Host-` prefixed (the default). `false` only for a
    /// development server reached over plain HTTP from another host than
    /// `localhost`.
    pub console_cookie_secure: bool,
    /// `CAVEA_RATE_LIMIT_SIGN_IN`: console sign-in, setup and password-change
    /// attempts per minute per client address; 0 is unlimited.
    pub rate_limit_sign_in: u32,
    /// `CAVEA_LICENCE`: the licence file's path, or its contents (12 §7.3,
    /// `crate::licence`). Unset, the licence is the one uploaded through the
    /// console, if any.
    pub licence: Option<crate::licence::LicenceSource>,
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
            db_wait_secs: setting("CAVEA_DB_WAIT_SECS", || 30)?,
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
            trusted_proxies: setting("CAVEA_TRUSTED_PROXIES", TrustedProxies::default)?,
            rate_limit_holds: setting("CAVEA_RATE_LIMIT_HOLDS", || 120)?,
            rate_limit_reads: setting("CAVEA_RATE_LIMIT_READS", || 3_000)?,
            asset_max_bytes: setting("CAVEA_ASSET_MAX_BYTES", || {
                NonZeroU32::MIN.saturating_add((10 << 20) - 1)
            })?,
            public_url: optional("CAVEA_PUBLIC_URL")?,
            console_cookie_secure: setting("CAVEA_CONSOLE_COOKIE_SECURE", || true)?,
            rate_limit_sign_in: setting("CAVEA_RATE_LIMIT_SIGN_IN", || 10)?,
            licence: std::env::var_os("CAVEA_LICENCE")
                .and_then(crate::licence::LicenceSource::from_setting),
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
        if self.asset_max_bytes.get() > MAX_ASSET_BYTES {
            return Err(ConfigError {
                var: "CAVEA_ASSET_MAX_BYTES",
                reason: "is invalid: must be at most 268435456 (256 MiB)".to_owned(),
            });
        }
        if self.db_wait_secs > MAX_DB_WAIT_SECS {
            return Err(ConfigError {
                var: "CAVEA_DB_WAIT_SECS",
                reason: "is invalid: must be at most 3600 (an hour)".to_owned(),
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

/// A setting the server runs with but should not in production (12 §4,
/// SEC-006): logged once at start as a `warn` record with `setting`, never
/// the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Insecure {
    /// The variable, such as `CAVEA_CONSOLE_COOKIE_SECURE`.
    pub setting: &'static str,
    /// What it exposes, for the record's message.
    pub why: &'static str,
}

impl Config {
    /// Every insecure setting this configuration has. The development
    /// defaults (no `CAVEA_PUBLIC_URL`, or one on `localhost`) and a correct
    /// production set-up have none but an unset admin key, which turns
    /// endpoints off.
    #[must_use]
    pub fn insecure(&self) -> Vec<Insecure> {
        let mut found = Vec::new();
        let mut flag = |on: bool, setting: &'static str, why: &'static str| {
            if on {
                found.push(Insecure { setting, why });
            }
        };
        flag(
            self.public_url
                .as_ref()
                .is_some_and(|u| !u.is_https() && !u.is_loopback()),
            "CAVEA_PUBLIC_URL",
            "is not HTTPS: sessions, keys and hold tokens cross the network in clear; use TLS in production",
        );
        flag(
            !self.console_cookie_secure,
            "CAVEA_CONSOLE_COOKIE_SECURE",
            "is false: the console session cookie is sent over plain HTTP; only for a development server",
        );
        flag(
            self.admin_key.is_none(),
            "CAVEA_ADMIN_KEY",
            "is not set: /v1/workspaces and /metrics refuse every request",
        );
        flag(
            self.rate_limit_holds == 0,
            "CAVEA_RATE_LIMIT_HOLDS",
            "is 0: public holds are not rate limited, so a bot can hold the house (12 §3 T1)",
        );
        flag(
            self.rate_limit_reads == 0,
            "CAVEA_RATE_LIMIT_READS",
            "is 0: public reads and realtime connection attempts are not rate limited",
        );
        flag(
            self.rate_limit_sign_in == 0,
            "CAVEA_RATE_LIMIT_SIGN_IN",
            "is 0: console sign-in is not rate limited per address (12 §3 T9)",
        );
        flag(
            self.trusted_proxies.trusts_everyone(),
            "CAVEA_TRUSTED_PROXIES",
            "trusts every address: any client chooses the address its rate limits count against",
        );
        found
    }

    /// Log [`Self::insecure`]: one `warn` record each, `insecure setting`
    /// with `setting` and `why`. Called once, at start.
    pub fn warn_insecure(&self) {
        for found in self.insecure() {
            tracing::warn!(setting = found.setting, why = found.why, "insecure setting");
        }
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

/// `CAVEA_PUBLIC_URL`: `http://` or `https://`, a host, an optional port and
/// path, without spaces; kept without a trailing slash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicUrl(String);

impl PublicUrl {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether it is `https://` (12 §4: the server warns otherwise).
    #[must_use]
    pub fn is_https(&self) -> bool {
        self.0.starts_with("https://")
    }

    /// Whether its host is `localhost`, `127.0.0.1` or `[::1]`: a browser
    /// treats it as secure without TLS, and it is never reached from another
    /// machine.
    #[must_use]
    pub fn is_loopback(&self) -> bool {
        let rest = self.0.split_once("://").map_or("", |(_, r)| r);
        let authority = rest.split('/').next().unwrap_or_default();
        let host = match authority.strip_prefix('[') {
            Some(v6) => v6.split(']').next().unwrap_or_default(),
            None => authority.split(':').next().unwrap_or_default(),
        };
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    }
}

impl FromStr for PublicUrl {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim().trim_end_matches('/');
        let rest = s
            .strip_prefix("https://")
            .or_else(|| s.strip_prefix("http://"))
            .ok_or("must start with https:// or http://")?;
        let host = rest.split('/').next().unwrap_or_default();
        if host.is_empty() || s.len() > 2048 || !s.bytes().all(|b| b.is_ascii_graphic()) {
            return Err("must be a URL such as https://seats.example.com");
        }
        Ok(Self(s.to_owned()))
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
