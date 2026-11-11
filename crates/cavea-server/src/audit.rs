//! The audit log (09 §4.2, §7; 12 §3 T9; 08 §5 `audit_log`): one row per
//! sensitive action that is not a seat command, written by [`record`] inside
//! the transaction that makes the change, so a change that rolls back leaves
//! no row and a committed one always has its row. Seat commands are not
//! repeated here: the event log is their record, with the console user as
//! `actor` (08 §11), so the hold path writes nothing more.
//!
//! A row holds identifiers only: the actor (`usr_…`, `key_…`, `admin`, or
//! `anonymous` for a failed sign-in), the workspace, the user it concerns, a
//! target (`usr_…`, `key_…`, `inv_…`, `ws_…`, or a chart's or event's key),
//! small facts such as a role or a version number, and the request's id.
//! Never a secret, a password, a request body, or an email: a failed sign-in
//! for an address that is no account's records nothing of the address.

use serde_json::Value;
use sqlx::PgConnection;

use crate::http::Auth;

/// Every action recorded (`audit_log.action`).
pub const ACTIONS: [&str; 17] = [
    "console.setup",
    "console.sign_in",
    "console.sign_in_failed",
    "console.sign_out",
    "console.password_changed",
    "member.invited",
    "member.joined",
    "member.role_changed",
    "member.removed",
    "invitation.revoked",
    "workspace.renamed",
    "api_key.created",
    "api_key.revoked",
    "chart.published",
    "chart.archived",
    "event.created",
    "event.archived",
];

/// One row, before it is written.
#[derive(Debug)]
pub struct Entry<'a> {
    /// The workspace (a UUID as 32 hex digits), or `None` for the instance:
    /// setup, sign-in, sign-out, a password change.
    pub workspace: Option<&'a str>,
    pub actor: &'a str,
    /// The console user the row concerns (32 hex digits): the one acting, or
    /// the account a failed sign-in named.
    pub user: Option<&'a str>,
    pub action: &'static str,
    pub target: Option<&'a str>,
    pub detail: Option<Value>,
}

impl<'a> Entry<'a> {
    /// An action of an authenticated caller in its workspace.
    pub fn by(auth: &'a Auth, actor: &'a str, action: &'static str) -> Self {
        Self {
            workspace: Some(auth.workspace().uuid()),
            actor,
            user: auth.user(),
            action,
            target: None,
            detail: None,
        }
    }

    /// An action of the instance's console: setup, sign-in, sign-out, a
    /// password change, by `user` (32 hex digits) acting as `actor`.
    #[must_use]
    pub fn console(actor: &'a str, user: &'a str, action: &'static str) -> Self {
        Self {
            workspace: None,
            actor,
            user: Some(user),
            action,
            target: None,
            detail: None,
        }
    }

    #[must_use]
    pub fn target(mut self, target: &'a str) -> Self {
        self.target = Some(target);
        self
    }

    #[must_use]
    pub fn detail(mut self, detail: Value) -> Self {
        self.detail = Some(detail);
        self
    }
}

const INSERT: &str = "INSERT INTO audit_log
  (workspace_id, actor, user_id, action, target, detail, request_id)
VALUES ($1::uuid, $2, $3::uuid, $4, $5, $6::jsonb, $7)";

/// Write `entry` on `tx`, the transaction of the change it records (or, for
/// a failed sign-in, which changes nothing, any connection).
pub async fn record(tx: &mut PgConnection, entry: Entry<'_>) -> Result<(), sqlx::Error> {
    let request = cavea_telemetry::request_id();
    sqlx::query(INSERT)
        .bind(entry.workspace)
        .bind(entry.actor)
        .bind(entry.user)
        .bind(entry.action)
        .bind(entry.target)
        .bind(entry.detail.map(|d| d.to_string()))
        .bind(request.as_ref().map(|r| r.as_str()))
        .execute(tx)
        .await?;
    Ok(())
}
