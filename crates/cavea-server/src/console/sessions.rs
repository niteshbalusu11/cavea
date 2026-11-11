//! Console sessions in `console_sessions` (08 §5): one row per sign-in, keyed
//! by the SHA-256 of its identifier (never the identifier). A session ends
//! [`IDLE`] after its last use, renewed at most every [`RENEW_EVERY`], and in
//! any case [`ABSOLUTE`] after it began.

use sqlx::{PgConnection, PgPool};
use tracing::Instrument as _;

use super::{ABSOLUTE, IDLE, RENEW_EVERY, Role};

fn secs(d: std::time::Duration) -> i64 {
    i64::try_from(d.as_secs()).unwrap_or(i64::MAX)
}

const INSERT: &str = "INSERT INTO console_sessions (id_hash, user_id, expires_at)
VALUES ($1, $2::uuid, now() + $3 * interval '1 second')";
/// A user's sessions that have ended, removed whenever they sign in.
const SWEEP: &str = "DELETE FROM console_sessions WHERE user_id = $1::uuid
   AND (expires_at <= now() OR created_at <= now() - $2 * interval '1 second')";

/// Start a session for `user` (32 hex digits), and remove the user's ended
/// ones, inside the caller's transaction.
pub async fn insert(
    tx: &mut PgConnection,
    id_hash: &[u8; 32],
    user: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(SWEEP)
        .bind(user)
        .bind(secs(ABSOLUTE))
        .execute(&mut *tx)
        .await?;
    sqlx::query(INSERT)
        .bind(id_hash.as_slice())
        .bind(user)
        .bind(secs(IDLE))
        .execute(&mut *tx)
        .await?;
    Ok(())
}

/// A live session, as a request finds it.
#[derive(Debug, Clone)]
pub struct Found {
    /// The user's UUID as 32 hex digits.
    pub user: String,
    /// The user's role in the workspace asked about, if a member.
    pub role: Option<Role>,
    /// RFC 3339: when the session ends unless used again.
    pub expires_at: String,
    /// RFC 3339: when it ends however it is used.
    pub absolute_expires_at: String,
}

const FIND: &str = "WITH s AS (
  SELECT s.id_hash, s.user_id, s.created_at, s.expires_at
    FROM console_sessions s JOIN users u ON u.id = s.user_id
   WHERE s.id_hash = $1 AND s.expires_at > now()
     AND s.created_at > now() - $3 * interval '1 second'
     AND u.disabled_at IS NULL
), renewed AS (
  UPDATE console_sessions c
     SET expires_at = least(now() + $2 * interval '1 second', c.created_at + $3 * interval '1 second')
    FROM s
   WHERE c.id_hash = s.id_hash
     AND c.expires_at < now() + ($2 - $4) * interval '1 second'
  RETURNING c.expires_at
)
SELECT replace(s.user_id::text, '-', ''),
       (SELECT m.role FROM memberships m WHERE m.user_id = s.user_id AND m.workspace_id = $5::uuid),
       to_char(coalesce((SELECT expires_at FROM renewed), s.expires_at) AT TIME ZONE 'UTC',
               'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"'),
       to_char((s.created_at + $3 * interval '1 second') AT TIME ZONE 'UTC',
               'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')
  FROM s";

/// The live session with this identifier hash, renewed if due, with the
/// user's role in `workspace` (a UUID as 32 hex digits) when one is named.
/// `None` for an unknown, ended or disabled user's session.
pub async fn find(
    db: &PgPool,
    id_hash: &[u8; 32],
    workspace: Option<&str>,
) -> Result<Option<Found>, sqlx::Error> {
    let row: Option<(String, Option<String>, String, String)> = sqlx::query_as(FIND)
        .bind(id_hash.as_slice())
        .bind(secs(IDLE))
        .bind(secs(ABSOLUTE))
        .bind(secs(RENEW_EVERY))
        .bind(workspace)
        .fetch_optional(db)
        .instrument(cavea_telemetry::db_span("find_console_session"))
        .await?;
    Ok(
        row.map(|(user, role, expires_at, absolute_expires_at)| Found {
            user,
            role: role.as_deref().and_then(Role::from_db),
            expires_at,
            absolute_expires_at,
        }),
    )
}

const REVOKE: &str = "DELETE FROM console_sessions WHERE id_hash = $1";
const REVOKE_ALL: &str = "DELETE FROM console_sessions WHERE user_id = $1::uuid";

/// End one session: sign-out, or the old session at sign-in (rotation).
pub async fn revoke(db: &PgPool, id_hash: &[u8; 32]) -> Result<(), sqlx::Error> {
    sqlx::query(REVOKE)
        .bind(id_hash.as_slice())
        .execute(db)
        .instrument(cavea_telemetry::db_span("revoke_console_session"))
        .await?;
    Ok(())
}

/// End every session of `user`, inside the caller's transaction (a password
/// change).
pub async fn revoke_all(tx: &mut PgConnection, user: &str) -> Result<(), sqlx::Error> {
    sqlx::query(REVOKE_ALL).bind(user).execute(tx).await?;
    Ok(())
}
