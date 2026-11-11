//! `/console/api` (09 §10): first-run setup, sign-in and sign-out, the
//! signed-in user, and changing one's password. Everything else the console
//! does goes through `/v1` with the session as a credential (`http::staff`).
//!
//! Sign-in answers an unknown email, a disabled user and a wrong password
//! alike, with `401` and the reason `credentials`, after the same argon2id
//! work (a dummy hash when there is no user). The back-off (`console::guard`) and
//! the per-address budget refuse before any of it, alike for every email.
//! Passwords, emails, session identifiers and CSRF tokens are never logged;
//! the user appears by id (`usr_…`).

use std::net::IpAddr;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use cavea_engine::Clock as _;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::Instrument as _;

use super::auth::ClientIp;
use super::extract::{Json, Secret, check_text, invalid_field};
use super::openapi::{Needs, Op, Routes};
use super::staff::{ConsoleUser, clear_cookie, same_origin, session_cookie, set_cookie};
use super::{ApiError, AppState, Code};
use crate::audit::{self, Entry};
use crate::console::guard::Wait;
use crate::console::password::{self, Busy};
use crate::console::{
    PASSWORD_MAX, Role, SessionId, check_password, normalise_email, sessions, setup,
};
use crate::limiter::{Budget, Limited};

pub(super) fn routes() -> Routes {
    Routes::new()
        .add(
            Op::post(
                "/console/api/setup",
                "consoleSetup",
                "Create the owner with the one-time setup token",
            )
            .about(
                "09 §2. Works while no user exists, once, with the token of the link the \
server logged at start-up (valid one hour; a restart makes a new one). The owner becomes owner \
of every workspace; with none, `workspace` (default `main`) is created. Signs the owner in. A \
wrong, expired or used token, or a server that has an owner, is `401` `setup_token`.",
            )
            .body("ConsoleSetup")
            .reply(201, "ConsoleMe", "The owner, signed in")
            .sets_cookie()
            .errors(&[401, 403, 409, 429]),
            setup,
        )
        .add(
            Op::post("/console/api/session", "consoleSignIn", "Sign in")
                .about(
                    "A new session every time (any presented one is ended). A wrong email or \
password is `401` `credentials`, the same for both; repeated failures wait (`429`, `backoff`, \
`Retry-After`).",
                )
                .body("ConsoleSignIn")
                .reply(200, "ConsoleMe", "Signed in")
                .sets_cookie()
                .errors(&[401, 403, 429]),
            sign_in,
        )
        .add(
            Op::delete("/console/api/session", "consoleSignOut", "Sign out")
                .about("Ends the session on the server and clears the cookie.")
                .needs(Needs::Console)
                .response(204, "Signed out", None)
                .sets_cookie(),
            sign_out,
        )
        .add(
            Op::get("/console/api/me", "consoleMe", "The signed-in user")
                .about("The user, their workspaces and roles, the session's CSRF token.")
                .needs(Needs::Console)
                .reply(200, "ConsoleMe", "The user"),
            me,
        )
        .add(
            Op::post(
                "/console/api/password",
                "consolePassword",
                "Change your password",
            )
            .about(
                "Ends every session of the user, this one included, and starts a new one. \
A wrong current password is `403` `credentials`.",
            )
            .needs(Needs::Console)
            .body("ConsolePassword")
            .reply(200, "ConsoleMe", "Signed in again")
            .sets_cookie()
            .errors(&[429]),
            change_password,
        )
}

fn now_ms(state: &AppState) -> u64 {
    state.seats.clock.now().0
}

/// The per-address budget of sign-in attempts (08 §12).
pub(super) fn budget(state: &AppState, addr: Option<IpAddr>) -> Result<(), ApiError> {
    state
        .limiter
        .check(Budget::SignIn, 0, addr, now_ms(state))
        .map_err(|Limited(seconds)| {
            ApiError::new(Code::RateLimited)
                .with_message("Too many sign-in attempts from this address; retry later.")
                .with_detail("reason", "sign_in")
                .with_retry_after(seconds)
        })
}

fn backoff(Wait(seconds): Wait) -> ApiError {
    ApiError::new(Code::RateLimited)
        .with_message("Too many failed attempts; retry later.")
        .with_detail("reason", "backoff")
        .with_retry_after(seconds)
}

pub(super) fn busy(busy: Busy) -> ApiError {
    match busy {
        Busy::Timeout => ApiError::new(Code::Overloaded)
            .with_message("Too many sign-ins at once; retry shortly.")
            .with_detail("reason", "password_hashing"),
        Busy::Failed(cause) => ApiError::internal(cause),
    }
}

fn credentials() -> ApiError {
    ApiError::new(Code::Unauthorized)
        .with_message("The email or password is wrong.")
        .with_detail("reason", "credentials")
}

const WORKSPACES: &str = "SELECT replace(w.id::text, '-', ''), w.key, w.name, m.role
  FROM memberships m JOIN workspaces w ON w.id = m.workspace_id
 WHERE m.user_id = $1::uuid
 ORDER BY w.created_at, w.id";
const EMAIL: &str = "SELECT email FROM users WHERE id = $1::uuid";

/// `ConsoleMe`: the user, their workspaces with roles and permissions, the
/// CSRF token and the session's ends.
pub(super) async fn me_json(
    state: &AppState,
    id: &SessionId,
    session: &sessions::Found,
) -> Result<Value, ApiError> {
    let read = async {
        let email: String = sqlx::query_scalar(EMAIL)
            .bind(&session.user)
            .fetch_one(&state.db)
            .await?;
        let rows: Vec<(String, String, String, String)> = sqlx::query_as(WORKSPACES)
            .bind(&session.user)
            .fetch_all(&state.db)
            .await?;
        Ok::<_, sqlx::Error>((email, rows))
    };
    let (email, rows) = read
        .instrument(cavea_telemetry::db_span("read_console_user"))
        .await?;
    let workspaces: Vec<Value> = rows
        .into_iter()
        .filter_map(|(ws, key, name, role)| {
            let role = Role::from_db(&role)?;
            Some(json!({
                "id": format!("ws_{ws}"), "key": key, "name": name,
                "role": role.as_str(), "permissions": role.permissions(),
            }))
        })
        .collect();
    Ok(json!({
        "user": { "id": format!("usr_{}", session.user), "email": email },
        "workspaces": workspaces,
        "csrf_token": id.csrf(),
        "session": {
            "expires_at": session.expires_at,
            "absolute_expires_at": session.absolute_expires_at,
        },
    }))
}

/// The reply of a new session: `ConsoleMe` and its cookie.
pub(super) async fn signed_in(
    state: &AppState,
    status: StatusCode,
    id: &SessionId,
) -> Result<Response, ApiError> {
    let session = sessions::find(&state.db, &id.hash(), None)
        .await?
        .ok_or_else(|| ApiError::internal("a new console session was not found"))?;
    let body = me_json(state, id, &session).await?;
    let mut response = (status, axum::Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, set_cookie(state, id)?);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().map_err(ApiError::internal)?,
    );
    Ok(response)
}

pub(super) fn new_session() -> Result<SessionId, ApiError> {
    SessionId::generate().map_err(|e| ApiError::internal(e.to_string()))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Setup {
    token: Secret,
    email: String,
    password: Secret,
    #[serde(default)]
    workspace: Option<FirstWorkspace>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FirstWorkspace {
    key: String,
    name: String,
}

fn setup_refused() -> ApiError {
    ApiError::new(Code::Unauthorized)
        .with_message("The setup link is wrong, expired or used, or setup is done.")
        .with_detail("reason", "setup_token")
}

async fn setup(
    State(state): State<AppState>,
    ClientIp(addr): ClientIp,
    headers: HeaderMap,
    Json(body): Json<Setup>,
) -> Result<Response, ApiError> {
    same_origin(&headers)?;
    budget(&state, addr)?;
    let email = normalise_email(&body.email)
        .ok_or_else(|| invalid_field("email", "must be an email address"))?;
    check_password(&body.password).map_err(|problem| invalid_field("password", problem))?;
    let (key, name) = body
        .workspace
        .as_ref()
        .map_or(("main", "Main"), |w| (w.key.as_str(), w.name.as_str()));
    check_text("key", key)?;
    check_text("name", name)?;
    // Before hashing: a wrong token costs one query, not argon2's work.
    if body.token.len() > 128 || !setup::valid(&state.db, &body.token).await? {
        return Err(setup_refused());
    }
    let hash = password::hash_bounded(body.password.into_inner())
        .await
        .map_err(busy)?;
    let id = new_session()?;
    let owner = setup::complete(
        &state.db,
        &body.token,
        &email,
        &hash,
        (key, name),
        &id.hash(),
    )
    .await
    .map_err(|e| match e {
        setup::SetupError::Refused => setup_refused(),
        setup::SetupError::WorkspaceTaken => ApiError::new(Code::AlreadyExists)
            .with_message("A workspace with this key already exists."),
        setup::SetupError::Db(e) => e.into(),
    })?;
    tracing::info!(user = %format!("usr_{}", owner.user), "console owner created");
    signed_in(&state, StatusCode::CREATED, &id).await
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignIn {
    email: String,
    password: Secret,
}

const FIND_USER: &str = "SELECT replace(id::text, '-', ''), password_hash FROM users
 WHERE email = $1 AND disabled_at IS NULL";

async fn sign_in(
    State(state): State<AppState>,
    ClientIp(addr): ClientIp,
    headers: HeaderMap,
    Json(body): Json<SignIn>,
) -> Result<Response, ApiError> {
    same_origin(&headers)?;
    budget(&state, addr)?;
    // The back-off's key: the email as stored, or as typed when it cannot be
    // one (which matches no account), cut to a bounded length.
    let account = normalise_email(&body.email).unwrap_or_else(|| {
        body.email
            .chars()
            .take(254)
            .collect::<String>()
            .to_lowercase()
    });
    let now = now_ms(&state);
    state.sign_in.check(&account, addr, now).map_err(backoff)?;
    let user: Option<(String, Option<String>)> = sqlx::query_as(FIND_USER)
        .bind(&account)
        .fetch_optional(&state.db)
        .instrument(cavea_telemetry::db_span("find_console_user"))
        .await?;
    // A password longer than any that can be set is wrong; hashing it would
    // only be work for whoever sent it.
    let password =
        (body.password.chars().count() <= PASSWORD_MAX).then_some(body.password.into_inner());
    let stored = user.as_ref().and_then(|(_, hash)| hash.clone());
    let verified = match password {
        Some(password) => password::verify_bounded(password, stored.clone())
            .await
            .map_err(busy)?,
        None => false,
    };
    // The account's id when the email is one's: a row for it, off the
    // reply's path; nothing for an address that is no account's (`audit`).
    let known = user.as_ref().map(|(id, _)| id.clone());
    let Some((user, _)) = user.filter(|_| verified) else {
        state.sign_in.failed(&account, addr, now);
        tracing::info!("console sign-in refused");
        if let Some(known) = known {
            audit::sign_in_failed(&state.db, known);
        }
        return Err(credentials());
    };
    state.sign_in.succeeded(&account, addr, now);
    // Rotation (12 §3 T9): a session presented with the sign-in ends; the
    // user gets a new identifier.
    let presented = session_cookie(&headers, &state).flatten();
    let id = new_session()?;
    let actor = format!("usr_{user}");
    let start = async {
        let mut tx = state.db.begin().await?;
        // The password verified must still be the user's: a change committed
        // meanwhile (which ended every session) refuses this sign-in, and one
        // starting now waits for it and then ends this session too.
        if !still_current(&mut tx, &user, stored.as_deref()).await? {
            return Ok(false);
        }
        if let Some(old) = &presented {
            sessions::revoke(&mut tx, &old.hash()).await?;
        }
        sessions::insert(&mut tx, &id.hash(), &user).await?;
        audit::record(&mut tx, Entry::console(&actor, &user, "console.sign_in")).await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(true)
    };
    let started = start
        .instrument(cavea_telemetry::db_span("start_console_session"))
        .await?;
    if !started {
        tracing::info!("console sign-in refused");
        return Err(credentials());
    }
    tracing::info!(user = %format!("usr_{user}"), "console sign-in");
    signed_in(&state, StatusCode::OK, &id).await
}

async fn sign_out(
    State(state): State<AppState>,
    console: ConsoleUser,
) -> Result<Response, ApiError> {
    let user = &console.session.user;
    let actor = format!("usr_{user}");
    let end = async {
        let mut tx = state.db.begin().await?;
        sessions::revoke(&mut tx, &console.id.hash()).await?;
        audit::record(&mut tx, Entry::console(&actor, user, "console.sign_out")).await?;
        tx.commit().await
    };
    end.instrument(cavea_telemetry::db_span("end_console_session"))
        .await?;
    tracing::info!(user = %actor, "console sign-out");
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, clear_cookie(&state)?);
    Ok(response)
}

async fn me(State(state): State<AppState>, console: ConsoleUser) -> Result<Response, ApiError> {
    let body = me_json(&state, &console.id, &console.session).await?;
    let mut response = axum::Json(body).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().map_err(ApiError::internal)?,
    );
    Ok(response)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PasswordChange {
    current_password: Secret,
    new_password: Secret,
}

const CREDENTIALS: &str = "SELECT email, password_hash FROM users WHERE id = $1::uuid";
const CURRENT_HASH: &str =
    "SELECT password_hash FROM users WHERE id = $1::uuid AND disabled_at IS NULL FOR UPDATE";

/// Whether `user`'s password hash is still `verified`, locking the row until
/// the transaction ends, so that a sign-in and a password change on the same
/// user serialise.
pub(super) async fn still_current(
    tx: &mut sqlx::PgConnection,
    user: &str,
    verified: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let current: Option<Option<String>> = sqlx::query_scalar(CURRENT_HASH)
        .bind(user)
        .fetch_optional(tx)
        .await?;
    Ok(verified.is_some() && current.flatten().as_deref() == verified)
}
const SET_PASSWORD: &str = "UPDATE users SET password_hash = $2 WHERE id = $1::uuid";

async fn change_password(
    State(state): State<AppState>,
    ClientIp(addr): ClientIp,
    console: ConsoleUser,
    Json(body): Json<PasswordChange>,
) -> Result<Response, ApiError> {
    budget(&state, addr)?;
    check_password(&body.new_password).map_err(|problem| invalid_field("new_password", problem))?;
    let user = console.session.user;
    let (email, stored): (String, Option<String>) = sqlx::query_as(CREDENTIALS)
        .bind(&user)
        .fetch_one(&state.db)
        .instrument(cavea_telemetry::db_span("find_console_user"))
        .await?;
    let now = now_ms(&state);
    state.sign_in.check(&email, addr, now).map_err(backoff)?;
    let current = (body.current_password.chars().count() <= PASSWORD_MAX)
        .then_some(body.current_password.into_inner());
    let verified = match current {
        Some(current) => password::verify_bounded(current, stored.clone())
            .await
            .map_err(busy)?,
        None => false,
    };
    if !verified {
        state.sign_in.failed(&email, addr, now);
        return Err(ApiError::new(Code::Forbidden)
            .with_message("The current password is wrong.")
            .with_detail("reason", "credentials"));
    }
    let hash = password::hash_bounded(body.new_password.into_inner())
        .await
        .map_err(busy)?;
    let id = new_session()?;
    let change = async {
        let mut tx = state.db.begin().await?;
        // Another change since the check: the current password is no longer
        // the one verified.
        if !still_current(&mut tx, &user, stored.as_deref()).await? {
            return Ok(false);
        }
        sqlx::query(SET_PASSWORD)
            .bind(&user)
            .bind(&hash)
            .execute(&mut *tx)
            .await?;
        sessions::revoke_all(&mut tx, &user).await?;
        sessions::insert(&mut tx, &id.hash(), &user).await?;
        let actor = format!("usr_{user}");
        let entry = Entry::console(&actor, &user, "console.password_changed");
        audit::record(&mut tx, entry).await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(true)
    };
    let changed = change
        .instrument(cavea_telemetry::db_span("change_console_password"))
        .await?;
    if !changed {
        return Err(ApiError::new(Code::Forbidden)
            .with_message("The current password is wrong.")
            .with_detail("reason", "credentials"));
    }
    tracing::info!(user = %format!("usr_{user}"), "console password changed; every session ended");
    signed_in(&state, StatusCode::OK, &id).await
}
