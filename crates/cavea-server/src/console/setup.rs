//! First-run setup (09 §2, CON-002): while no user exists, each start makes a
//! one-time token, stores its SHA-256 in `console_setup` (one row), and logs
//! a link holding it; `POST /console/api/setup` with the token creates the
//! owner and burns it.
//!
//! The link is the one secret the server ever logs (12 §4), on purpose: it
//! reaches only whoever reads the log of a server that has no owner yet, it
//! works once, for [`TTL`], and is useless once an owner exists. A server
//! with a user logs nothing and stores no token.

use std::time::Duration;

use sha2::{Digest as _, Sha256};
use sqlx::PgPool;
use subtle::ConstantTimeEq as _;
use tracing::Instrument as _;

/// How long a setup token lasts. A restart makes a new one.
pub const TTL: Duration = Duration::from_secs(3600);

/// The console path the link opens; the token is in the fragment, which a
/// browser never sends to a server or puts in a `Referer`.
pub const PATH: &str = "/console/setup";

/// A setup token: 64 lowercase hex digits (256 bits). Never logged but in
/// the setup link.
pub fn token() -> Result<String, getrandom::Error> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// Replace any earlier token, unless a user exists. Of two nodes starting at
/// once, the link of the later one works.
const PREPARE: &str = "
INSERT INTO console_setup (id, token_hash, expires_at, used_at)
SELECT 1, $1, now() + $2 * interval '1 second', NULL
 WHERE NOT EXISTS (SELECT FROM users)
ON CONFLICT (id) DO UPDATE
   SET token_hash = excluded.token_hash, expires_at = excluded.expires_at, used_at = NULL
RETURNING true";

/// At start-up: if no user exists, store a new token and return the setup
/// link to log, `{public_url}/console/setup#token=…`, or the path alone when
/// `CAVEA_PUBLIC_URL` is not set. `None` once an owner exists.
pub async fn prepare(db: &PgPool, public_url: Option<&str>) -> Result<Option<String>, sqlx::Error> {
    let token = token().map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    let stored: Option<bool> = sqlx::query_scalar(PREPARE)
        .bind(token_hash(&token).as_slice())
        .bind(i64::try_from(TTL.as_secs()).unwrap_or(i64::MAX))
        .fetch_optional(db)
        .instrument(cavea_telemetry::db_span("prepare_setup"))
        .await?;
    Ok(stored.map(|_| {
        let base = public_url.unwrap_or_default().trim_end_matches('/');
        format!("{base}{PATH}#token={token}")
    }))
}

/// Log the setup link: one `info` record, the deliberate exception to "never
/// log secrets" (12 §4). Without `CAVEA_PUBLIC_URL` the link is a path, to
/// open on the server's own address.
pub fn announce(link: &str, absolute: bool) {
    let expires_in_minutes = TTL.as_secs() / 60;
    if absolute {
        tracing::info!(
            setup_link = link,
            expires_in_minutes,
            "no console user yet: open the setup link to create the owner"
        );
    } else {
        tracing::info!(
            setup_link = link,
            expires_in_minutes,
            "no console user yet: open the setup link on this server's address (CAVEA_PUBLIC_URL is not set) to create the owner"
        );
    }
}

const VALID: &str = "SELECT token_hash FROM console_setup
 WHERE id = 1 AND expires_at > now() AND used_at IS NULL AND NOT EXISTS (SELECT FROM users)";

/// Whether `token` is the live setup token: checked before the password is
/// hashed, so that a wrong token costs a query, not argon2's work.
/// [`complete`] checks again under the lock.
pub async fn valid(db: &PgPool, token: &str) -> Result<bool, sqlx::Error> {
    let stored: Option<Vec<u8>> = sqlx::query_scalar(VALID)
        .fetch_optional(db)
        .instrument(cavea_telemetry::db_span("check_setup"))
        .await?;
    Ok(stored.is_some_and(|stored| token_hash(token).ct_eq(stored.as_slice()).into()))
}

/// Why setup was refused. Every case answers the same (`401`, `setup_token`):
/// a wrong token, an expired or used one, and a server that has an owner
/// look alike.
#[derive(Debug)]
pub enum SetupError {
    Refused,
    /// The workspace key named for the first workspace is taken.
    WorkspaceTaken,
    Db(sqlx::Error),
}

impl From<sqlx::Error> for SetupError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

/// What a completed setup made.
#[derive(Debug)]
pub struct Owner {
    /// The user's UUID as 32 hex digits.
    pub user: String,
}

const LOCK: &str = "SELECT token_hash, expires_at > now() AND used_at IS NULL
  FROM console_setup WHERE id = 1 FOR UPDATE";
const NO_USERS: &str = "SELECT NOT EXISTS (SELECT FROM users)";
const CREATE_USER: &str = "INSERT INTO users (id, email, password_hash)
VALUES (gen_random_uuid(), $1, $2) RETURNING replace(id::text, '-', '')";
const ANY_WORKSPACE: &str = "SELECT EXISTS (SELECT FROM workspaces)";
const CREATE_WORKSPACE: &str =
    "INSERT INTO workspaces (id, key, name) VALUES (gen_random_uuid(), $1, $2)";
const OWN_ALL: &str = "INSERT INTO memberships (user_id, workspace_id, role)
SELECT $1::uuid, id, 'owner' FROM workspaces";
const BURN: &str = "UPDATE console_setup SET used_at = now() WHERE id = 1";

/// Create the owner with `token`, in one transaction under the setup row's
/// lock, so that of two setups at once exactly one creates a user: the
/// second finds the token used. The owner becomes owner of every workspace
/// (created earlier with the admin key); with none, `workspace` (key, name)
/// is created first. `password_hash` is already argon2id. Then the token is
/// burnt, and `session` (an identifier's SHA-256) signs the owner in.
pub async fn complete(
    db: &PgPool,
    token: &str,
    email: &str,
    password_hash: &str,
    workspace: (&str, &str),
    session: &[u8; 32],
) -> Result<Owner, SetupError> {
    let work = async {
        let mut tx = db.begin().await?;
        let row: Option<(Vec<u8>, bool)> = sqlx::query_as(LOCK).fetch_optional(&mut *tx).await?;
        let Some((stored, live)) = row else {
            return Err(SetupError::Refused);
        };
        let matches: bool = token_hash(token).ct_eq(stored.as_slice()).into();
        let no_users: bool = sqlx::query_scalar(NO_USERS).fetch_one(&mut *tx).await?;
        if !(matches && live && no_users) {
            return Err(SetupError::Refused);
        }
        let user: String = sqlx::query_scalar(CREATE_USER)
            .bind(email)
            .bind(password_hash)
            .fetch_one(&mut *tx)
            .await?;
        let any: bool = sqlx::query_scalar(ANY_WORKSPACE)
            .fetch_one(&mut *tx)
            .await?;
        if !any {
            let created = sqlx::query(CREATE_WORKSPACE)
                .bind(workspace.0)
                .bind(workspace.1)
                .execute(&mut *tx)
                .await;
            if let Err(sqlx::Error::Database(e)) = &created
                && e.is_unique_violation()
            {
                return Err(SetupError::WorkspaceTaken);
            }
            created?;
        }
        sqlx::query(OWN_ALL).bind(&user).execute(&mut *tx).await?;
        sqlx::query(BURN).execute(&mut *tx).await?;
        super::sessions::insert(&mut tx, session, &user).await?;
        tx.commit().await?;
        Ok(Owner { user })
    };
    work.instrument(cavea_telemetry::db_span("complete_setup"))
        .await
}
