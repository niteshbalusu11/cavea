//! A console account's second factor and password reset links (09 §2, §10;
//! 12 §3 T9; D-89, D-90; CON-003).
//!
//! **Two-step sign-in.** `POST /console/api/session` verifies the password
//! as before and, for a user with a second factor, answers `202` with a
//! challenge instead of a session: 256 random bits, returned once, stored as
//! SHA-256 with the user and the password hash it was verified against, for
//! [`CHALLENGE_TTL`]. Only a right password makes one, so a challenge cannot
//! skip the password; it is deleted when used, so it cannot be replayed; it
//! dies after [`CHALLENGE_FAILURES`] wrong codes, and every wrong code counts
//! in the sign-in back-off (`console::guard`) as a wrong password does; and a
//! password changed or reset since refuses it. Before the password is
//! verified nothing differs between accounts with and without a second
//! factor: the reply, its status and the work done are the first step's.
//!
//! **Codes.** A TOTP code (`console::totp`) is accepted once: the step it
//! belongs to must be later than the last one used, which the same statement
//! that accepts it records. A recovery code is accepted once: accepting it
//! deletes it.
//!
//! **Reset links.** An owner or admin makes a one-time link for a member, or
//! the admin key for a workspace's owner; nothing is sent. The token is 256
//! bits, stored as SHA-256, for [`RESET_TTL`]; one live link per user (a new
//! one replaces it). Using it sets the password, ends every session of the
//! user and, only when its maker chose so, clears the second factor; then it
//! signs in, or asks for the second step when the user still has one.
//!
//! **Who may reset whom.** An account is the instance's, not a workspace's:
//! an admin of one workspace may reset a member only if they could manage
//! that person in every workspace the person belongs to (owner or admin
//! there, and owner where the person is an owner); otherwise
//! `other_workspaces`. Nobody resets their own account here: that is the
//! password change and turning the factor off, which need the password.
//!
//! Secrets (challenges, TOTP secrets and URIs, recovery codes, reset tokens
//! and links, passwords, codes) are `Secret` in bodies and never logged.

use std::net::IpAddr;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tracing::Instrument as _;

use super::Reason;
use super::auth::{AdminKeyAuth, Auth, ClientIp};
use super::console::{backoff, budget, busy, me_json, new_session, now_ms, signed_in};
use super::extract::{Json, Path, Secret, invalid_field};
use super::openapi::{Needs, Op, Routes};
use super::staff::{ConsoleUser, Member, same_origin, session_cookie};
use super::{ApiError, AppState, Code};
use crate::audit::{self, Entry};
use crate::console::guard::Wait;
use crate::console::{
    PASSWORD_MAX, Permission, Role, check_password, normalise_email, password, sessions, totp,
};

/// How long a sign-in's second step may wait.
pub const CHALLENGE_TTL: Duration = Duration::from_secs(5 * 60);
/// Wrong codes a challenge takes before it is gone.
pub const CHALLENGE_FAILURES: i32 = 5;
/// How long a password reset link works.
pub const RESET_TTL: Duration = Duration::from_secs(3600);
/// The console path a reset link opens; the token is in the fragment.
pub const RESET_PATH: &str = "/console/reset";
/// The issuer an authenticator app shows beside the account.
const ISSUER: &str = "Cavea";

fn secs(d: Duration) -> i64 {
    i64::try_from(d.as_secs()).unwrap_or(i64::MAX)
}

pub(super) fn routes() -> Routes {
    let settings = |op: Op| op.needs(Needs::Member(Permission::Settings));
    Routes::new()
        .add(
            Op::post(
                "/console/api/session/second-factor",
                "consoleSignInSecondFactor",
                "Sign in: the second step",
            )
            .about(
                "With the `challenge` of a sign-in that answered `202`, and a code from the \
authenticator app or an unused recovery code. A wrong code is `401` `second_factor` (after five, \
or once the challenge is used or five minutes old, `401` `challenge`: sign in again); repeated \
failures wait (`429`, `backoff`), as wrong passwords do.",
            )
            .body("ConsoleSecondStep")
            .reply(200, "ConsoleMe", "Signed in")
            .sets_cookie()
            .errors(&[401, 403, 429]),
            second_step,
        )
        .add(
            Op::post(
                "/console/api/totp",
                "consoleTotpEnrol",
                "Start turning on a second factor",
            )
            .about(
                "Needs the password. Returns a new secret, shown only here, as the \
`otpauth://` URI for a QR code and as text; it signs nothing in until confirmed with a code \
(`/console/api/totp/confirm`). Starting again replaces an unconfirmed secret. A user who has a \
second factor is `409` `second_factor`; a wrong password `403` `credentials`.",
            )
            .needs(Needs::Console)
            .body("ConsolePasswordProof")
            .reply(200, "ConsoleTotpSecret", "The secret, shown only here")
            .errors(&[409, 429]),
            enrol,
        )
        .add(
            Op::post(
                "/console/api/totp/confirm",
                "consoleTotpConfirm",
                "Turn on the second factor",
            )
            .about(
                "With a current code of the secret from `/console/api/totp`: turns the \
second factor on and returns ten recovery codes, shown only here, each usable once instead of a \
code. A wrong code is `403` `second_factor`; nothing started `400` `no_second_factor`; a \
second factor already on `409` `second_factor`.",
            )
            .needs(Needs::Console)
            .body("ConsoleCode")
            .reply(
                200,
                "ConsoleRecoveryCodes",
                "The recovery codes, shown only here",
            )
            .errors(&[409, 429]),
            confirm,
        )
        .add(
            Op::post(
                "/console/api/totp/disable",
                "consoleTotpDisable",
                "Turn off the second factor",
            )
            .about(
                "Needs the password and a current code or a recovery code. Removes the \
secret and the recovery codes. A wrong password is `403` `credentials`, a wrong code `403` \
`second_factor`; none on `400` `no_second_factor`.",
            )
            .needs(Needs::Console)
            .body("ConsoleTotpDisable")
            .reply(200, "ConsoleMe", "The user, without a second factor")
            .errors(&[429]),
            disable,
        )
        .add(
            settings(Op::delete(
                "/console/api/members/{user}/second-factor",
                "resetMemberSecondFactor",
                "Reset a member's second factor",
            ))
            .about(
                "For someone who lost their authenticator: removes their secret and recovery \
codes; they sign in with the password alone and may enrol again. Resetting an owner needs \
`ownership`. The person must be one the caller may manage in every workspace they belong to \
(`403` `other_workspaces`), and not the caller (`403` `own_account`). Idempotent.",
            )
            .response(204, "Reset", None),
            reset_member_second_factor,
        )
        .add(
            settings(Op::post(
                "/console/api/members/{user}/password-reset",
                "createMemberPasswordReset",
                "Make a password reset link for a member",
            ))
            .about(
                "A one-time link, shown only here (nothing is sent), that sets the member's \
password, for one hour; it replaces any earlier link for them. Using it ends every session of \
theirs; it clears their second factor only with `clear_second_factor`. The same rules as \
resetting a second factor.",
            )
            .body("CreatePasswordReset")
            .reply(201, "PasswordResetCreated", "The link, shown only here"),
            member_password_reset,
        )
        .add(
            Op::post(
                "/v1/workspaces/{workspace}/password-resets",
                "createOwnerPasswordReset",
                "Make a password reset link for a workspace's owner",
            )
            .about(
                "The operator's recovery for an owner who cannot sign in: a one-time link for \
`email`, an owner of the workspace, as an owner or admin makes for a member. With \
`clear_second_factor`, using it also removes their second factor. An email that is not an \
owner's is `404` (`missing: \"owner\"`).",
            )
            .needs(Needs::Admin)
            .body("CreateOwnerPasswordReset")
            .reply(201, "PasswordResetCreated", "The link, shown only here"),
            owner_password_reset,
        )
        .add(
            Op::post(
                "/console/api/password-reset",
                "consolePasswordReset",
                "Set a new password with a reset link",
            )
            .about(
                "With the token of a reset link and the new password. Ends every session of \
the user, then signs in (`200`), or answers `202` with the second step's challenge while the \
user has a second factor. A wrong, expired or used token is `401` `reset_token`.",
            )
            .body("ConsolePasswordReset")
            .reply(200, "ConsoleMe", "Signed in")
            .sets_cookie()
            .reply(
                202,
                "ConsoleSecondFactor",
                "The password is set; now the second step",
            )
            .errors(&[401, 403, 429]),
            use_reset,
        )
}

fn hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// 64 lowercase hex digits: the form of every token here.
fn well_formed(token: &str) -> bool {
    token.len() == 64
        && token
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn new_token() -> Result<String, ApiError> {
    crate::console::setup::token().map_err(|e| ApiError::internal(e.to_string()))
}

// ---- the second step of sign-in ----

const SWEEP_CHALLENGES: &str =
    "DELETE FROM console_challenges WHERE user_id = $1::uuid AND expires_at <= now()";
const INSERT_CHALLENGE: &str = concat!(
    "INSERT INTO console_challenges (id_hash, user_id, password_hash, expires_at)
VALUES ($1, $2::uuid, $3, now() + $4 * interval '1 second')
RETURNING ",
    rfc3339!("expires_at")
);
const END_CHALLENGES: &str = "DELETE FROM console_challenges WHERE user_id = $1::uuid";

/// A challenge for `user`, whose password was just verified against
/// `password_hash`, inside the caller's transaction: the token, shown once,
/// and when it ends.
pub(super) async fn challenge(
    tx: &mut sqlx::PgConnection,
    user: &str,
    password_hash: &str,
) -> Result<(Secret, String), ApiError> {
    sqlx::query(SWEEP_CHALLENGES)
        .bind(user)
        .execute(&mut *tx)
        .await?;
    let token = new_token()?;
    let expires_at: String = sqlx::query_scalar(INSERT_CHALLENGE)
        .bind(hash(&token).as_slice())
        .bind(user)
        .bind(password_hash)
        .bind(secs(CHALLENGE_TTL))
        .fetch_one(&mut *tx)
        .await?;
    Ok((Secret::new(token), expires_at))
}

/// `202` with the second step's challenge.
pub(super) fn second_step_reply(
    challenge: Secret,
    expires_at: String,
) -> Result<Response, ApiError> {
    let mut response = (
        StatusCode::ACCEPTED,
        axum::Json(json!({ "challenge": challenge.as_str(), "expires_at": expires_at })),
    )
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().map_err(ApiError::internal)?,
    );
    Ok(response)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SecondStep {
    challenge: Secret,
    code: Secret,
}

fn challenge_refused() -> ApiError {
    ApiError::from(Reason::Challenge)
        .with_message("This sign-in has ended or was used: sign in again.")
}

/// The challenge's user, unlocked: the user's row is locked first, as every
/// change to a user's factor or challenges locks it (no lock cycle with a
/// reset, which locks the user and then deletes the challenges).
const CHALLENGE_USER: &str =
    "SELECT replace(user_id::text, '-', '') FROM console_challenges WHERE id_hash = $1";
const LOCK_USER_ROW: &str = "SELECT FROM users WHERE id = $1::uuid FOR UPDATE";
/// The challenge, its user and what the second step checks, the challenge
/// locked after the user.
const FIND_CHALLENGE: &str =
    "SELECT replace(c.user_id::text, '-', ''), c.password_hash = u.password_hash,
       c.failures, u.email, u.totp_secret, u.totp_last_step
  FROM console_challenges c JOIN users u ON u.id = c.user_id
 WHERE c.id_hash = $1 AND c.user_id = $2::uuid AND c.expires_at > now()
   AND u.disabled_at IS NULL AND u.totp_enabled_at IS NOT NULL
   FOR UPDATE OF c";
const FAILED_CHALLENGE: &str =
    "UPDATE console_challenges SET failures = failures + 1 WHERE id_hash = $1";
const USE_CHALLENGE: &str = "DELETE FROM console_challenges WHERE id_hash = $1";

type ChallengeRow = (
    String,
    Option<bool>,
    i32,
    String,
    Option<Vec<u8>>,
    Option<i64>,
);

/// How a code was accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Used {
    Totp,
    /// A recovery code, with how many are left.
    Recovery(i64),
}

impl Used {
    fn as_str(self) -> &'static str {
        match self {
            Self::Totp => "totp",
            Self::Recovery(_) => "recovery_code",
        }
    }
}

const TAKE_STEP: &str = "UPDATE users SET totp_last_step = $2
 WHERE id = $1::uuid AND totp_enabled_at IS NOT NULL
   AND (totp_last_step IS NULL OR totp_last_step < $2)";
const TAKE_RECOVERY: &str =
    "DELETE FROM totp_recovery_codes WHERE user_id = $1::uuid AND code_hash = $2";
const RECOVERY_LEFT: &str = "SELECT count(*) FROM totp_recovery_codes WHERE user_id = $1::uuid";

/// Accept `code` for `user` inside `tx`, once: a TOTP code of a step later
/// than the last one used (recorded by the same statement), or an unused
/// recovery code (deleted). `None` for a wrong or used code.
async fn accept_code(
    tx: &mut sqlx::PgConnection,
    state: &AppState,
    user: &str,
    secret: &[u8],
    last_step: Option<i64>,
    code: &str,
) -> Result<Option<Used>, ApiError> {
    let code = totp::normalise(code);
    if totp::is_totp(&code) {
        let after = last_step.and_then(|s| u64::try_from(s).ok());
        let Some(step) = totp::verify(secret, &code, now_ms(state), after) else {
            return Ok(None);
        };
        let step = i64::try_from(step).map_err(ApiError::internal)?;
        let taken = sqlx::query(TAKE_STEP)
            .bind(user)
            .bind(step)
            .execute(&mut *tx)
            .await?;
        return Ok((taken.rows_affected() == 1).then_some(Used::Totp));
    }
    // A recovery code is ten characters; anything far longer is no code.
    if code.is_empty() || code.len() > 32 {
        return Ok(None);
    }
    let taken = sqlx::query(TAKE_RECOVERY)
        .bind(user)
        .bind(totp::recovery_hash(&code).as_slice())
        .execute(&mut *tx)
        .await?;
    if taken.rows_affected() == 0 {
        return Ok(None);
    }
    let left: i64 = sqlx::query_scalar(RECOVERY_LEFT)
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
    Ok(Some(Used::Recovery(left)))
}

/// The audit row of a recovery code used, with how many are left.
async fn record_recovery(
    tx: &mut sqlx::PgConnection,
    user: &str,
    used: Used,
) -> Result<(), sqlx::Error> {
    if let Used::Recovery(left) = used {
        let actor = format!("usr_{user}");
        let entry = Entry::console(&actor, user, "console.recovery_code_used")
            .detail(json!({ "remaining": left }));
        audit::record(tx, entry).await?;
    }
    Ok(())
}

/// What the second step decided.
enum Step {
    SignedIn(String),
    Wrong(String, String),
    Wait(Wait),
}

async fn second_step(
    State(state): State<AppState>,
    ClientIp(addr): ClientIp,
    headers: HeaderMap,
    Json(body): Json<SecondStep>,
) -> Result<Response, ApiError> {
    same_origin(&headers)?;
    budget(&state, addr)?;
    if !well_formed(&body.challenge) {
        return Err(challenge_refused());
    }
    let id_hash = hash(&body.challenge);
    // Rotation (12 §3 T9), as for a one-step sign-in: a session presented
    // with the sign-in ends when it completes.
    let presented = session_cookie(&headers, &state).flatten();
    let session = new_session()?;
    let now = now_ms(&state);
    let work = async {
        let mut tx = state.db.begin().await?;
        let owner: Option<String> = sqlx::query_scalar(CHALLENGE_USER)
            .bind(id_hash.as_slice())
            .fetch_optional(&mut *tx)
            .await?;
        let owner = owner.ok_or_else(challenge_refused)?;
        sqlx::query(LOCK_USER_ROW)
            .bind(&owner)
            .execute(&mut *tx)
            .await?;
        let row: Option<ChallengeRow> = sqlx::query_as(FIND_CHALLENGE)
            .bind(id_hash.as_slice())
            .bind(&owner)
            .fetch_optional(&mut *tx)
            .await?;
        let Some((user, current, failures, email, secret, last_step)) = row else {
            return Err(challenge_refused());
        };
        if let Err(wait) = state.sign_in.check(&email, addr, now) {
            return Ok(Step::Wait(wait));
        }
        // The password changed or was reset since: start again.
        if current != Some(true) || failures >= CHALLENGE_FAILURES {
            sqlx::query(USE_CHALLENGE)
                .bind(id_hash.as_slice())
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Err(challenge_refused());
        }
        let secret = secret.unwrap_or_default();
        let used = accept_code(&mut tx, &state, &user, &secret, last_step, &body.code).await?;
        let Some(used) = used else {
            // A refused code changed nothing but this count.
            sqlx::query(FAILED_CHALLENGE)
                .bind(id_hash.as_slice())
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(Step::Wrong(user, email));
        };
        sqlx::query(USE_CHALLENGE)
            .bind(id_hash.as_slice())
            .execute(&mut *tx)
            .await?;
        if let Some(old) = &presented {
            sessions::revoke(&mut tx, &old.hash()).await?;
        }
        sessions::insert(&mut tx, &session.hash(), &user).await?;
        let actor = format!("usr_{user}");
        let entry = Entry::console(&actor, &user, "console.sign_in")
            .detail(json!({ "second_factor": used.as_str() }));
        audit::record(&mut tx, entry).await?;
        record_recovery(&mut tx, &user, used).await?;
        tx.commit().await?;
        Ok(Step::SignedIn(email))
    };
    let step = work
        .instrument(cavea_telemetry::db_span("console_second_step"))
        .await?;
    match step {
        Step::Wait(wait) => Err(backoff(wait)),
        Step::Wrong(user, email) => {
            state.sign_in.failed(&email, addr, now);
            tracing::info!("console sign-in refused at the second step");
            audit::sign_in_failed(&state.db, user);
            Err(ApiError::from(Reason::SecondFactor).with_message("The code is wrong."))
        }
        Step::SignedIn(email) => {
            state.sign_in.succeeded(&email, addr, now);
            tracing::info!("console sign-in with a second factor");
            signed_in(&state, StatusCode::OK, &session).await
        }
    }
}

// ---- enrolment, confirmation, turning off ----

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PasswordProof {
    password: Secret,
}

const ACCOUNT: &str = "SELECT email, password_hash, totp_enabled_at IS NOT NULL, totp_secret,
       totp_last_step
  FROM users WHERE id = $1::uuid";

type AccountRow = (String, Option<String>, bool, Option<Vec<u8>>, Option<i64>);

async fn account(state: &AppState, user: &str) -> Result<AccountRow, ApiError> {
    Ok(sqlx::query_as(ACCOUNT)
        .bind(user)
        .fetch_one(&state.db)
        .instrument(cavea_telemetry::db_span("find_console_user"))
        .await?)
}

/// The signed-in user's password, checked as a password change checks it:
/// the back-off first, a failure counted.
async fn check_own_password(
    state: &AppState,
    addr: Option<IpAddr>,
    email: &str,
    stored: Option<String>,
    password: Secret,
) -> Result<(), ApiError> {
    let now = now_ms(state);
    state.sign_in.check(email, addr, now).map_err(backoff)?;
    let given = (password.chars().count() <= PASSWORD_MAX).then_some(password.into_inner());
    let verified = match given {
        Some(given) => password::verify_bounded(given, stored)
            .await
            .map_err(busy)?,
        None => false,
    };
    if !verified {
        state.sign_in.failed(email, addr, now);
        return Err(ApiError::from(Reason::CurrentPassword).with_message("The password is wrong."));
    }
    Ok(())
}

fn already_on() -> ApiError {
    ApiError::from(Reason::SecondFactorEnabled)
        .with_message("A second factor is already on: turn it off first.")
}

fn none_on() -> ApiError {
    ApiError::from(Reason::NoSecondFactor).with_message("There is no second factor to do this to.")
}

fn wrong_code() -> ApiError {
    ApiError::from(Reason::WrongCode).with_message("The code is wrong.")
}

/// `ConsoleTotpSecret`.
#[derive(Debug, Serialize)]
struct TotpSecret {
    /// The secret in base32, for typing in.
    secret: Secret,
    /// `otpauth://totp/…`, for a QR code.
    uri: Secret,
    algorithm: &'static str,
    digits: usize,
    period: u64,
}

const START_ENROLMENT: &str = "UPDATE users SET totp_secret = $2, totp_last_step = NULL
 WHERE id = $1::uuid AND totp_enabled_at IS NULL";

async fn enrol(
    State(state): State<AppState>,
    ClientIp(addr): ClientIp,
    console: ConsoleUser,
    Json(body): Json<PasswordProof>,
) -> Result<Response, ApiError> {
    budget(&state, addr)?;
    let user = console.session.user;
    let (email, stored, on, ..) = account(&state, &user).await?;
    check_own_password(&state, addr, &email, stored, body.password).await?;
    if on {
        return Err(already_on());
    }
    let secret = totp::secret().map_err(|e| ApiError::internal(e.to_string()))?;
    let started = sqlx::query(START_ENROLMENT)
        .bind(&user)
        .bind(secret.as_slice())
        .execute(&state.db)
        .instrument(cavea_telemetry::db_span("start_totp_enrolment"))
        .await?;
    if started.rows_affected() == 0 {
        return Err(already_on());
    }
    tracing::info!(user = %format!("usr_{user}"), "second factor enrolment started");
    let body = TotpSecret {
        secret: Secret::new(totp::base32(&secret)),
        uri: Secret::new(totp::uri(ISSUER, &email, &secret)),
        algorithm: "SHA1",
        digits: totp::DIGITS,
        period: totp::STEP_MS / 1000,
    };
    no_store(axum::Json(body))
}

fn no_store(body: impl IntoResponse) -> Result<Response, ApiError> {
    let mut response = body.into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().map_err(ApiError::internal)?,
    );
    Ok(response)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CodeBody {
    code: Secret,
}

const PENDING: &str = "SELECT email, totp_secret, totp_enabled_at IS NOT NULL
  FROM users WHERE id = $1::uuid FOR UPDATE";
const TURN_ON: &str = "UPDATE users SET totp_enabled_at = now(), totp_last_step = $2
 WHERE id = $1::uuid";
const CLEAR_RECOVERY: &str = "DELETE FROM totp_recovery_codes WHERE user_id = $1::uuid";
const INSERT_RECOVERY: &str =
    "INSERT INTO totp_recovery_codes (user_id, code_hash) SELECT $1::uuid, unnest($2::bytea[])";

/// What a confirmation decided.
enum Confirmed {
    On(Vec<String>),
    Wrong(String),
}

async fn confirm(
    State(state): State<AppState>,
    ClientIp(addr): ClientIp,
    console: ConsoleUser,
    Json(body): Json<CodeBody>,
) -> Result<Response, ApiError> {
    budget(&state, addr)?;
    let user = console.session.user;
    let now = now_ms(&state);
    let codes = totp::recovery_codes().map_err(|e| ApiError::internal(e.to_string()))?;
    let work = async {
        let mut tx = state.db.begin().await?;
        let (email, secret, on): (String, Option<Vec<u8>>, bool) = sqlx::query_as(PENDING)
            .bind(&user)
            .fetch_one(&mut *tx)
            .await?;
        state.sign_in.check(&email, addr, now).map_err(backoff)?;
        if on {
            return Err(already_on());
        }
        let secret = secret.ok_or_else(none_on)?;
        let code = totp::normalise(&body.code);
        let Some(step) = totp::verify(&secret, &code, now, None) else {
            return Ok(Confirmed::Wrong(email));
        };
        sqlx::query(TURN_ON)
            .bind(&user)
            .bind(i64::try_from(step).map_err(ApiError::internal)?)
            .execute(&mut *tx)
            .await?;
        sqlx::query(CLEAR_RECOVERY)
            .bind(&user)
            .execute(&mut *tx)
            .await?;
        let hashes: Vec<Vec<u8>> = codes
            .iter()
            .map(|c| totp::recovery_hash(c).to_vec())
            .collect();
        sqlx::query(INSERT_RECOVERY)
            .bind(&user)
            .bind(hashes)
            .execute(&mut *tx)
            .await?;
        let actor = format!("usr_{user}");
        let entry = Entry::console(&actor, &user, "console.second_factor_enabled");
        audit::record(&mut tx, entry).await?;
        tx.commit().await?;
        Ok(Confirmed::On(codes))
    };
    let confirmed = work
        .instrument(cavea_telemetry::db_span("confirm_totp"))
        .await?;
    match confirmed {
        Confirmed::Wrong(email) => {
            state.sign_in.failed(&email, addr, now);
            Err(wrong_code())
        }
        Confirmed::On(codes) => {
            tracing::info!(user = %format!("usr_{user}"), "second factor on");
            let codes: Vec<Secret> = codes.into_iter().map(Secret::new).collect();
            no_store(axum::Json(json!({ "recovery_codes": codes })))
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Disable {
    password: Secret,
    code: Secret,
}

const LOCK_ACCOUNT: &str = "SELECT password_hash, totp_enabled_at IS NOT NULL, totp_secret,
       totp_last_step
  FROM users WHERE id = $1::uuid FOR UPDATE";
const TURN_OFF: &str = "UPDATE users
   SET totp_secret = NULL, totp_enabled_at = NULL, totp_last_step = NULL
 WHERE id = $1::uuid";

/// Remove `user`'s second factor inside `tx`: the secret, the recovery
/// codes and any sign-in waiting for its second step.
async fn clear_second_factor(tx: &mut sqlx::PgConnection, user: &str) -> Result<(), sqlx::Error> {
    sqlx::query(TURN_OFF).bind(user).execute(&mut *tx).await?;
    sqlx::query(CLEAR_RECOVERY)
        .bind(user)
        .execute(&mut *tx)
        .await?;
    sqlx::query(END_CHALLENGES)
        .bind(user)
        .execute(&mut *tx)
        .await?;
    Ok(())
}

async fn disable(
    State(state): State<AppState>,
    ClientIp(addr): ClientIp,
    console: ConsoleUser,
    Json(body): Json<Disable>,
) -> Result<Response, ApiError> {
    budget(&state, addr)?;
    let user = console.session.user.clone();
    let (email, stored, on, ..) = account(&state, &user).await?;
    check_own_password(&state, addr, &email, stored.clone(), body.password).await?;
    if !on {
        return Err(none_on());
    }
    let now = now_ms(&state);
    let work = async {
        let mut tx = state.db.begin().await?;
        // Read again under the user's lock: the password verified must still
        // be theirs, and the code is checked against the factor they have
        // now, not one an admin reset and they replaced meanwhile.
        let (current, on, secret, last_step): (Option<String>, bool, Option<Vec<u8>>, Option<i64>) =
            sqlx::query_as(LOCK_ACCOUNT)
                .bind(&user)
                .fetch_one(&mut *tx)
                .await?;
        if current.is_none() || current != stored {
            return Err(
                ApiError::from(Reason::CurrentPassword).with_message("The password is wrong.")
            );
        }
        if !on {
            return Err(none_on());
        }
        let secret = secret.unwrap_or_default();
        let used = accept_code(&mut tx, &state, &user, &secret, last_step, &body.code).await?;
        let Some(used) = used else {
            return Ok(false);
        };
        record_recovery(&mut tx, &user, used).await?;
        clear_second_factor(&mut tx, &user).await?;
        let actor = format!("usr_{user}");
        let entry = Entry::console(&actor, &user, "console.second_factor_disabled");
        audit::record(&mut tx, entry).await?;
        tx.commit().await?;
        Ok::<_, ApiError>(true)
    };
    let off = work
        .instrument(cavea_telemetry::db_span("disable_totp"))
        .await?;
    if !off {
        state.sign_in.failed(&email, addr, now);
        return Err(wrong_code());
    }
    tracing::info!(user = %format!("usr_{user}"), "second factor off");
    let body = me_json(&state, &console.id, &console.session).await?;
    no_store(axum::Json(body))
}

// ---- an owner's or admin's resets ----

/// `usr_` and 32 hex digits, as 32 hex digits; else `404`.
fn user_uuid(id: &str) -> Result<&str, ApiError> {
    id.strip_prefix("usr_")
        .filter(|hex| {
            hex.len() == 32 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
        .ok_or_else(not_a_member)
}

fn not_a_member() -> ApiError {
    ApiError::new(Code::NotFound).with_detail("missing", "member")
}

const LOCK_WORKSPACE: &str = "SELECT FROM workspaces WHERE id = $1::uuid FOR UPDATE";
const ROLE_OF: &str =
    "SELECT role FROM memberships WHERE workspace_id = $1::uuid AND user_id = $2::uuid";
/// Workspaces other than `$3` where `$1` is a member whom `$2` may not
/// manage: `$2` is not an owner or admin there, or `$1` is an owner there
/// and `$2` is not.
const OUT_OF_REACH: &str = "SELECT EXISTS (
  SELECT FROM memberships t
    LEFT JOIN memberships a ON a.workspace_id = t.workspace_id AND a.user_id = $2::uuid
   WHERE t.user_id = $1::uuid AND t.workspace_id <> $3::uuid
     AND (a.role IS NULL OR a.role NOT IN ('owner', 'admin')
          OR (t.role = 'owner' AND a.role <> 'owner')))";

/// Whether the caller may reset `user`'s account, under the workspace's
/// lock (09 §2, D-90): a member of this workspace, not the caller, an
/// owner only by an owner, and one the caller could manage in every other
/// workspace they belong to.
async fn may_reset(tx: &mut sqlx::PgConnection, auth: &Auth, user: &str) -> Result<(), ApiError> {
    let ws = auth.workspace().uuid();
    let me = auth.user().ok_or_else(|| ApiError::new(Code::Forbidden))?;
    if me == user {
        return Err(ApiError::from(Reason::OwnAccount).with_message(
            "Change your own password, or turn off your own second factor, from your account.",
        ));
    }
    sqlx::query(LOCK_WORKSPACE)
        .bind(ws)
        .execute(&mut *tx)
        .await?;
    let role: Option<String> = sqlx::query_scalar(ROLE_OF)
        .bind(ws)
        .bind(user)
        .fetch_optional(&mut *tx)
        .await?;
    let role = role
        .as_deref()
        .and_then(Role::from_db)
        .ok_or_else(not_a_member)?;
    if role == Role::Owner && !auth.role().is_some_and(|r| r.allows(Permission::Ownership)) {
        return Err(ApiError::from(Reason::Role)
            .with_message("Only an owner can reset an owner.")
            .with_detail("permission", Permission::Ownership.as_str()));
    }
    let out_of_reach: bool = sqlx::query_scalar(OUT_OF_REACH)
        .bind(user)
        .bind(me)
        .bind(ws)
        .fetch_one(&mut *tx)
        .await?;
    if out_of_reach {
        return Err(ApiError::from(Reason::OtherWorkspaces).with_message(
            "This person belongs to a workspace where you cannot manage them; ask an owner there.",
        ));
    }
    Ok(())
}

async fn reset_member_second_factor(
    Member(auth): Member,
    Path(user): Path<String>,
    State(state): State<AppState>,
) -> Result<StatusCode, ApiError> {
    let user = user_uuid(&user)?;
    let actor = auth.actor();
    let target = format!("usr_{user}");
    let work = async {
        let mut tx = state.db.begin().await?;
        may_reset(&mut tx, &auth, user).await?;
        let had: bool = sqlx::query_scalar(
            "SELECT totp_secret IS NOT NULL FROM users WHERE id = $1::uuid FOR UPDATE",
        )
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
        if had {
            clear_second_factor(&mut tx, user).await?;
            let entry = Entry::by(&auth, &actor, "member.second_factor_reset").target(&target);
            audit::record(&mut tx, entry).await?;
        }
        tx.commit().await?;
        Ok::<_, ApiError>(had)
    };
    let had = work
        .instrument(cavea_telemetry::db_span("reset_second_factor"))
        .await?;
    if had {
        tracing::info!(member = %target, by = %actor, "member's second factor reset");
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---- password reset links ----

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateReset {
    #[serde(default)]
    clear_second_factor: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateOwnerReset {
    email: String,
    #[serde(default)]
    clear_second_factor: bool,
}

/// `PasswordResetCreated`.
#[derive(Debug, Serialize)]
struct ResetCreated {
    reset: Value,
    /// The one-time link, shown only here.
    link: Secret,
}

const REPLACE_RESET: &str = "DELETE FROM password_resets WHERE user_id = $1::uuid";
const INSERT_RESET: &str = concat!(
    "INSERT INTO password_resets
  (id, user_id, token_hash, clear_second_factor, created_by, expires_at)
VALUES (gen_random_uuid(), $1::uuid, $2, $3, $4, now() + $5 * interval '1 second')
RETURNING replace(id::text, '-', ''), ",
    rfc3339!("expires_at")
);

/// A reset link for `user`, replacing any earlier one, inside `tx`, with
/// its audit row in `workspace`.
async fn create_reset(
    tx: &mut sqlx::PgConnection,
    state: &AppState,
    entry: Entry<'_>,
    user: &str,
    clear: bool,
) -> Result<ResetCreated, ApiError> {
    sqlx::query(REPLACE_RESET)
        .bind(user)
        .execute(&mut *tx)
        .await?;
    let token = new_token()?;
    let (id, expires_at): (String, String) = sqlx::query_as(INSERT_RESET)
        .bind(user)
        .bind(hash(&token).as_slice())
        .bind(clear)
        .bind(entry.actor)
        .bind(secs(RESET_TTL))
        .fetch_one(&mut *tx)
        .await?;
    let id = format!("pwr_{id}");
    let target = format!("usr_{user}");
    let entry = Entry {
        target: Some(&target),
        detail: Some(json!({ "reset": id, "clear_second_factor": clear })),
        ..entry
    };
    audit::record(tx, entry).await?;
    let base = state.config.public_url.as_ref().map_or("", |u| u.as_str());
    let link = format!("{}{RESET_PATH}#token={token}", base.trim_end_matches('/'));
    Ok(ResetCreated {
        reset: json!({
            "id": id,
            "user": target,
            "expires_at": expires_at,
            "clear_second_factor": clear,
        }),
        link: Secret::new(link),
    })
}

async fn member_password_reset(
    Member(auth): Member,
    Path(user): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<CreateReset>,
) -> Result<(StatusCode, axum::Json<ResetCreated>), ApiError> {
    let user = user_uuid(&user)?;
    let actor = auth.actor();
    let work = async {
        let mut tx = state.db.begin().await?;
        may_reset(&mut tx, &auth, user).await?;
        let entry = Entry::by(&auth, &actor, "member.password_reset_created");
        let created = create_reset(&mut tx, &state, entry, user, body.clear_second_factor).await?;
        tx.commit().await?;
        Ok::<_, ApiError>(created)
    };
    let created = work
        .instrument(cavea_telemetry::db_span("create_password_reset"))
        .await?;
    tracing::info!(member = %format!("usr_{user}"), by = %actor, "password reset link made");
    Ok((StatusCode::CREATED, axum::Json(created)))
}

const OWNER: &str = "SELECT replace(m.user_id::text, '-', '')
  FROM memberships m JOIN users u ON u.id = m.user_id
 WHERE m.workspace_id = $1::uuid AND u.email = $2 AND m.role = 'owner'";

async fn owner_password_reset(
    _: AdminKeyAuth,
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<CreateOwnerReset>,
) -> Result<(StatusCode, axum::Json<ResetCreated>), ApiError> {
    let no_workspace = || {
        ApiError::new(Code::NotFound)
            .with_message("No such workspace.")
            .with_detail("missing", "workspace")
    };
    let workspace = super::workspaces::workspace_uuid(&id).ok_or_else(no_workspace)?;
    let email = normalise_email(&body.email)
        .ok_or_else(|| invalid_field("email", "must be an email address"))?;
    let work = async {
        let mut tx = state.db.begin().await?;
        let found: Option<()> = sqlx::query_as(LOCK_WORKSPACE)
            .bind(workspace)
            .fetch_optional(&mut *tx)
            .await?;
        found.ok_or_else(no_workspace)?;
        let user: Option<String> = sqlx::query_scalar(OWNER)
            .bind(workspace)
            .bind(&email)
            .fetch_optional(&mut *tx)
            .await?;
        let user = user.ok_or_else(|| {
            ApiError::new(Code::NotFound)
                .with_message("No owner of this workspace has this email.")
                .with_detail("missing", "owner")
        })?;
        let entry = Entry {
            workspace: Some(workspace),
            actor: "admin",
            user: None,
            action: "member.password_reset_created",
            target: None,
            detail: None,
        };
        let created = create_reset(&mut tx, &state, entry, &user, body.clear_second_factor).await?;
        tx.commit().await?;
        Ok::<_, ApiError>((user, created))
    };
    let (user, created) = work
        .instrument(cavea_telemetry::db_span("create_password_reset"))
        .await?;
    cavea_telemetry::record_workspace(&id);
    tracing::info!(member = %format!("usr_{user}"), by = "admin", "password reset link made");
    Ok((StatusCode::CREATED, axum::Json(created)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UseReset {
    token: Secret,
    password: Secret,
}

fn reset_refused() -> ApiError {
    ApiError::from(Reason::ResetToken)
        .with_message("The reset link is wrong, expired or used: ask for a new one.")
}

const LIVE_RESET: &str =
    "SELECT EXISTS (SELECT FROM password_resets WHERE token_hash = $1 AND expires_at > now())";
/// Taking the link is what makes it single-use: of two uses, one deletes it.
const TAKE_RESET: &str = "DELETE FROM password_resets WHERE token_hash = $1 AND expires_at > now()
RETURNING replace(user_id::text, '-', ''), clear_second_factor";
const LOCK_USER: &str = "SELECT totp_enabled_at IS NOT NULL FROM users
 WHERE id = $1::uuid AND disabled_at IS NULL FOR UPDATE";
const SET_PASSWORD: &str = "UPDATE users SET password_hash = $2 WHERE id = $1::uuid";

/// What using a reset link ended in.
enum Reset {
    SignedIn,
    SecondStep(Secret, String),
}

async fn use_reset(
    State(state): State<AppState>,
    ClientIp(addr): ClientIp,
    headers: HeaderMap,
    Json(body): Json<UseReset>,
) -> Result<Response, ApiError> {
    same_origin(&headers)?;
    budget(&state, addr)?;
    if !well_formed(&body.token) {
        return Err(reset_refused());
    }
    check_password(&body.password).map_err(|problem| invalid_field("password", problem))?;
    let token_hash = hash(&body.token);
    // Before hashing: a wrong token costs one query, not argon2's work.
    let live: bool = sqlx::query_scalar(LIVE_RESET)
        .bind(token_hash.as_slice())
        .fetch_one(&state.db)
        .instrument(cavea_telemetry::db_span("check_password_reset"))
        .await?;
    if !live {
        return Err(reset_refused());
    }
    let password_hash = password::hash_bounded(body.password.into_inner())
        .await
        .map_err(busy)?;
    let session = new_session()?;
    let work = async {
        let mut tx = state.db.begin().await?;
        let taken: Option<(String, bool)> = sqlx::query_as(TAKE_RESET)
            .bind(token_hash.as_slice())
            .fetch_optional(&mut *tx)
            .await?;
        let (user, clear) = taken.ok_or_else(reset_refused)?;
        let on: Option<bool> = sqlx::query_scalar(LOCK_USER)
            .bind(&user)
            .fetch_optional(&mut *tx)
            .await?;
        let on = on.ok_or_else(reset_refused)?;
        sqlx::query(SET_PASSWORD)
            .bind(&user)
            .bind(&password_hash)
            .execute(&mut *tx)
            .await?;
        sessions::revoke_all(&mut tx, &user).await?;
        sqlx::query(END_CHALLENGES)
            .bind(&user)
            .execute(&mut *tx)
            .await?;
        if clear {
            clear_second_factor(&mut tx, &user).await?;
        }
        let actor = format!("usr_{user}");
        let entry = Entry::console(&actor, &user, "console.password_reset")
            .detail(json!({ "second_factor_cleared": clear && on }));
        audit::record(&mut tx, entry).await?;
        let reset = if on && !clear {
            let (challenge, expires_at) = challenge(&mut tx, &user, &password_hash).await?;
            Reset::SecondStep(challenge, expires_at)
        } else {
            sessions::insert(&mut tx, &session.hash(), &user).await?;
            let entry = Entry::console(&actor, &user, "console.sign_in");
            audit::record(&mut tx, entry).await?;
            Reset::SignedIn
        };
        tx.commit().await?;
        Ok::<_, ApiError>((user, reset))
    };
    let (user, reset) = work
        .instrument(cavea_telemetry::db_span("use_password_reset"))
        .await?;
    tracing::info!(user = %format!("usr_{user}"), "password reset; every session ended");
    match reset {
        Reset::SignedIn => signed_in(&state, StatusCode::OK, &session).await,
        Reset::SecondStep(challenge, expires_at) => second_step_reply(challenge, expires_at),
    }
}
