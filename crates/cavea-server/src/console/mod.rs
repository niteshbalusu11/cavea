//! The console's people (09 §2, 08 §5, 12 §3 T9): roles and what each may
//! do, emails, passwords, console sessions and the first-run setup token.
//! The HTTP side is `http::console` (the `/console/api` routes) and
//! `http::staff` (the console session as a credential on `/v1`).
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

    /// The role table of 09 §2, cell by cell: owner and admin "Full" (the
    /// admin "all except ownership transfer"); editor charts "Create, edit,
    /// publish" and events "View"; operator charts "View" and "Block,
    /// release, book, assign channels" (and so the events it acts on);
    /// viewer "View" and events "View, reports".
    #[must_use]
    pub fn allows(self, permission: Permission) -> bool {
        use Permission as P;
        match self {
            Self::Owner => true,
            Self::Admin => permission != P::Ownership,
            Self::Editor => matches!(permission, P::ChartsRead | P::ChartsWrite | P::EventsRead),
            Self::Operator => matches!(
                permission,
                P::ChartsRead | P::EventsRead | P::SeatsWrite | P::ChannelsAssign
            ),
            Self::Viewer => matches!(permission, P::ChartsRead | P::EventsRead | P::ReportsRead),
        }
    }

    /// Every permission the role grants, in [`Permission::ALL`] order.
    #[must_use]
    pub fn permissions(self) -> Vec<&'static str> {
        Permission::ALL
            .into_iter()
            .filter(|&p| self.allows(p))
            .map(Permission::as_str)
            .collect()
    }
}

/// What a route needs of a console member's role (09 §2). Every route a
/// secret key or a member may call declares one in the route table
/// (`http::openapi::Needs`), which the OpenAPI document shows as
/// `x-cavea-permission`; `GET /console/api/me` lists each role's, for the
/// interface to hide what it cannot do. Some have no route yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// Charts, drafts, versions. Every role.
    ChartsRead,
    /// Create charts, save drafts, validate, publish; upload images. Owner,
    /// admin, editor.
    ChartsWrite,
    /// Archive a chart (not in the editor's "create, edit, publish"). Owner,
    /// admin.
    ChartsArchive,
    /// Events, their objects, hold tokens. Every role.
    EventsRead,
    /// Create, change and archive events; their channel lists. Owner, admin.
    EventsWrite,
    /// Hold tokens (create, extend), hold, best-available: not in the
    /// operator's "block, release, book". Owner, admin.
    SeatsHold,
    /// Block and other statuses, release (a token's too), book. Owner, admin,
    /// operator.
    SeatsWrite,
    /// Put objects in a channel (no route yet). Owner, admin, operator.
    ChannelsAssign,
    /// Reports and exports (no route yet). Owner, admin, viewer.
    ReportsRead,
    /// API keys (later webhooks, embed tokens). Owner, admin.
    Developers,
    /// Workspace settings and members. Owner, admin.
    Settings,
    /// Ownership transfer: making or unmaking an owner, which the members
    /// routes check besides `Settings`. Owner.
    Ownership,
}

impl Permission {
    pub const ALL: [Self; 12] = [
        Self::ChartsRead,
        Self::ChartsWrite,
        Self::ChartsArchive,
        Self::EventsRead,
        Self::EventsWrite,
        Self::SeatsHold,
        Self::SeatsWrite,
        Self::ChannelsAssign,
        Self::ReportsRead,
        Self::Developers,
        Self::Settings,
        Self::Ownership,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChartsRead => "charts.read",
            Self::ChartsWrite => "charts.write",
            Self::ChartsArchive => "charts.archive",
            Self::EventsRead => "events.read",
            Self::EventsWrite => "events.write",
            Self::SeatsHold => "seats.hold",
            Self::SeatsWrite => "seats.write",
            Self::ChannelsAssign => "channels.assign",
            Self::ReportsRead => "reports.read",
            Self::Developers => "developers",
            Self::Settings => "settings",
            Self::Ownership => "ownership",
        }
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
    fn the_role_table_is_09s() {
        use Permission as P;
        let table = [
            // charts r/w/archive, events r/w, seats hold/write, channels,
            // reports, developers, settings, ownership
            (Role::Owner, "111 11 11 1 1 1 1 1"),
            (Role::Admin, "111 11 11 1 1 1 1 0"),
            (Role::Editor, "110 10 00 0 0 0 0 0"),
            (Role::Operator, "100 10 01 1 0 0 0 0"),
            (Role::Viewer, "100 10 00 0 1 0 0 0"),
        ];
        for (role, row) in table {
            let got: String = P::ALL
                .into_iter()
                .map(|p| if role.allows(p) { '1' } else { '0' })
                .collect();
            assert_eq!(got, row.replace(' ', ""), "{role:?}");
            assert_eq!(Role::from_db(role.as_str()), Some(role));
        }
    }

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
