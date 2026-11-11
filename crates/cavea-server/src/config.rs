//! Settings from `CAVEA_`-prefixed environment variables (08 §13). Every
//! setting has a safe default; a malformed value is an error, never ignored.

use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;

use cavea_telemetry::LogFormat;

#[derive(Debug, Clone)]
pub struct Config {
    /// `CAVEA_LISTEN`: the socket address to bind.
    pub listen: SocketAddr,
    /// `CAVEA_LOG`: a `tracing` filter such as `info,cavea_server=debug`.
    /// Checked when logging starts.
    pub log: String,
    /// `CAVEA_LOG_FORMAT`: `json` (production) or `text` (development).
    pub log_format: LogFormat,
}

impl Config {
    /// Read every setting. Add a new one here, with its default, and to 08 §13.
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            listen: setting("CAVEA_LISTEN", || SocketAddr::from(([0, 0, 0, 0], 8080)))?,
            log: setting("CAVEA_LOG", || "info".to_owned())?,
            log_format: setting("CAVEA_LOG_FORMAT", LogFormat::default)?,
        })
    }
}

/// A setting that is set but cannot be used. The value is not repeated: some
/// settings are secrets.
#[derive(Debug)]
pub struct ConfigError {
    pub var: &'static str,
    pub reason: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} is invalid: {}", self.var, self.reason)
    }
}

impl std::error::Error for ConfigError {}

fn setting<T>(var: &'static str, default: impl FnOnce() -> T) -> Result<T, ConfigError>
where
    T: FromStr<Err: fmt::Display>,
{
    let Some(raw) = std::env::var_os(var) else {
        return Ok(default());
    };
    let error = |reason: String| ConfigError { var, reason };
    raw.to_str()
        .ok_or_else(|| error("not valid UTF-8".to_owned()))?
        .parse()
        .map_err(|e: T::Err| error(e.to_string()))
}
