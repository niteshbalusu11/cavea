//! `/v1/workspaces`: list and create workspaces with the admin key (08 §6.2,
//! §11).

use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use tracing::Instrument as _;

use super::Reason;
use super::auth::{AdminKeyAuth, KeyKind, NewKey};
use super::extract::{Json, Page, Path, Query, check_text};
use super::keys::{KeyRow, KeyView, key_columns};
use super::members::{InvitationCreated, create_invitation};
use super::openapi::{Needs, Op, Routes};
use super::{ApiError, AppState, Code, List};
use crate::audit::{self, Entry};

pub(super) fn routes() -> Routes {
    Routes::new()
        .add(
            Op::get("/v1/workspaces", "listWorkspaces", "List workspaces")
                .needs(Needs::Admin)
                .page()
                .reply(200, "WorkspaceList", "A page of workspaces"),
            list,
        )
        .add(
            Op::post("/v1/workspaces", "createWorkspace", "Create a workspace")
                .needs(Needs::Admin)
                .body("CreateWorkspace")
                .reply(
                    201,
                    "WorkspaceCreated",
                    "The workspace and its first secret key, shown only here",
                )
                .errors(&[409]),
            create,
        )
        .add(
            Op::post(
                "/v1/workspaces/{workspace}/keys",
                "mintWorkspaceKey",
                "Mint a secret key for a workspace",
            )
            .needs(Needs::Admin)
            .body("MintKey")
            .reply(201, "KeyCreated", "The key, shown only here"),
            create_key,
        )
        .add(
            Op::post(
                "/v1/workspaces/{workspace}/invitations",
                "inviteWorkspaceOwner",
                "Invite an owner to a workspace",
            )
            .about(
                "How a workspace the admin key made gets its first member (and the operator's \
recovery for one whose owners are gone): a one-time link making `email` an owner, shown only \
here, accepted in the console as any invitation. `409` if the email is already a \
member's.",
            )
            .needs(Needs::Admin)
            .body("InviteOwner")
            .reply(201, "InvitationCreated", "The invitation and its link")
            .errors(&[409]),
            invite_owner,
        )
}

#[derive(Debug, Serialize)]
struct Workspace {
    id: String,
    key: String,
    name: String,
    created_at: String,
}

type WorkspaceRow = (String, String, String, String);

impl From<WorkspaceRow> for Workspace {
    fn from((id, key, name, created_at): WorkspaceRow) -> Self {
        Self {
            id: format!("ws_{id}"),
            key,
            name,
            created_at,
        }
    }
}

macro_rules! workspace_columns {
    () => {
        concat!(
            "replace(w.id::text, '-', ''), w.key, w.name, ",
            rfc3339!("w", ".created_at")
        )
    };
}

/// Oldest first.
const LIST: &str = concat!(
    "SELECT ",
    workspace_columns!(),
    " FROM workspaces w
 WHERE $1::uuid IS NULL OR (w.created_at, w.id) >
       (SELECT created_at, id FROM workspaces WHERE id = $1::uuid)
 ORDER BY w.created_at, w.id
 LIMIT $2"
);

async fn list(
    _: AdminKeyAuth,
    Query(page): Query<Page>,
    State(state): State<AppState>,
) -> Result<axum::Json<List<Workspace>>, ApiError> {
    let limit = page.limit()?;
    let after = match page.after.as_deref() {
        None => None,
        Some(after) => Some(workspace_uuid(after).ok_or_else(|| {
            ApiError::from(Reason::Query).with_message("`after` is not a workspace id.")
        })?),
    };
    let rows: Vec<WorkspaceRow> = sqlx::query_as(LIST)
        .bind(after)
        .bind(i64::from(limit) + 1)
        .fetch_all(&state.db)
        .instrument(cavea_telemetry::db_span("list_workspaces"))
        .await?;
    let workspaces = rows.into_iter().map(Workspace::from).collect();
    Ok(axum::Json(List::new(workspaces, limit, |w| w.id.clone())))
}

pub(super) fn workspace_uuid(id: &str) -> Option<&str> {
    let hex = id.strip_prefix("ws_")?;
    (hex.len() == 32 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))).then_some(hex)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateWorkspace {
    key: String,
    name: String,
}

#[derive(Debug, Serialize)]
struct Created {
    #[serde(flatten)]
    workspace: Workspace,
    /// The workspace's first secret key, shown once.
    secret_key: KeyView,
}

/// The workspace, its first secret key and the audit row, in one statement.
const CREATE: &str = concat!(
    "WITH w AS (
  INSERT INTO workspaces (id, key, name) VALUES (gen_random_uuid(), $1, $2)
  RETURNING *
), k AS (
  INSERT INTO api_keys (id, workspace_id, kind, prefix, hash, name)
  SELECT gen_random_uuid(), w.id, 'secret', $3, $4, 'Initial secret key' FROM w
  RETURNING *
), a AS (
  INSERT INTO audit_log (workspace_id, actor, action, target, detail, request_id)
  SELECT w.id, 'admin', 'workspace.created', 'ws_' || replace(w.id::text, '-', ''),
         jsonb_build_object('secret_key', 'key_' || replace(k.id::text, '-', '')), $5
    FROM w, k
)
SELECT ",
    workspace_columns!(),
    ", ",
    key_columns!("k"),
    " FROM w, k"
);

type CreatedRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<Vec<String>>,
    String,
    Option<String>,
    Option<String>,
);

/// Create a workspace. The response carries its first secret key, so the
/// operator can act in it at once; it is shown only here.
async fn create(
    _: AdminKeyAuth,
    State(state): State<AppState>,
    Json(body): Json<CreateWorkspace>,
) -> Result<(StatusCode, axum::Json<Created>), ApiError> {
    check_text("key", &body.key)?;
    check_text("name", &body.name)?;
    let new = NewKey::generate(KeyKind::Secret)?;
    let row: Result<CreatedRow, _> = sqlx::query_as(CREATE)
        .bind(&body.key)
        .bind(&body.name)
        .bind(&new.prefix)
        .bind(new.hash.as_slice())
        .bind(cavea_telemetry::request_id().as_ref().map(|r| r.as_str()))
        .fetch_one(&state.db)
        .instrument(cavea_telemetry::db_span("create_workspace"))
        .await;
    let row = match row {
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(ApiError::new(Code::AlreadyExists)
                .with_message("A workspace with this key already exists."));
        }
        row => row?,
    };
    let (id, key, name, created_at, k0, k1, k2, k3, k4, k5, k6, k7) = row;
    let workspace = Workspace::from((id, key, name, created_at));
    let secret_key = KeyView::new((k0, k1, k2, k3, k4, k5, k6, k7), Some(new.key))?;
    cavea_telemetry::record_workspace(&workspace.id);
    tracing::info!(workspace = %workspace.id, "workspace created");
    Ok((
        StatusCode::CREATED,
        axum::Json(Created {
            workspace,
            secret_key,
        }),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateKey {
    name: String,
}

const CREATE_KEY: &str = concat!(
    "INSERT INTO api_keys (id, workspace_id, kind, prefix, hash, name)
 SELECT gen_random_uuid(), w.id, 'secret', $2, $3, $4 FROM workspaces w WHERE w.id = $1::uuid
 RETURNING ",
    key_columns!("api_keys")
);

/// A new secret key for an existing workspace, shown only here: the
/// operator's way back into a workspace whose keys are all lost (08 §11).
async fn create_key(
    _: AdminKeyAuth,
    Path(workspace): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<CreateKey>,
) -> Result<(StatusCode, axum::Json<KeyView>), ApiError> {
    let not_found = || {
        ApiError::new(Code::NotFound)
            .with_message("No such workspace.")
            .with_detail("missing", "workspace")
    };
    let uuid = workspace_uuid(&workspace).ok_or_else(not_found)?;
    check_text("name", &body.name)?;
    let new = NewKey::generate(KeyKind::Secret)?;
    let mint = async {
        let mut tx = state.db.begin().await?;
        let row: Option<KeyRow> = sqlx::query_as(CREATE_KEY)
            .bind(uuid)
            .bind(&new.prefix)
            .bind(new.hash.as_slice())
            .bind(&body.name)
            .fetch_optional(&mut *tx)
            .await?;
        if let Some(row) = &row {
            let target = format!("key_{}", row.0);
            let entry = Entry {
                workspace: Some(uuid),
                actor: "admin",
                user: None,
                action: "api_key.created",
                target: Some(&target),
                detail: Some(serde_json::json!({ "kind": "secret" })),
            };
            audit::record(&mut tx, entry).await?;
        }
        tx.commit().await?;
        Ok::<_, sqlx::Error>(row)
    };
    let row = mint
        .instrument(cavea_telemetry::db_span("create_key"))
        .await?;
    let view = KeyView::new(row.ok_or_else(not_found)?, Some(new.key))?;
    cavea_telemetry::record_workspace(&workspace);
    tracing::info!(key_id = %view.id, by = "admin", "api key created");
    Ok((StatusCode::CREATED, axum::Json(view)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InviteOwner {
    email: String,
}

/// An owner invitation to an existing workspace, made by the admin key.
async fn invite_owner(
    _: AdminKeyAuth,
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<InviteOwner>,
) -> Result<(StatusCode, axum::Json<InvitationCreated>), ApiError> {
    let missing = || {
        ApiError::new(Code::NotFound)
            .with_message("No such workspace.")
            .with_detail("missing", "workspace")
    };
    let workspace = workspace_uuid(&id).ok_or_else(missing)?;
    let email = crate::console::normalise_email(&body.email)
        .ok_or_else(|| super::extract::invalid_field("email", "must be an email address"))?;
    let work = async {
        let mut tx = state.db.begin().await?;
        let found: Option<()> =
            sqlx::query_as("SELECT FROM workspaces WHERE id = $1::uuid FOR UPDATE")
                .bind(workspace)
                .fetch_optional(&mut *tx)
                .await?;
        found.ok_or_else(missing)?;
        let created = create_invitation(
            &mut tx,
            &state,
            workspace,
            ("admin", None),
            &email,
            crate::console::Role::Owner,
        )
        .await?;
        tx.commit().await?;
        Ok::<_, ApiError>(created)
    };
    let created = work
        .instrument(cavea_telemetry::db_span("create_invitation"))
        .await?;
    cavea_telemetry::record_workspace(&id);
    tracing::info!(invitation = %created.id(), by = "admin", "owner invited");
    Ok((StatusCode::CREATED, axum::Json(created)))
}
