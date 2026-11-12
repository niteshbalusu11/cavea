//! The licence for the console (12 §7.3, 09 §7, D-81): `licence` in
//! `ConsoleMe`, and `PUT /console/api/licence`, the upload.
//!
//! The licence is the server's, not a workspace's: only a user who owns
//! every workspace on the server may upload one or read its details
//! (`manage`), so that on a server shared by several organisations no
//! workspace's owner can change what every other one runs under. Every
//! signed-in user sees the state and whether the console is read-only.

use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use cavea_engine::Clock as _;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use tracing::Instrument as _;

use super::extract::Json;
use super::openapi::{Needs, Op, Routes};
use super::staff::ConsoleUser;
use super::{ApiError, AppState, Code};
use crate::audit::{self, Entry};
use crate::licence::{Found, State as LicenceState};

pub(super) fn routes() -> Routes {
    Routes::new().add(
        Op::put(
            "/console/api/licence",
            "consoleLicence",
            "Upload a licence file",
        )
        .about(
            "12 §7.3. The file's text in `licence`. Only a user who owns every workspace on \
the server may (`403` `instance_owner`). A file that is not a licence signed by a key this \
build trusts is `400` `invalid_field` (`licence`) with `details.problem` saying why; nothing \
is stored. Otherwise it is stored for every node (each reads it within 30 s) and answered \
with the licence as `me` shows it; if `CAVEA_LICENCE` holds a better one, that one still \
counts. Works in every licence state.",
        )
        .needs(Needs::Console)
        .body("LicenceUpload")
        .reply(200, "ConsoleLicence", "The licence that now counts"),
        upload,
    )
}

const INSTANCE_OWNER: &str = "SELECT EXISTS (
    SELECT FROM memberships WHERE user_id = $1::uuid AND role = 'owner')
  AND NOT EXISTS (
    SELECT FROM workspaces w
     WHERE NOT EXISTS (
       SELECT FROM memberships m
        WHERE m.user_id = $1::uuid AND m.workspace_id = w.id AND m.role = 'owner'))";

/// Whether `user` (32 hex digits) owns every workspace, and at least one.
async fn instance_owner(db: &PgPool, user: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(INSTANCE_OWNER)
        .bind(user)
        .fetch_one(db)
        .instrument(cavea_telemetry::db_span("read_instance_owner"))
        .await
}

/// `ConsoleLicence` for `user`.
pub(super) async fn for_user(state: &AppState, user: &str) -> Result<Value, ApiError> {
    let manage = instance_owner(&state.db, user).await?;
    Ok(json_of(state, manage))
}

fn json_of(state: &AppState, manage: bool) -> Value {
    let status = state.licences.status(state.seats.clock.now().0);
    let grace_until = match (status.state, &status.licence) {
        (LicenceState::Grace, Some(l)) => Some(l.last_grace_day().to_string()),
        _ => None,
    };
    let licence = status.licence.as_ref().filter(|_| manage).map(|l| {
        json!({
            "id": l.id,
            "customer": l.customer,
            "plan": l.plan,
            "features": l.features,
            "domains": l.domains,
            "max_workspaces": l.max_workspaces,
            "issued_at": l.issued_at.to_string(),
            "expires_at": l.expires_at.to_string(),
            "grace_days": l.grace_days,
            "key_id": l.key_id,
        })
    });
    json!({
        "state": status.state.as_str(),
        "read_only": status.read_only(),
        "grace_until": grace_until,
        "manage": manage,
        "source": status.source.filter(|_| manage).map(crate::licence::Source::as_str),
        "reason": status.reason.filter(|_| manage),
        "licence": licence,
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Upload {
    licence: String,
}

const STORE: &str = "INSERT INTO licence (id, document, uploaded_by) VALUES (1, $1, $2)
ON CONFLICT (id) DO UPDATE SET document = excluded.document, uploaded_at = now(),
  uploaded_by = excluded.uploaded_by";

async fn upload(
    State(state): State<AppState>,
    console: ConsoleUser,
    Json(body): Json<Upload>,
) -> Result<Response, ApiError> {
    let user = console.session.user;
    if !instance_owner(&state.db, &user).await? {
        return Err(ApiError::new(Code::Forbidden)
            .with_message(
                "Only a user who owns every workspace on this server may change its licence.",
            )
            .with_detail("reason", "instance_owner"));
    }
    let bytes = body.licence.into_bytes();
    let found = Found::of(state.licences.verifier(), &bytes);
    let licence = match &found {
        Found::Licence(licence) => licence.clone(),
        Found::Invalid(problem) => {
            return Err(ApiError::new(Code::InvalidRequest)
                .with_message(format!(
                    "`licence` is not a licence this server accepts ({problem})."
                ))
                .with_detail("reason", "invalid_field")
                .with_detail("field", "licence")
                .with_detail("problem", *problem));
        }
        Found::Missing => return Err(ApiError::internal("a verified upload is missing")),
    };
    let actor = format!("usr_{user}");
    let now = state.seats.clock.now().0;
    let standing = Found::Licence(licence.clone());
    let store = async {
        let mut tx = state.db.begin().await?;
        sqlx::query(STORE)
            .bind(&bytes)
            .bind(&actor)
            .execute(&mut *tx)
            .await?;
        let entry = Entry::console(&actor, &user, "licence.uploaded")
            .target(&licence.id)
            .detail(json!({ "expires_at": licence.expires_at.to_string() }));
        audit::record(&mut tx, entry).await?;
        tx.commit().await
    };
    store
        .instrument(cavea_telemetry::db_span("store_licence"))
        .await?;
    state.licences.set_upload(standing, now);
    tracing::info!(user = %actor, licence = %licence.id, "licence uploaded");
    let mut response = axum::Json(json_of(&state, true)).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().map_err(ApiError::internal)?,
    );
    Ok(response)
}
