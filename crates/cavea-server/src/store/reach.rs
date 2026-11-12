//! Reaching PostgreSQL at start (08 §16, D-98). The first connection is
//! tried again, for up to `CAVEA_DB_WAIT_SECS`, while it fails the way a
//! database that is not up yet fails; a wrong password or database name fails
//! at once. A failure is named in words of our own and a SQLSTATE, never the
//! driver's or the operating system's text, so no log line or error can quote
//! the connection URL or its password.

use std::fmt;
use std::io::ErrorKind;
use std::time::Duration;

use sqlx::Connection as _;
use sqlx::postgres::{PgConnectOptions, PgConnection};
use tokio::time::Instant;
use tracing::Instrument as _;

/// The longest one attempt may take: a firewall that drops packets would
/// otherwise hold it for the operating system's TCP timeout, minutes. Within
/// a wait, an attempt also ends when the wait does, but is given at least
/// [`MIN_ATTEMPT`], so the start ends at most that long after the wait.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(5);
const MIN_ATTEMPT: Duration = Duration::from_secs(1);
/// The pause after the first failed attempt, doubled after each until
/// [`MAX_PAUSE`]: about fifteen attempts, and as many log records, in 30 s.
const FIRST_PAUSE: Duration = Duration::from_millis(250);
const MAX_PAUSE: Duration = Duration::from_secs(2);

/// Why a connection to PostgreSQL failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unreachable {
    /// Nothing listens at the address: not started yet, or the wrong address.
    Refused,
    /// The connection closed during its start-up exchange. A published
    /// container port with nothing behind it yet accepts and closes at once,
    /// which sqlx's TLS request reports as a protocol error of its own.
    Closed,
    /// No answer within the attempt's time ([`ATTEMPT_TIMEOUT`]).
    TimedOut,
    /// Any other network failure: a host name that does not resolve (yet), no
    /// route to the host.
    Network,
    /// PostgreSQL is starting up, recovering or shutting down (`57P01` to
    /// `57P03`).
    Starting,
    /// PostgreSQL's `max_connections` is reached (`53300`), as while the
    /// process this one replaces still holds its pool.
    TooManyConnections,
    /// Authentication failed: the user name or password, or `pg_hba.conf`.
    Authentication(Sqlstate),
    /// The database named in the URL does not exist (`3D000`).
    NoDatabase,
    /// Any other refusal by PostgreSQL.
    Refusal(Sqlstate),
    /// The TLS handshake failed.
    Tls,
    /// The start-up exchange failed otherwise: PostgreSQL asked for an
    /// authentication method this client does not support, or what answered
    /// is not PostgreSQL.
    Unsupported,
}

impl Unreachable {
    #[must_use]
    pub fn of(error: &sqlx::Error) -> Self {
        match error {
            sqlx::Error::Io(e) => match e.kind() {
                ErrorKind::ConnectionRefused => Self::Refused,
                ErrorKind::ConnectionReset
                | ErrorKind::ConnectionAborted
                | ErrorKind::UnexpectedEof
                | ErrorKind::BrokenPipe => Self::Closed,
                ErrorKind::TimedOut => Self::TimedOut,
                _ => Self::Network,
            },
            sqlx::Error::Database(e) => {
                let code = Sqlstate::of(e.code().as_deref());
                match code.as_str() {
                    "57P01" | "57P02" | "57P03" => Self::Starting,
                    "53300" => Self::TooManyConnections,
                    "28000" | "28P01" => Self::Authentication(code),
                    "3D000" => Self::NoDatabase,
                    _ => Self::Refusal(code),
                }
            }
            sqlx::Error::Tls(_) => Self::Tls,
            // The one byte answering sqlx's TLS request, read at the end of
            // the stream: the connection closed (`sslmode=prefer`, the default).
            // Read for this alone, never printed.
            sqlx::Error::Protocol(text)
                if text.starts_with("unexpected response from SSLRequest: 0x00") =>
            {
                Self::Closed
            }
            _ => Self::Unsupported,
        }
    }

    /// Whether a database that is still starting, or restarting, fails this
    /// way, so that trying again can succeed.
    #[must_use]
    pub fn passes(self) -> bool {
        matches!(
            self,
            Self::Refused
                | Self::Closed
                | Self::TimedOut
                | Self::Network
                | Self::Starting
                | Self::TooManyConnections
        )
    }
}

impl fmt::Display for Unreachable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused => f.write_str("connection refused: nothing listens at the address yet"),
            Self::Closed => f.write_str("the connection closed while it was being opened"),
            Self::TimedOut => f.write_str("no answer in time"),
            Self::Network => f.write_str(
                "network error: the host name does not resolve or the host cannot be reached",
            ),
            Self::Starting => f.write_str("PostgreSQL is starting up or shutting down"),
            Self::TooManyConnections => {
                f.write_str("PostgreSQL has no connection to spare (max_connections)")
            }
            Self::Authentication(code) => write!(
                f,
                "PostgreSQL refused the user name or password in CAVEA_DATABASE_URL, or \
                 pg_hba.conf refused this client (SQLSTATE {code})"
            ),
            Self::NoDatabase => f.write_str(
                "the database named in CAVEA_DATABASE_URL does not exist (SQLSTATE 3D000)",
            ),
            Self::Refusal(code) => write!(f, "PostgreSQL refused the connection (SQLSTATE {code})"),
            Self::Tls => f.write_str("the TLS handshake failed"),
            Self::Unsupported => f.write_str(
                "the start-up exchange failed: PostgreSQL asked for an authentication method \
                 this server cannot use, or what answered is not PostgreSQL",
            ),
        }
    }
}

/// A five-character SQLSTATE, or `?????` when the server sent none or
/// something else: it is printed, so nothing else of the reply gets through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sqlstate([u8; 5]);

impl Sqlstate {
    fn of(code: Option<&str>) -> Self {
        match code.map(str::as_bytes) {
            Some(&[a, b, c, d, e]) if [a, b, c, d, e].iter().all(u8::is_ascii_alphanumeric) => {
                Self([a, b, c, d, e])
            }
            _ => Self(*b"?????"),
        }
    }

    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap_or("?????")
    }
}

impl fmt::Display for Sqlstate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The first connection, given up on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GaveUp {
    /// The last attempt's failure.
    pub reason: Unreachable,
    pub attempts: u32,
    /// From the first attempt to the last one's failure.
    pub waited: Duration,
}

impl fmt::Display for GaveUp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.attempts > 1 {
            write!(
                f,
                "cannot connect to PostgreSQL (CAVEA_DATABASE_URL) after {} attempts in {} s, \
                 the most CAVEA_DB_WAIT_SECS allows: {}",
                self.attempts,
                self.waited.as_secs(),
                self.reason
            )
        } else {
            write!(
                f,
                "cannot connect to PostgreSQL (CAVEA_DATABASE_URL): {}",
                self.reason
            )
        }
    }
}

/// Open one connection, trying again for up to `wait` while the failure is
/// one a database that is not up yet gives ([`Unreachable::passes`]). Each
/// failure that will be tried again is logged once, at `warn`, with its
/// reason; `wait` zero tries once, for up to [`ATTEMPT_TIMEOUT`].
pub async fn connect(options: &PgConnectOptions, wait: Duration) -> Result<PgConnection, GaveUp> {
    let start = Instant::now();
    let mut pause = FIRST_PAUSE;
    let mut attempts = 0;
    loop {
        attempts += 1;
        let time = if wait.is_zero() {
            ATTEMPT_TIMEOUT
        } else {
            wait.saturating_sub(start.elapsed())
                .clamp(MIN_ATTEMPT, ATTEMPT_TIMEOUT)
        };
        let attempt =
            PgConnection::connect_with(options).instrument(cavea_telemetry::db_span("connect"));
        let reason = match tokio::time::timeout(time, attempt).await {
            Ok(Ok(connection)) => return Ok(connection),
            Ok(Err(error)) => Unreachable::of(&error),
            Err(_) => Unreachable::TimedOut,
        };
        let waited = start.elapsed();
        if !reason.passes() || waited >= wait {
            return Err(GaveUp {
                reason,
                attempts,
                waited,
            });
        }
        // The last attempt is made when the wait runs out.
        let pause_now = pause.min(wait - waited);
        tracing::warn!(
            %reason,
            attempts,
            waited_s = waited.as_secs(),
            wait_s = wait.as_secs(),
            "PostgreSQL is not reachable yet; trying again until CAVEA_DB_WAIT_SECS runs out"
        );
        tokio::time::sleep(pause_now).await;
        pause = (pause * 2).min(MAX_PAUSE);
    }
}
