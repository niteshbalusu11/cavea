//! Forward-only schema migrations, embedded in the binary and run at startup
//! under a PostgreSQL advisory lock (01 §6, 08 §5, §14).
//!
//! To add a migration, put `NNNN_name.sql` in `crates/cavea-server/migrations`
//! and append it to [`MIGRATIONS`]. A released migration is never edited: the
//! server refuses to start against a database whose applied migration differs
//! from the embedded one.

use std::borrow::Cow;
use std::fmt;

use sqlx::migrate::{MigrateError, Migration, MigrationType, Migrator};
use sqlx::postgres::{PgConnectOptions, PgConnection};
use sqlx::{Connection as _, SqlSafeStr as _};
use tracing::Instrument as _;

/// `(version, description, sql)`, in version order.
const MIGRATIONS: &[(i64, &str, &str)] = &[
    (
        1,
        "initial",
        include_str!("../../migrations/0001_initial.sql"),
    ),
    (
        2,
        "chart_versions_full_hash",
        include_str!("../../migrations/0002_chart_versions_full_hash.sql"),
    ),
    (
        3,
        "holds_session_sub",
        include_str!("../../migrations/0003_holds_session_sub.sql"),
    ),
    (
        4,
        "assets_per_workspace",
        include_str!("../../migrations/0004_assets_per_workspace.sql"),
    ),
    (
        5,
        "console_people",
        include_str!("../../migrations/0005_console_people.sql"),
    ),
    (
        6,
        "members_audit",
        include_str!("../../migrations/0006_members_audit.sql"),
    ),
    (
        7,
        "console_needs",
        include_str!("../../migrations/0007_console_needs.sql"),
    ),
];

/// Where applied migrations are recorded, with each one's SHA-384.
const TABLE: &str = "cavea_migrations";

#[derive(Debug)]
pub enum MigrationError {
    /// PostgreSQL could not be reached.
    Connect(sqlx::Error),
    /// The database has a migration this binary does not have: it was
    /// migrated by a newer `cavea-server`.
    Newer(i64),
    /// An applied migration's content differs from the embedded one.
    Changed(i64),
    Failed(MigrateError),
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "cannot connect to PostgreSQL: {e}"),
            Self::Newer(v) => write!(
                f,
                "the database has migration {v}, which this cavea-server does not know: it was \
                 migrated by a newer version; run that version or a later one"
            ),
            Self::Changed(v) => write!(
                f,
                "migration {v} applied to the database differs from the one in this \
                 cavea-server; the schema cannot be trusted, refusing to start"
            ),
            Self::Failed(e) => write!(f, "database migration failed: {e}"),
        }
    }
}

impl std::error::Error for MigrationError {}

/// Bring the database's schema up to date. Concurrent callers, in this
/// process or another, serialise on the advisory lock; all but the first find
/// nothing to do.
pub async fn migrate(options: &PgConnectOptions) -> Result<(), MigrationError> {
    // A connection of its own, closed afterwards: the advisory lock is held
    // for the session, so it must never go back to a pool still held.
    let mut conn = PgConnection::connect_with(options)
        .instrument(cavea_telemetry::db_span("connect"))
        .await
        .map_err(MigrationError::Connect)?;
    let before = applied(&mut conn).await;
    let mut migrator = Migrator::with_migrations(
        MIGRATIONS
            .iter()
            .map(|&(version, description, sql)| {
                Migration::new(
                    version,
                    Cow::Borrowed(description),
                    MigrationType::Simple,
                    sql.into_sql_str(),
                    false,
                )
            })
            .collect(),
    );
    migrator.dangerous_set_table_name(TABLE);
    migrator
        .run(&mut conn)
        .instrument(cavea_telemetry::db_span("migrate"))
        .await
        .map_err(|e| match e {
            MigrateError::VersionMissing(v) => MigrationError::Newer(v),
            MigrateError::VersionMismatch(v) => MigrationError::Changed(v),
            e => MigrationError::Failed(e),
        })?;
    let after = applied(&mut conn).await;
    let version = MIGRATIONS.last().map_or(0, |m| m.0);
    tracing::info!(
        version,
        applied = after.saturating_sub(before),
        "database schema up to date"
    );
    // Dropping the connection closes it too; a clean close is only politer.
    let _ = conn.close().await;
    Ok(())
}

/// How many migrations are recorded, for the log line only; 0 if unknown,
/// including before the table exists.
async fn applied(conn: &mut PgConnection) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM cavea_migrations")
        .fetch_one(conn)
        .instrument(cavea_telemetry::db_span("count_migrations"))
        .await
        .unwrap_or(0)
}
