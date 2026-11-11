//! A workspace's members (09 §7, §10; CON-012): list them, change a role,
//! remove one. Every route takes [`Member`]: the console session of an owner
//! or admin naming the workspace in `Cavea-Workspace` (`settings`); making
//! or unmaking an owner also needs `ownership`, which only owners have (09
//! §2: "Admin: all except ownership transfer").
//!
//! Invariants, whatever the order of requests: a workspace keeps at least
//! one owner (changes to a workspace's members are serialised on its row);
//! a role change or a removal applies to the member's next request, since
//! the role is read with each (`staff::member`), and ends nothing else of
//! theirs; a user removed from every workspace can still sign in.

use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::Instrument as _;

use super::auth::Auth;
use super::extract::{Json, Page, Path, Query, invalid_field};
use super::openapi::{Needs, Op, Routes};
use super::staff::Member;
use super::{ApiError, AppState, Code, List};
use crate::console::{Permission, Role};

pub(super) fn routes() -> Routes {
    let settings = |op: Op| op.needs(Needs::Member(Permission::Settings));
    Routes::new()
        .add(
            settings(Op::get(
                "/console/api/members",
                "listMembers",
                "List the workspace's members",
            ))
            .about("Ordered by user id.")
            .page()
            .reply(200, "MemberList", "A page of members"),
            list,
        )
        .add(
            settings(Op::patch(
                "/console/api/members/{user}",
                "changeMemberRole",
                "Change a member's role",
            ))
            .about(
                "Applies to the member's next request. Making or unmaking an owner needs \
`ownership`; the last owner cannot be changed (`400`, `last_owner`).",
            )
            .body("ChangeRole")
            .reply(200, "WorkspaceMember", "The member"),
            change_role,
        )
        .add(
            settings(Op::delete(
                "/console/api/members/{user}",
                "removeMember",
                "Remove a member",
            ))
            .about(
                "Applies to the member's next request; the user and their other workspaces \
stay. Removing an owner needs `ownership`; the last owner cannot be removed.",
            )
            .response(204, "Removed", None),
            remove,
        )
}

/// `usr_` and 32 hex digits, as 32 hex digits; else `404`.
fn user_uuid(id: &str) -> Result<&str, ApiError> {
    id.strip_prefix("usr_")
        .filter(|hex| {
            hex.len() == 32 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
        .ok_or_else(|| ApiError::new(Code::NotFound).with_detail("missing", "member"))
}

fn query_refused(what: &'static str) -> ApiError {
    ApiError::new(Code::InvalidRequest)
        .with_message(what)
        .with_detail("reason", "query")
}

fn ownership() -> ApiError {
    ApiError::new(Code::Forbidden)
        .with_message("Only an owner can make or unmake an owner.")
        .with_detail("reason", "role")
        .with_detail("permission", Permission::Ownership.as_str())
}

fn last_owner() -> ApiError {
    ApiError::new(Code::InvalidRequest)
        .with_message("A workspace keeps at least one owner.")
        .with_detail("reason", "last_owner")
}

#[derive(Debug, Serialize)]
struct MemberView {
    user: Value,
    role: &'static str,
}

impl MemberView {
    fn new(user: &str, email: String, role: Role) -> Self {
        Self {
            user: json!({ "id": format!("usr_{user}"), "email": email }),
            role: role.as_str(),
        }
    }
}

const LIST: &str = "SELECT replace(u.id::text, '-', ''), u.email, m.role
  FROM memberships m JOIN users u ON u.id = m.user_id
 WHERE m.workspace_id = $1::uuid AND ($2::uuid IS NULL OR m.user_id > $2::uuid)
 ORDER BY m.user_id
 LIMIT $3";

async fn list(
    Member(auth): Member,
    Query(page): Query<Page>,
    State(state): State<AppState>,
) -> Result<axum::Json<List<MemberView>>, ApiError> {
    let limit = page.limit()?;
    let after = match page.after.as_deref() {
        None => None,
        Some(after) => {
            Some(user_uuid(after).map_err(|_| query_refused("`after` is not a user id."))?)
        }
    };
    let rows: Vec<(String, String, String)> = sqlx::query_as(LIST)
        .bind(auth.workspace().uuid())
        .bind(after)
        .bind(i64::from(limit) + 1)
        .fetch_all(&state.db)
        .instrument(cavea_telemetry::db_span("list_members"))
        .await?;
    let members = rows
        .into_iter()
        .filter_map(|(user, email, role)| {
            Some(MemberView::new(&user, email, Role::from_db(&role)?))
        })
        .collect();
    Ok(axum::Json(List::new(members, limit, |m| {
        m.user["id"].as_str().unwrap_or_default().to_owned()
    })))
}

/// Serialise every change to the workspace's members on its row.
const LOCK_WORKSPACE: &str = "SELECT FROM workspaces WHERE id = $1::uuid FOR UPDATE";
const ROLE_OF: &str = "SELECT m.role, u.email FROM memberships m JOIN users u ON u.id = m.user_id
 WHERE m.workspace_id = $1::uuid AND m.user_id = $2::uuid";
const OWNERS: &str =
    "SELECT count(*) FROM memberships WHERE workspace_id = $1::uuid AND role = 'owner'";
const SET_ROLE: &str =
    "UPDATE memberships SET role = $3 WHERE workspace_id = $1::uuid AND user_id = $2::uuid";
const REMOVE: &str = "DELETE FROM memberships WHERE workspace_id = $1::uuid AND user_id = $2::uuid";

/// The checks of a change to `user`'s membership, under the workspace's
/// lock: the member's current role and email, `404` if not a member;
/// `ownership` when the change makes or unmakes an owner; `last_owner` when
/// it would leave none.
async fn checked_change(
    tx: &mut sqlx::PgConnection,
    auth: &Auth,
    user: &str,
    new: Option<Role>,
) -> Result<(Role, String), ApiError> {
    let ws = auth.workspace().uuid();
    sqlx::query(LOCK_WORKSPACE)
        .bind(ws)
        .execute(&mut *tx)
        .await?;
    let row: Option<(String, String)> = sqlx::query_as(ROLE_OF)
        .bind(ws)
        .bind(user)
        .fetch_optional(&mut *tx)
        .await?;
    let (old, email) = row
        .and_then(|(role, email)| Some((Role::from_db(&role)?, email)))
        .ok_or_else(|| ApiError::new(Code::NotFound).with_detail("missing", "member"))?;
    let touches_owner = old == Role::Owner || new == Some(Role::Owner);
    if touches_owner && !auth.role().is_some_and(|r| r.allows(Permission::Ownership)) {
        return Err(ownership());
    }
    if old == Role::Owner && new != Some(Role::Owner) {
        let owners: i64 = sqlx::query_scalar(OWNERS)
            .bind(ws)
            .fetch_one(&mut *tx)
            .await?;
        if owners <= 1 {
            return Err(last_owner());
        }
    }
    Ok((old, email))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeRole {
    role: String,
}

fn role_field(role: &str) -> Result<Role, ApiError> {
    Role::from_db(role)
        .ok_or_else(|| invalid_field("role", "must be owner, admin, editor, operator or viewer"))
}

async fn change_role(
    Member(auth): Member,
    Path(user): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<ChangeRole>,
) -> Result<axum::Json<MemberView>, ApiError> {
    let user = user_uuid(&user)?;
    let role = role_field(&body.role)?;
    let actor = auth.actor();
    let target = format!("usr_{user}");
    let work = async {
        let mut tx = state.db.begin().await?;
        let (old, email) = checked_change(&mut tx, &auth, user, Some(role)).await?;
        if old != role {
            sqlx::query(SET_ROLE)
                .bind(auth.workspace().uuid())
                .bind(user)
                .bind(role.as_str())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok::<_, ApiError>(email)
    };
    let email = work
        .instrument(cavea_telemetry::db_span("change_member_role"))
        .await?;
    tracing::info!(member = %target, role = role.as_str(), by = %actor, "member role changed");
    Ok(axum::Json(MemberView::new(user, email, role)))
}

async fn remove(
    Member(auth): Member,
    Path(user): Path<String>,
    State(state): State<AppState>,
) -> Result<StatusCode, ApiError> {
    let user = user_uuid(&user)?;
    let actor = auth.actor();
    let target = format!("usr_{user}");
    let work = async {
        let mut tx = state.db.begin().await?;
        checked_change(&mut tx, &auth, user, None).await?;
        sqlx::query(REMOVE)
            .bind(auth.workspace().uuid())
            .bind(user)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok::<_, ApiError>(())
    };
    work.instrument(cavea_telemetry::db_span("remove_member"))
        .await?;
    tracing::info!(member = %target, by = %actor, "member removed");
    Ok(StatusCode::NO_CONTENT)
}
