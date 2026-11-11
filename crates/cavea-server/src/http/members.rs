//! A workspace's people (09 §7, §10; CON-012, CON-013): its members and
//! their roles, invitations by a one-time link, its name, and the audit log.
//! Every route but acceptance takes [`Member`]: the console session of an
//! owner or admin naming the workspace in `Cavea-Workspace` (`settings`);
//! making or unmaking an owner also needs `ownership`, which only owners
//! have (09 §2: "Admin: all except ownership transfer").
//!
//! Invariants, whatever the order of requests: a workspace keeps at least
//! one owner (changes to a workspace's members are serialised on its row);
//! a role change or a removal applies to the member's next request, since
//! the role is read with each (`staff::member`), and ends nothing else of
//! theirs; a user removed from every workspace can still sign in.
//!
//! An invitation is a link, `{CAVEA_PUBLIC_URL}/console/invite#token=…`,
//! returned once to whoever created it and never logged or stored (only the
//! token's SHA-256 is). It names an email and a role, works once, for
//! [`INVITATION_TTL`], and is accepted either by a signed-in user with that
//! email or, for an email with no account, by setting a password.

use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;
use tracing::Instrument as _;

use super::auth::{Auth, ClientIp};
use super::console::{budget, busy, me_json, new_session, signed_in};
use super::extract::{Json, Page, Path, Query, check_text, invalid_field};
use super::openapi::{Needs, Op, Routes};
use super::staff::{Member, cookie_session, session_cookie};
use super::{ApiError, AppState, Code, List};
use crate::audit::{self, Entry};
use crate::console::password;
use crate::console::{Permission, Role, check_password, normalise_email, sessions};

/// How long an invitation link works.
pub const INVITATION_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// The console path an invitation link opens; the token is in the fragment.
pub const INVITE_PATH: &str = "/console/invite";

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
            settings(Op::post(
                "/console/api/members",
                "inviteMember",
                "Invite someone by email",
            ))
            .about(
                "Returns a one-time link for the email and role, shown only here (nothing is \
sent). It works once, for 7 days, and replaces any earlier invitation of the email to this \
workspace. Inviting an owner needs `ownership`. An email that is already a member's is `409`.",
            )
            .body("InviteMember")
            .reply(201, "InvitationCreated", "The invitation and its link")
            .errors(&[409]),
            invite,
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
        .add(
            settings(Op::get(
                "/console/api/invitations",
                "listInvitations",
                "List pending invitations",
            ))
            .about("Unexpired invitations of the workspace, oldest first; never a link.")
            .page()
            .reply(200, "InvitationList", "A page of invitations"),
            list_invitations,
        )
        .add(
            settings(Op::delete(
                "/console/api/invitations/{invitation}",
                "revokeInvitation",
                "Revoke an invitation",
            ))
            .about("Its link stops working.")
            .response(204, "Revoked", None),
            revoke_invitation,
        )
        .add(
            Op::post(
                "/console/api/invitations/accept",
                "acceptInvitation",
                "Accept an invitation",
            )
            .about(
                "With the token of an invitation link. Signed in (the cookie, with \
`Cavea-Csrf-Token`) as the invited email: joins the workspace. Not signed in, for an email with \
no account: `password` creates the user, who joins and is signed in. A wrong, expired or used \
token is `401` `invitation`; signed in as another email `403` `invitation_email`; an email that \
has an account, not signed in, `409` `sign_in`.",
            )
            .body("AcceptInvitation")
            .reply(200, "ConsoleMe", "Joined, signed in")
            .sets_cookie()
            .errors(&[401, 403, 409, 429]),
            accept,
        )
        .add(
            settings(Op::patch(
                "/console/api/workspace",
                "renameWorkspace",
                "Rename the workspace",
            ))
            .body("RenameWorkspace")
            .reply(200, "ConsoleWorkspace", "The workspace"),
            rename,
        )
        .add(
            settings(Op::get(
                "/console/api/audit",
                "listAudit",
                "The workspace's audit log",
            ))
            .about(
                "Newest first (the cursor is an entry's id): the workspace's sensitive \
actions, and the sign-ins, sign-outs and password changes of its current members.",
            )
            .page()
            .reply(200, "AuditList", "A page of entries"),
            list_audit,
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
            let entry = Entry::by(&auth, &actor, "member.role_changed")
                .target(&target)
                .detail(json!({ "from": old.as_str(), "to": role.as_str() }));
            audit::record(&mut tx, entry).await?;
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
        let (old, _) = checked_change(&mut tx, &auth, user, None).await?;
        sqlx::query(REMOVE)
            .bind(auth.workspace().uuid())
            .bind(user)
            .execute(&mut *tx)
            .await?;
        let entry = Entry::by(&auth, &actor, "member.removed")
            .target(&target)
            .detail(json!({ "role": old.as_str() }));
        audit::record(&mut tx, entry).await?;
        tx.commit().await?;
        Ok::<_, ApiError>(())
    };
    work.instrument(cavea_telemetry::db_span("remove_member"))
        .await?;
    tracing::info!(member = %target, by = %actor, "member removed");
    Ok(StatusCode::NO_CONTENT)
}

// ---- invitations ----

#[derive(Debug, Serialize)]
pub(super) struct InvitationView {
    id: String,
    email: String,
    role: String,
    created_at: String,
    expires_at: String,
}

type InvitationRow = (String, String, String, String, String);

impl From<InvitationRow> for InvitationView {
    fn from((id, email, role, created_at, expires_at): InvitationRow) -> Self {
        Self {
            id: format!("inv_{id}"),
            email,
            role,
            created_at,
            expires_at,
        }
    }
}

macro_rules! invitation_columns {
    () => {
        concat!(
            "replace(i.id::text, '-', ''), i.email, i.role, ",
            rfc3339!("i", ".created_at"),
            ", ",
            rfc3339!("i", ".expires_at")
        )
    };
}

/// An invitation token: 64 lowercase hex digits (256 bits).
fn invitation_token() -> Result<String, ApiError> {
    crate::console::setup::token().map_err(|e| ApiError::internal(e.to_string()))
}

fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

const IS_MEMBER: &str = "SELECT EXISTS (SELECT FROM memberships m JOIN users u ON u.id = m.user_id
 WHERE m.workspace_id = $1::uuid AND u.email = $2)";
const SWEEP_INVITATIONS: &str =
    "DELETE FROM invitations WHERE workspace_id = $1::uuid AND (email = $2 OR expires_at <= now())";
const INSERT_INVITATION: &str = concat!(
    "WITH i AS (
  INSERT INTO invitations (id, workspace_id, email, role, token_hash, created_by, expires_at)
  VALUES (gen_random_uuid(), $1::uuid, $2, $3, $4, $5::uuid, now() + $6 * interval '1 second')
  RETURNING *
)
SELECT ",
    invitation_columns!(),
    " FROM i"
);

#[derive(Debug, Serialize)]
pub(super) struct InvitationCreated {
    invitation: InvitationView,
    /// The one-time link, shown only here.
    link: String,
}

impl InvitationCreated {
    pub(super) fn id(&self) -> &str {
        &self.invitation.id
    }
}

/// Create an invitation of `email` to the workspace as `role`, replacing
/// any earlier one of that email, inside `tx`, with its audit row. `409` if
/// the email is already a member's.
pub(super) async fn create_invitation(
    tx: &mut sqlx::PgConnection,
    state: &AppState,
    workspace: &str,
    by: (&str, Option<&str>),
    email: &str,
    role: Role,
) -> Result<InvitationCreated, ApiError> {
    let member: bool = sqlx::query_scalar(IS_MEMBER)
        .bind(workspace)
        .bind(email)
        .fetch_one(&mut *tx)
        .await?;
    if member {
        return Err(
            ApiError::new(Code::AlreadyExists).with_message("This email is already a member's.")
        );
    }
    sqlx::query(SWEEP_INVITATIONS)
        .bind(workspace)
        .bind(email)
        .execute(&mut *tx)
        .await?;
    let token = invitation_token()?;
    let ttl = i64::try_from(INVITATION_TTL.as_secs()).unwrap_or(i64::MAX);
    let row: InvitationRow = sqlx::query_as(INSERT_INVITATION)
        .bind(workspace)
        .bind(email)
        .bind(role.as_str())
        .bind(token_hash(&token).as_slice())
        .bind(by.1)
        .bind(ttl)
        .fetch_one(&mut *tx)
        .await?;
    let invitation = InvitationView::from(row);
    let entry = Entry {
        workspace: Some(workspace),
        actor: by.0,
        user: by.1,
        action: "member.invited",
        target: Some(&invitation.id),
        detail: Some(json!({ "role": role.as_str() })),
    };
    audit::record(tx, entry).await?;
    let base = state.config.public_url.as_ref().map_or("", |u| u.as_str());
    let link = format!("{}{INVITE_PATH}#token={token}", base.trim_end_matches('/'));
    Ok(InvitationCreated { invitation, link })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InviteMember {
    email: String,
    role: String,
}

async fn invite(
    Member(auth): Member,
    State(state): State<AppState>,
    Json(body): Json<InviteMember>,
) -> Result<(StatusCode, axum::Json<InvitationCreated>), ApiError> {
    let email = normalise_email(&body.email)
        .ok_or_else(|| invalid_field("email", "must be an email address"))?;
    let role = role_field(&body.role)?;
    if role == Role::Owner && !auth.role().is_some_and(|r| r.allows(Permission::Ownership)) {
        return Err(ownership());
    }
    let actor = auth.actor();
    let work = async {
        let mut tx = state.db.begin().await?;
        sqlx::query(LOCK_WORKSPACE)
            .bind(auth.workspace().uuid())
            .execute(&mut *tx)
            .await?;
        let by = (&*actor, auth.user());
        let created =
            create_invitation(&mut tx, &state, auth.workspace().uuid(), by, &email, role).await?;
        tx.commit().await?;
        Ok::<_, ApiError>(created)
    };
    let created = work
        .instrument(cavea_telemetry::db_span("create_invitation"))
        .await?;
    tracing::info!(invitation = %created.invitation.id, role = role.as_str(), by = %actor, "member invited");
    Ok((StatusCode::CREATED, axum::Json(created)))
}

const LIST_INVITATIONS: &str = concat!(
    "SELECT ",
    invitation_columns!(),
    " FROM invitations i
 WHERE i.workspace_id = $1::uuid AND i.expires_at > now()
   AND ($2::uuid IS NULL OR (i.created_at, i.id) >
        (SELECT created_at, id FROM invitations WHERE id = $2::uuid AND workspace_id = $1::uuid))
 ORDER BY i.created_at, i.id
 LIMIT $3"
);

fn invitation_uuid(id: &str) -> Option<&str> {
    id.strip_prefix("inv_").filter(|hex| {
        hex.len() == 32 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

async fn list_invitations(
    Member(auth): Member,
    Query(page): Query<Page>,
    State(state): State<AppState>,
) -> Result<axum::Json<List<InvitationView>>, ApiError> {
    let limit = page.limit()?;
    let after = match page.after.as_deref() {
        None => None,
        Some(after) => Some(
            invitation_uuid(after)
                .ok_or_else(|| query_refused("`after` is not an invitation id."))?,
        ),
    };
    let rows: Vec<InvitationRow> = sqlx::query_as(LIST_INVITATIONS)
        .bind(auth.workspace().uuid())
        .bind(after)
        .bind(i64::from(limit) + 1)
        .fetch_all(&state.db)
        .instrument(cavea_telemetry::db_span("list_invitations"))
        .await?;
    let invitations = rows.into_iter().map(InvitationView::from).collect();
    Ok(axum::Json(List::new(invitations, limit, |i| i.id.clone())))
}

const REVOKE_INVITATION: &str =
    "DELETE FROM invitations WHERE workspace_id = $1::uuid AND id = $2::uuid";

async fn revoke_invitation(
    Member(auth): Member,
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<StatusCode, ApiError> {
    let missing = || ApiError::new(Code::NotFound).with_detail("missing", "invitation");
    let uuid = invitation_uuid(&id).ok_or_else(missing)?;
    let actor = auth.actor();
    let work = async {
        let mut tx = state.db.begin().await?;
        let done = sqlx::query(REVOKE_INVITATION)
            .bind(auth.workspace().uuid())
            .bind(uuid)
            .execute(&mut *tx)
            .await?;
        if done.rows_affected() == 0 {
            return Err(missing());
        }
        let entry = Entry::by(&auth, &actor, "invitation.revoked").target(&id);
        audit::record(&mut tx, entry).await?;
        tx.commit().await?;
        Ok(())
    };
    work.instrument(cavea_telemetry::db_span("revoke_invitation"))
        .await?;
    tracing::info!(invitation = %id, by = %actor, "invitation revoked");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Accept {
    token: String,
    #[serde(default)]
    password: Option<String>,
}

fn invitation_refused() -> ApiError {
    ApiError::new(Code::Unauthorized)
        .with_message("The invitation link is wrong, expired or used.")
        .with_detail("reason", "invitation")
}

const LIVE_INVITATION: &str =
    "SELECT token_hash FROM invitations WHERE token_hash = $1 AND expires_at > now()";
/// Taking the invitation is what makes it single-use: of two acceptances,
/// one deletes the row.
const TAKE_INVITATION: &str = "DELETE FROM invitations WHERE token_hash = $1 AND expires_at > now()
RETURNING replace(id::text, '-', ''), replace(workspace_id::text, '-', ''), email, role";
const USER_EMAIL: &str = "SELECT email FROM users WHERE id = $1::uuid";
const USER_EXISTS: &str = "SELECT EXISTS (SELECT FROM users WHERE email = $1)";
const CREATE_USER: &str = "INSERT INTO users (id, email, password_hash)
VALUES (gen_random_uuid(), $1, $2) RETURNING replace(id::text, '-', '')";
const JOIN: &str = "INSERT INTO memberships (user_id, workspace_id, role)
VALUES ($1::uuid, $2::uuid, $3) ON CONFLICT DO NOTHING";

/// Signed in as the invited email, the user joins (their session stays);
/// not signed in, an email without an account sets a password, and the new
/// user joins and is signed in. Every wrong, expired or used token answers
/// the same; nothing tells a stranger without a token whether an email has
/// an account. The token, the link and the email are never logged.
async fn accept(
    State(state): State<AppState>,
    ClientIp(addr): ClientIp,
    headers: HeaderMap,
    Json(body): Json<Accept>,
) -> Result<Response, ApiError> {
    super::staff::same_origin(&headers)?;
    budget(&state, addr)?;
    let token = body.token.as_str();
    let well_formed = token.len() == 64
        && token
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !well_formed {
        return Err(invitation_refused());
    }
    let hash = token_hash(token);
    // Signed in: the live session decides the path, and then its CSRF token
    // is required. A cookie of an ended session is ignored.
    let live = match session_cookie(&headers, &state) {
        Some(Some(id)) => sessions::find(&state.db, &id.hash(), None)
            .await?
            .map(|session| (id, session)),
        _ => None,
    };
    if let Some((_, session)) = live {
        let id = cookie_session(&headers, &Method::POST, &state)?;
        {
            let user = session.user.clone();
            accept_signed_in(&state, &hash, &user).await?;
            let body = me_json(&state, &id, &session).await?;
            return Ok(axum::Json(body).into_response());
        }
    }
    let password = body
        .password
        .ok_or_else(|| invalid_field("password", "is required unless signed in"))?;
    check_password(&password).map_err(|problem| invalid_field("password", problem))?;
    // Before hashing: a wrong token costs one query, not argon2's work.
    let live: Option<Vec<u8>> = sqlx::query_scalar(LIVE_INVITATION)
        .bind(hash.as_slice())
        .fetch_optional(&state.db)
        .instrument(cavea_telemetry::db_span("check_invitation"))
        .await?;
    if !live.is_some_and(|stored| bool::from(hash.ct_eq(stored.as_slice()))) {
        return Err(invitation_refused());
    }
    let password_hash = password::hash_bounded(password).await.map_err(busy)?;
    let id = new_session()?;
    let work = async {
        let mut tx = state.db.begin().await?;
        let (invitation, workspace, email, role) = take(&mut tx, &hash).await?;
        let exists: bool = sqlx::query_scalar(USER_EXISTS)
            .bind(&email)
            .fetch_one(&mut *tx)
            .await?;
        if exists {
            return Err(ApiError::new(Code::AlreadyExists)
                .with_message("This email has an account: sign in, then open the link again.")
                .with_detail("reason", "sign_in"));
        }
        let created = sqlx::query_scalar(CREATE_USER)
            .bind(&email)
            .bind(&password_hash)
            .fetch_one(&mut *tx)
            .await;
        let user: String = match created {
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Err(ApiError::new(Code::AlreadyExists)
                    .with_message("This email has an account: sign in, then open the link again.")
                    .with_detail("reason", "sign_in"));
            }
            other => other?,
        };
        join(&mut tx, &invitation, &workspace, &user, &role).await?;
        sessions::insert(&mut tx, &id.hash(), &user).await?;
        tx.commit().await?;
        Ok(user)
    };
    let user = work
        .instrument(cavea_telemetry::db_span("accept_invitation"))
        .await?;
    tracing::info!(user = %format!("usr_{user}"), "invitation accepted; user created");
    signed_in(&state, StatusCode::OK, &id).await
}

/// Take the live invitation with this token's hash, inside `tx`.
async fn take(
    tx: &mut sqlx::PgConnection,
    hash: &[u8; 32],
) -> Result<(String, String, String, String), ApiError> {
    let row: Option<(String, String, String, String)> = sqlx::query_as(TAKE_INVITATION)
        .bind(hash.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
    row.ok_or_else(invitation_refused)
}

/// Make `user` a member as invited, with the audit row. A user already in
/// the workspace keeps their role.
async fn join(
    tx: &mut sqlx::PgConnection,
    invitation: &str,
    workspace: &str,
    user: &str,
    role: &str,
) -> Result<(), ApiError> {
    sqlx::query(JOIN)
        .bind(user)
        .bind(workspace)
        .bind(role)
        .execute(&mut *tx)
        .await?;
    let actor = format!("usr_{user}");
    let target = format!("inv_{invitation}");
    let entry = Entry {
        workspace: Some(workspace),
        actor: &actor,
        user: Some(user),
        action: "member.joined",
        target: Some(&target),
        detail: Some(json!({ "role": role })),
    };
    audit::record(tx, entry).await?;
    Ok(())
}

async fn accept_signed_in(state: &AppState, hash: &[u8; 32], user: &str) -> Result<(), ApiError> {
    let work = async {
        let mut tx = state.db.begin().await?;
        let (invitation, workspace, email, role) = take(&mut tx, hash).await?;
        let mine: String = sqlx::query_scalar(USER_EMAIL)
            .bind(user)
            .fetch_one(&mut *tx)
            .await?;
        if mine != email {
            // Rolled back: the invitation stays for its own email.
            return Err(ApiError::new(Code::Forbidden)
                .with_message("This invitation is for another email.")
                .with_detail("reason", "invitation_email"));
        }
        join(&mut tx, &invitation, &workspace, user, &role).await?;
        tx.commit().await?;
        Ok(())
    };
    work.instrument(cavea_telemetry::db_span("accept_invitation"))
        .await?;
    tracing::info!(user = %format!("usr_{user}"), "invitation accepted");
    Ok(())
}

// ---- the workspace ----

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameWorkspace {
    name: String,
}

const RENAME: &str = "UPDATE workspaces SET name = $2 WHERE id = $1::uuid RETURNING key";

async fn rename(
    Member(auth): Member,
    State(state): State<AppState>,
    Json(body): Json<RenameWorkspace>,
) -> Result<axum::Json<Value>, ApiError> {
    check_text("name", &body.name)?;
    let actor = auth.actor();
    let id = auth.workspace().to_string();
    let work = async {
        let mut tx = state.db.begin().await?;
        let key: String = sqlx::query_scalar(RENAME)
            .bind(auth.workspace().uuid())
            .bind(&body.name)
            .fetch_one(&mut *tx)
            .await?;
        let entry = Entry::by(&auth, &actor, "workspace.renamed").target(&id);
        audit::record(&mut tx, entry).await?;
        tx.commit().await?;
        Ok::<_, ApiError>(key)
    };
    let key = work
        .instrument(cavea_telemetry::db_span("rename_workspace"))
        .await?;
    Ok(axum::Json(
        json!({ "id": id, "key": key, "name": body.name }),
    ))
}

// ---- the audit log ----

#[derive(Debug, Serialize)]
struct AuditView {
    id: String,
    at: String,
    actor: String,
    action: String,
    target: Option<String>,
    detail: Option<Value>,
    request_id: Option<String>,
}

type AuditRow = (
    i64,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// The workspace's rows, and the instance's rows (sign-ins and the like) of
/// users who are its members now; newest first.
const LIST_AUDIT: &str = concat!(
    "SELECT a.id, ",
    rfc3339!("a", ".at"),
    ", a.actor, a.action, a.target, a.detail::text, a.request_id
  FROM audit_log a
 WHERE (a.workspace_id = $1::uuid
        OR (a.workspace_id IS NULL AND a.user_id IN
            (SELECT user_id FROM memberships WHERE workspace_id = $1::uuid)))
   AND ($2::bigint IS NULL OR a.id < $2)
 ORDER BY a.id DESC
 LIMIT $3"
);

async fn list_audit(
    Member(auth): Member,
    Query(page): Query<Page>,
    State(state): State<AppState>,
) -> Result<axum::Json<List<AuditView>>, ApiError> {
    let limit = page.limit()?;
    let after = match page.after.as_deref() {
        None => None,
        Some(after) => Some(
            after
                .strip_prefix("aud_")
                .and_then(|n| n.parse::<i64>().ok())
                .ok_or_else(|| query_refused("`after` is not an audit entry id."))?,
        ),
    };
    let rows: Vec<AuditRow> = sqlx::query_as(LIST_AUDIT)
        .bind(auth.workspace().uuid())
        .bind(after)
        .bind(i64::from(limit) + 1)
        .fetch_all(&state.db)
        .instrument(cavea_telemetry::db_span("list_audit"))
        .await?;
    let entries = rows
        .into_iter()
        .map(
            |(id, at, actor, action, target, detail, request_id)| AuditView {
                id: format!("aud_{id}"),
                at,
                actor,
                action,
                target,
                detail: detail.and_then(|d| serde_json::from_str(&d).ok()),
                request_id,
            },
        )
        .collect();
    Ok(axum::Json(List::new(entries, limit, |e| e.id.clone())))
}
