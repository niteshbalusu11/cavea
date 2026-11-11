//! `/v1/keys`: list, create and revoke the API keys of the caller's
//! workspace (08 §6.2, §11).

use axum::Router;
use axum::http::StatusCode;
use axum::routing::{delete, get};
use serde::{Deserialize, Serialize};
use tracing::Instrument as _;

use super::auth::{KeyId, KeyKind, NewKey, SecretKey, normalise_origin};
use super::extract::{Json, Page, Path, Query, check_text, invalid_field};
use super::{ApiError, AppState, Code, List};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/keys", get(list).post(create))
        .route("/v1/keys/{id}", delete(revoke))
}

/// The most entries in a publishable key's origin allow-list.
const MAX_ORIGINS: usize = 50;

/// `api_keys` columns in [`KeyRow`] order. Times are RFC 3339 UTC, formatted
/// in SQL.
macro_rules! key_columns {
    ($t:literal) => {
        concat!(
            "replace(",
            $t,
            ".id::text, '-', ''), ",
            $t,
            ".kind, ",
            $t,
            ".name, ",
            $t,
            ".prefix, ",
            $t,
            ".allowed_origins, ",
            rfc3339!($t, ".created_at"),
            ", ",
            rfc3339!($t, ".last_used_at"),
            ", ",
            rfc3339!($t, ".revoked_at")
        )
    };
}
pub(super) use key_columns;

pub(super) type KeyRow = (
    String,
    String,
    String,
    String,
    Option<Vec<String>>,
    String,
    Option<String>,
    Option<String>,
);

/// An API key as the API shows it. `key` is present only in the response that
/// created it.
#[derive(Debug, Serialize)]
pub(super) struct KeyView {
    id: String,
    kind: KeyKind,
    name: String,
    prefix: String,
    allowed_origins: Option<Vec<String>>,
    created_at: String,
    last_used_at: Option<String>,
    revoked_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
}

impl KeyView {
    pub(super) fn new(row: KeyRow, key: Option<String>) -> Result<Self, ApiError> {
        let (id, kind, name, prefix, allowed_origins, created_at, last_used_at, revoked_at) = row;
        Ok(Self {
            id: format!("key_{id}"),
            kind: KeyKind::from_db(&kind)?,
            name,
            prefix,
            allowed_origins,
            created_at,
            last_used_at,
            revoked_at,
            key,
        })
    }
}

/// Oldest first; revoked keys included.
const LIST: &str = concat!(
    "SELECT ",
    key_columns!("k"),
    " FROM api_keys k
 WHERE k.workspace_id = $1::uuid
   AND ($2::uuid IS NULL OR (k.created_at, k.id) >
        (SELECT created_at, id FROM api_keys WHERE id = $2::uuid AND workspace_id = $1::uuid))
 ORDER BY k.created_at, k.id
 LIMIT $3"
);

async fn list(
    SecretKey(auth): SecretKey,
    Query(page): Query<Page>,
    state: axum::extract::State<AppState>,
) -> Result<axum::Json<List<KeyView>>, ApiError> {
    let limit = page.limit()?;
    let after = match page.after.as_deref() {
        None => None,
        Some(after) => Some(KeyId::parse(after).ok_or_else(|| {
            ApiError::new(Code::InvalidRequest)
                .with_message("`after` is not a key id.")
                .with_detail("reason", "query")
        })?),
    };
    let rows: Vec<KeyRow> = sqlx::query_as(LIST)
        .bind(auth.workspace().uuid())
        .bind(after.as_ref().map(KeyId::uuid))
        .bind(i64::from(limit) + 1)
        .fetch_all(&state.db)
        .instrument(cavea_telemetry::db_span("list_keys"))
        .await?;
    let keys = rows
        .into_iter()
        .map(|row| KeyView::new(row, None))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(axum::Json(List::new(keys, limit, |k| k.id.clone())))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateKey {
    kind: KeyKind,
    name: String,
    #[serde(default)]
    allowed_origins: Option<Vec<String>>,
}

const CREATE: &str = concat!(
    "INSERT INTO api_keys (id, workspace_id, kind, prefix, hash, name, allowed_origins)
 VALUES (gen_random_uuid(), $1::uuid, $2, $3, $4, $5, $6)
 RETURNING ",
    key_columns!("api_keys")
);

async fn create(
    SecretKey(auth): SecretKey,
    state: axum::extract::State<AppState>,
    Json(body): Json<CreateKey>,
) -> Result<(StatusCode, axum::Json<KeyView>), ApiError> {
    check_text("name", &body.name)?;
    let origins = match (body.kind, body.allowed_origins) {
        (_, None) => None,
        (KeyKind::Secret, Some(_)) => {
            return Err(invalid_field(
                "allowed_origins",
                "is only for publishable keys",
            ));
        }
        (KeyKind::Publishable, Some(list)) => {
            if list.is_empty() || list.len() > MAX_ORIGINS {
                return Err(invalid_field(
                    "allowed_origins",
                    "must list 1 to 50 origins; omit it to allow every origin",
                ));
            }
            let list = list
                .iter()
                .map(|o| normalise_origin(o))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    invalid_field(
                        "allowed_origins",
                        "must hold origins such as https://tickets.example.com",
                    )
                })?;
            Some(list)
        }
    };
    let new = NewKey::generate(body.kind)?;
    let row: KeyRow = sqlx::query_as(CREATE)
        .bind(auth.workspace().uuid())
        .bind(body.kind.as_str())
        .bind(&new.prefix)
        .bind(new.hash.as_slice())
        .bind(&body.name)
        .bind(origins)
        .fetch_one(&state.db)
        .instrument(cavea_telemetry::db_span("create_key"))
        .await?;
    let view = KeyView::new(row, Some(new.key))?;
    tracing::info!(key_id = %view.id, kind = body.kind.as_str(), by = %auth.key_id(), "api key created");
    Ok((StatusCode::CREATED, axum::Json(view)))
}

/// Serialises key changes in a workspace, so that two keys revoking each
/// other at once cannot both succeed.
const LOCK_WORKSPACE: &str = "SELECT FROM workspaces WHERE id = $1::uuid FOR NO KEY UPDATE";
const CALLER_ACTIVE: &str =
    "SELECT EXISTS (SELECT FROM api_keys WHERE id = $1::uuid AND revoked_at IS NULL)";
/// Idempotent: revoking a revoked key returns it unchanged.
const REVOKE: &str = concat!(
    "UPDATE api_keys SET revoked_at = coalesce(revoked_at, now())
 WHERE id = $2::uuid AND workspace_id = $1::uuid
 RETURNING ",
    key_columns!("api_keys")
);

/// Revoke a key of the caller's workspace. A key may not revoke itself: every
/// revocation is made by another unrevoked secret key, so a workspace always
/// keeps one (08 §11).
async fn revoke(
    SecretKey(auth): SecretKey,
    Path(id): Path<String>,
    state: axum::extract::State<AppState>,
) -> Result<axum::Json<KeyView>, ApiError> {
    let id = KeyId::parse(&id).ok_or_else(|| ApiError::new(Code::NotFound))?;
    if &id == auth.key_id() {
        return Err(ApiError::new(Code::Forbidden)
            .with_message("A key cannot revoke itself; revoke it with another secret key.")
            .with_detail("reason", "self_revocation"));
    }
    let revoke = async {
        let mut tx = state.db.begin().await?;
        sqlx::query(LOCK_WORKSPACE)
            .bind(auth.workspace().uuid())
            .execute(&mut *tx)
            .await?;
        // Read after taking the lock: a concurrent revocation of the caller
        // has committed or not yet begun.
        let active: bool = sqlx::query_scalar(CALLER_ACTIVE)
            .bind(auth.key_id().uuid())
            .fetch_one(&mut *tx)
            .await?;
        if !active {
            return Err(ApiError::new(Code::Unauthorized).with_detail("reason", "invalid"));
        }
        let row: Option<KeyRow> = sqlx::query_as(REVOKE)
            .bind(auth.workspace().uuid())
            .bind(id.uuid())
            .fetch_optional(&mut *tx)
            .await?;
        tx.commit().await?;
        row.ok_or_else(|| ApiError::new(Code::NotFound))
    };
    let row = revoke
        .instrument(cavea_telemetry::db_span("revoke_key"))
        .await?;
    tracing::info!(key_id = %id, by = %auth.key_id(), "api key revoked");
    Ok(axum::Json(KeyView::new(row, None)?))
}
