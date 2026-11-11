//! The console's people (09 §2, 08 §5, 12 §3 T9): roles and what each may
//! do, emails, passwords, console sessions and the first-run setup token.
//! The HTTP side is `http::console` (the `/console/api` routes) and
//! `http::staff` (the cookie, CSRF and origin checks).
//!
//! A console session is not the buyer's session token (08 §11, D-34): it is
//! a staff member's sign-in, carried in a cookie. Names here say "console
//! session" to keep the two apart.

pub mod guard;
pub mod password;
pub mod sessions;
pub mod setup;

use std::fmt;
use std::time::Duration;

use sha2::{Digest as _, Sha256};

/// A console session ends this long after its last use (09 §2: "valid 12
/// hours, renewed on activity").
pub const IDLE: Duration = Duration::from_secs(12 * 3600);
/// …and in any case this long after sign-in, however active.
pub const ABSOLUTE: Duration = Duration::from_secs(7 * 24 * 3600);
/// A session is renewed at most this often, so that a busy console costs one
/// write per session per interval, not one per request.
pub const RENEW_EVERY: Duration = Duration::from_secs(300);

/// A member's role in a workspace (09 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Owner,
    Admin,
    Editor,
    Operator,
    Viewer,
}

impl Role {
    pub const ALL: [Self; 5] = [
        Self::Owner,
        Self::Admin,
        Self::Editor,
        Self::Operator,
        Self::Viewer,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Editor => "editor",
            Self::Operator => "operator",
            Self::Viewer => "viewer",
        }
    }

    #[must_use]
    pub fn from_db(role: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == role)
    }
}

/// Emails are stored trimmed and lowercased, so the `UNIQUE` on
/// `users.email` is case-insensitive. `None` unless it looks like an address:
/// 3 to 254 characters, one `@` with something on both sides, no spaces or
/// controls. Nothing is sent to it.
#[must_use]
pub fn normalise_email(email: &str) -> Option<String> {
    let email = email.trim().to_lowercase();
    let (local, domain) = email.split_once('@')?;
    let valid = (3..=254).contains(&email.chars().count())
        && !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && !email.chars().any(|c| c.is_whitespace() || c.is_control());
    valid.then_some(email)
}

/// The shortest and longest passwords, in characters: NIST SP 800-63B rev. 4
/// asks for at least 15 where the password is the only factor (TOTP is not
/// built yet) and a maximum of at least 64; no composition rules. The
/// maximum also bounds the work of hashing what a stranger sends.
pub const PASSWORD_MIN: usize = 15;
pub const PASSWORD_MAX: usize = 128;

/// Whether `password` may be set: its length in characters, nothing else.
pub fn check_password(password: &str) -> Result<(), &'static str> {
    let n = password.chars().count();
    if n < PASSWORD_MIN {
        Err("must be at least 15 characters")
    } else if n > PASSWORD_MAX {
        Err("must be at most 128 characters")
    } else {
        Ok(())
    }
}

/// Random bytes in a console session identifier: 256 bits.
const SESSION_BYTES: usize = 32;

/// A console session's identifier, as the cookie carries it: `cs_` and 64
/// lowercase hex digits from the operating system's secure random source.
/// Only its SHA-256 is stored; it is never logged.
#[derive(Clone)]
pub struct SessionId(String);

impl SessionId {
    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut bytes = [0; SESSION_BYTES];
        getrandom::fill(&mut bytes)?;
        Ok(Self(format!("cs_{}", hex(&bytes))))
    }

    /// The cookie's value, if it can be an identifier.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let digits = value.strip_prefix("cs_")?;
        let valid = digits.len() == 2 * SESSION_BYTES
            && digits
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        valid.then(|| Self(value.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `console_sessions.id_hash`.
    #[must_use]
    pub fn hash(&self) -> [u8; 32] {
        Sha256::digest(self.0.as_bytes()).into()
    }

    /// The session's CSRF token: SHA-256 of a fixed label and the identifier,
    /// in hex. The page reads it from `GET /console/api/me` and sends it in
    /// `Cavea-Csrf-Token`; another site can neither read the cookie nor
    /// compute this from anything it sees, and the token does not reveal the
    /// identifier. It changes with the session, and nothing of it is stored.
    #[must_use]
    pub fn csrf(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"cavea console csrf v1\0");
        digest.update(self.0.as_bytes());
        hex(&digest.finalize())
    }
}

impl fmt::Debug for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionId(..)")
    }
}

fn hex(bytes: &[u8]) -> String {
    use fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]

    use super::*;

    #[test]
    fn emails_are_normalised_and_checked() {
        assert_eq!(
            normalise_email("  Ada@Example.ORG ").as_deref(),
            Some("ada@example.org")
        );
        for bad in [
            "",
            "ada",
            "@example.org",
            "ada@",
            "a b@example.org",
            "a@b@c",
        ] {
            assert_eq!(normalise_email(bad), None, "{bad}");
        }
        assert!(check_password(&"x".repeat(14)).is_err());
        assert!(check_password(&"é".repeat(15)).is_ok());
        assert!(check_password(&"x".repeat(129)).is_err());
    }

    #[test]
    fn session_ids_parse_and_derive_a_csrf_token() {
        let id = SessionId::generate().unwrap();
        let again = SessionId::parse(id.as_str()).unwrap();
        assert_eq!(id.hash(), again.hash());
        assert_eq!(id.csrf(), again.csrf());
        assert_eq!(id.csrf().len(), 64);
        assert_ne!(id.csrf(), SessionId::generate().unwrap().csrf());
        assert!(SessionId::parse(&id.as_str().to_uppercase()).is_none());
        assert!(SessionId::parse("cs_00").is_none());
        assert_eq!(format!("{id:?}"), "SessionId(..)");
    }
}
