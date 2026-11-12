//! `/v1/keys`: list, create and revoke the API keys of the caller's
//! workspace (08 §6.2, §11).

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use tracing::Instrument as _;

use super::auth::{KeyId, KeyKind, NewKey, normalise_origin};
use super::extract::{Json, Page, Path, Query, Secret, check_text, invalid_field};
use super::openapi::{Needs, Op, Routes};
use super::staff::Staff;
use super::{ApiError, AppState, Code, List};
use crate::audit::{self, Entry};
use crate::console::Permission;
use serde_json::json;

pub(super) fn routes() -> Routes {
    Routes::new()
        .add(
            Op::get("/v1/keys", "listKeys", "List the workspace's API keys")
                .about("Revoked keys included; never a key or its hash.")
                .needs(Needs::Staff(Permission::Developers))
                .page()
                .reply(200, "KeyList", "A page of keys"),
            list,
        )
        .add(
            Op::post("/v1/keys", "createKey", "Create an API key")
                .needs(Needs::Staff(Permission::Developers))
                .body("CreateKey")
                .reply(201, "KeyCreated", "The key, shown only here"),
            create,
        )
        .add(
            Op::patch(
                "/v1/keys/{id}",
                "updateKeyOrigins",
                "Change a publishable key's origins",
            )
            .about(
                "Replaces the origin allow-list of an unrevoked publishable key; `null` allows \
every origin. It applies to the key's next request.",
            )
            .needs(Needs::Staff(Permission::Developers))
            .body("UpdateKey")
            .reply(200, "Key", "The key"),
            update_origins,
        )
        .add(
            Op::delete("/v1/keys/{id}", "revokeKey", "Revoke an API key")
                .about("Idempotent. A key may not revoke itself (`403`, `reason: \"self_revocation\"`).")
                .needs(Needs::Staff(Permission::Developers))
                .reply(200, "Key", "The key, revoked"),
            revoke,
        )
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
    pub(super) id: String,
    kind: KeyKind,
    name: String,
    prefix: String,
    allowed_origins: Option<Vec<String>>,
    created_at: String,
    last_used_at: Option<String>,
    revoked_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<Secret>,
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
            key: key.map(Secret::new),
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
    Staff(auth): Staff,
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

/// An allow-list as given, checked and normalised.
fn origins(list: &[String]) -> Result<Vec<String>, ApiError> {
    if list.is_empty() || list.len() > MAX_ORIGINS {
        return Err(invalid_field(
            "allowed_origins",
            "must list 1 to 50 origins; omit it to allow every origin",
        ));
    }
    list.iter()
        .map(|o| normalise_origin(o))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            invalid_field(
                "allowed_origins",
                "must hold origins such as https://tickets.example.com",
            )
        })
}

const CREATE: &str = concat!(
    "INSERT INTO api_keys (id, workspace_id, kind, prefix, hash, name, allowed_origins)
 VALUES (gen_random_uuid(), $1::uuid, $2, $3, $4, $5, $6)
 RETURNING ",
    key_columns!("api_keys")
);

async fn create(
    Staff(auth): Staff,
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
        (KeyKind::Publishable, Some(list)) => Some(origins(&list)?),
    };
    let new = NewKey::generate(body.kind)?;
    let actor = auth.actor();
    let create = async {
        let mut tx = state.db.begin().await?;
        let row: KeyRow = sqlx::query_as(CREATE)
            .bind(auth.workspace().uuid())
            .bind(body.kind.as_str())
            .bind(&new.prefix)
            .bind(new.hash.as_slice())
            .bind(&body.name)
            .bind(origins)
            .fetch_one(&mut *tx)
            .await?;
        let id = format!("key_{}", row.0);
        let entry = Entry::by(&auth, &actor, "api_key.created")
            .target(&id)
            .detail(json!({ "kind": body.kind.as_str() }));
        audit::record(&mut tx, entry).await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(row)
    };
    let row = create
        .instrument(cavea_telemetry::db_span("create_key"))
        .await?;
    let view = KeyView::new(row, Some(new.key))?;
    tracing::info!(key_id = %view.id, kind = body.kind.as_str(), by = %auth.actor(), "api key created");
    Ok((StatusCode::CREATED, axum::Json(view)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateKey {
    /// Required: `null` allows every origin, so leaving it out must not.
    #[serde(deserialize_with = "present")]
    allowed_origins: Option<Vec<String>>,
}

fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::deserialize(d)
}

const UPDATE_ORIGINS: &str = concat!(
    "UPDATE api_keys SET allowed_origins = $3
 WHERE id = $2::uuid AND workspace_id = $1::uuid AND kind = 'publishable' AND revoked_at IS NULL
 RETURNING ",
    key_columns!("api_keys")
);

/// Why a key's origins could not change: its kind and whether it is revoked.
const KEY_STATE: &str = "SELECT kind, revoked_at IS NOT NULL FROM api_keys
 WHERE id = $2::uuid AND workspace_id = $1::uuid";

/// Replace a publishable key's origin allow-list (09 §6). Keys are read on
/// every request, so the next one follows it.
async fn update_origins(
    Staff(auth): Staff,
    Path(id): Path<String>,
    state: axum::extract::State<AppState>,
    Json(body): Json<UpdateKey>,
) -> Result<axum::Json<KeyView>, ApiError> {
    let id = KeyId::parse(&id).ok_or_else(|| ApiError::new(Code::NotFound))?;
    let list = body.allowed_origins.as_deref().map(origins).transpose()?;
    let count = list.as_ref().map(Vec::len);
    let actor = auth.actor();
    let update = async {
        let mut tx = state.db.begin().await?;
        let row: Option<KeyRow> = sqlx::query_as(UPDATE_ORIGINS)
            .bind(auth.workspace().uuid())
            .bind(id.uuid())
            .bind(list)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            let found: Option<(String, bool)> = sqlx::query_as(KEY_STATE)
                .bind(auth.workspace().uuid())
                .bind(id.uuid())
                .fetch_optional(&mut *tx)
                .await?;
            return Err(match found {
                None => ApiError::new(Code::NotFound),
                Some((_, true)) => super::extract::invalid("revoked", "The key is revoked."),
                Some(_) => invalid_field("allowed_origins", "is only for publishable keys"),
            });
        };
        let target = id.to_string();
        let entry = Entry::by(&auth, &actor, "api_key.origins_changed")
            .target(&target)
            .detail(json!({ "origins": count }));
        audit::record(&mut tx, entry).await?;
        tx.commit().await?;
        Ok(row)
    };
    let row = update
        .instrument(cavea_telemetry::db_span("update_key_origins"))
        .await?;
    tracing::info!(key_id = %id, origins = ?count, by = %actor, "api key origins changed");
    Ok(axum::Json(KeyView::new(row, None)?))
}

/// Serialises key changes in a workspace, so that two keys revoking each
/// other at once cannot both succeed.
const LOCK_WORKSPACE: &str = "SELECT FROM workspaces WHERE id = $1::uuid FOR NO KEY UPDATE";
const CALLER_ACTIVE: &str =
    "SELECT EXISTS (SELECT FROM api_keys WHERE id = $1::uuid AND revoked_at IS NULL)";
/// Whether the key is unrevoked, read under the workspace's lock: only a
/// revocation that changes it is audited.
const LIVE: &str =
    "SELECT revoked_at IS NULL FROM api_keys WHERE workspace_id = $1::uuid AND id = $2::uuid";
/// Idempotent: revoking a revoked key returns it unchanged.
const REVOKE: &str = concat!(
    "UPDATE api_keys SET revoked_at = coalesce(revoked_at, now())
 WHERE id = $2::uuid AND workspace_id = $1::uuid
 RETURNING ",
    key_columns!("api_keys")
);

/// Revoke a key of the caller's workspace. A key may not revoke itself: every
/// revocation by a key is made by another unrevoked secret key, so a
/// workspace keeps one unless a console member revokes it, who can mint
/// another (08 §11).
async fn revoke(
    Staff(auth): Staff,
    Path(id): Path<String>,
    state: axum::extract::State<AppState>,
) -> Result<axum::Json<KeyView>, ApiError> {
    let id = KeyId::parse(&id).ok_or_else(|| ApiError::new(Code::NotFound))?;
    if auth.key_id() == Some(&id) {
        return Err(ApiError::new(Code::Forbidden)
            .with_message("A key cannot revoke itself; revoke it with another secret key.")
            .with_detail("reason", "self_revocation"));
    }
    let actor = auth.actor();
    let revoke = async {
        let mut tx = state.db.begin().await?;
        sqlx::query(LOCK_WORKSPACE)
            .bind(auth.workspace().uuid())
            .execute(&mut *tx)
            .await?;
        // Read after taking the lock: a concurrent revocation of the caller
        // has committed or not yet begun. A console member has no key.
        if let Some(caller) = auth.key_id() {
            let active: bool = sqlx::query_scalar(CALLER_ACTIVE)
                .bind(caller.uuid())
                .fetch_one(&mut *tx)
                .await?;
            if !active {
                return Err(ApiError::new(Code::Unauthorized).with_detail("reason", "invalid"));
            }
        }
        let live: Option<bool> = sqlx::query_scalar(LIVE)
            .bind(auth.workspace().uuid())
            .bind(id.uuid())
            .fetch_optional(&mut *tx)
            .await?;
        let row: Option<KeyRow> = sqlx::query_as(REVOKE)
            .bind(auth.workspace().uuid())
            .bind(id.uuid())
            .fetch_optional(&mut *tx)
            .await?;
        if live == Some(true) {
            let target = id.to_string();
            let entry = Entry::by(&auth, &actor, "api_key.revoked").target(&target);
            audit::record(&mut tx, entry).await?;
        }
        tx.commit().await?;
        row.ok_or_else(|| ApiError::new(Code::NotFound))
    };
    let row = revoke
        .instrument(cavea_telemetry::db_span("revoke_key"))
        .await?;
    tracing::info!(key_id = %id, by = %auth.actor(), "api key revoked");
    Ok(axum::Json(KeyView::new(row, None)?))
}
