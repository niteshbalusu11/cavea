//! Cavea licence files (12 §7.2, D-80): the format, and its offline
//! verification against the public keys compiled into the build
//! ([`TRUSTED`]). Signing is behind the `sign` feature, for the private tool
//! (`cargo xtask licence`) and tests; the server never enables it.
//!
//! A licence file is one JSON object, at most [`MAX_BYTES`]:
//!
//! - `cavea_licence`: the envelope's version, `1`;
//! - `key_id`: the signing key's id ([`PublicKey::id`]);
//! - `payload`: the licence document's exact bytes, in standard base64;
//! - `signature`: the Ed25519 signature, in standard base64, of
//!   `cavea-licence-v1`, a zero byte, and those bytes.
//!
//! Verification never re-serialises anything: the signature covers the
//! payload's bytes as issued, which are then read as JSON. The envelope's and
//! the payload's unknown fields are ignored, so a licence issued with fields
//! a later version adds still verifies here; an unknown plan or feature is
//! carried as text. Strict about signatures, lenient about the rest: every
//! failure is an [`Invalid`] with a reason, none panics, and the work is
//! bounded by the size cap.

mod date;
mod keys;
#[cfg(any(test, feature = "sign"))]
pub mod sign;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::Deserialize;

pub use date::{DAY_MS, Date};
pub use keys::TRUSTED;

/// The largest licence file read, in bytes. Real ones are under 1 KiB.
pub const MAX_BYTES: usize = 16 * 1024;
/// The envelope version this build reads and writes.
pub const VERSION: u64 = 1;
/// What every signature covers before the payload: no Cavea key ever signs
/// bytes that could be read as anything but a licence.
const CONTEXT: &[u8] = b"cavea-licence-v1\0";
/// JSON nesting allowed in the envelope and the payload (both are flat).
const MAX_DEPTH: usize = 16;

/// An Ed25519 public key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicKey([u8; 32]);

impl PublicKey {
    /// From its 32 bytes in standard base64, as [`TRUSTED`] lists them.
    #[must_use]
    pub fn from_base64(s: &str) -> Option<Self> {
        let bytes = STANDARD.decode(s.trim()).ok()?;
        Some(Self(bytes.try_into().ok()?))
    }

    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn to_base64(&self) -> String {
        STANDARD.encode(self.0)
    }

    /// The key's id: the first 8 bytes of its SHA-256, as 16 lowercase hex
    /// digits. Derived from the key, so it can never name another.
    #[must_use]
    pub fn id(&self) -> String {
        let digest = ring::digest::digest(&ring::digest::SHA256, &self.0);
        digest
            .as_ref()
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// Why a licence file is invalid: logged, and shown on the owner's licence
/// screen ([`Invalid::as_str`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalid {
    /// Over [`MAX_BYTES`].
    TooLarge,
    /// Not UTF-8.
    NotUtf8,
    /// Not a licence envelope: not JSON, nested too deep, a field missing,
    /// duplicated or of the wrong type, base64 that does not decode.
    Malformed,
    /// An envelope version this build does not read.
    Version,
    /// Signed by a key this build does not trust.
    UnknownKey,
    /// The signature does not match.
    Signature,
    /// Signed, but the payload is not a licence document.
    Payload,
    /// Signed, but a date does not exist or it expires before it is issued.
    Dates,
}

impl Invalid {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TooLarge => "too_large",
            Self::NotUtf8 => "not_utf8",
            Self::Malformed => "malformed",
            Self::Version => "version",
            Self::UnknownKey => "unknown_key",
            Self::Signature => "signature",
            Self::Payload => "payload",
            Self::Dates => "dates",
        }
    }

    /// Every reason, for documentation and tests.
    pub const ALL: [Self; 8] = [
        Self::TooLarge,
        Self::NotUtf8,
        Self::Malformed,
        Self::Version,
        Self::UnknownKey,
        Self::Signature,
        Self::Payload,
        Self::Dates,
    ];
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::error::Error for Invalid {}

/// Where a verified licence stands at an instant (12 §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Up to the end of `expires_at` (UTC).
    Valid,
    /// The `grace_days` whole days after `expires_at`.
    Grace,
    /// From then on.
    Expired,
}

/// A licence document, as signed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Licence {
    /// Such as `lic_2027_0142`.
    pub id: String,
    pub customer: String,
    /// `starter`, `growth`, `scale` or one a later version adds.
    pub plan: String,
    /// Feature names, unknown ones kept.
    pub features: Vec<String>,
    /// Where the renderer runs unmarked: host names, or `*.` and a host name
    /// for every name under it (not the name itself). Lowercase.
    pub domains: Vec<String>,
    /// `None`: unlimited.
    pub max_workspaces: Option<u32>,
    /// From 00:00 UTC of this day.
    pub issued_at: Date,
    /// Through 23:59:59.999 UTC of this day.
    pub expires_at: Date,
    pub grace_days: u32,
    /// The id of the key that signed it.
    pub key_id: String,
}

impl Licence {
    /// The first instant it is no longer valid: 00:00 UTC the day after
    /// `expires_at`.
    #[must_use]
    pub fn expires_ms(&self) -> u64 {
        self.expires_at.plus(1).start_ms()
    }

    /// The first instant it is expired: `grace_days` days after
    /// [`Self::expires_ms`].
    #[must_use]
    pub fn grace_ends_ms(&self) -> u64 {
        self.expires_at.plus(1).plus(self.grace_days).start_ms()
    }

    /// The last day of grace, or `expires_at` without grace.
    #[must_use]
    pub fn last_grace_day(&self) -> Date {
        self.expires_at.plus(self.grace_days)
    }

    /// Where it stands at `now_ms` (milliseconds since the epoch). A clock
    /// before `issued_at` is the server's clock being wrong, or a licence
    /// issued ahead of its term: either way it is [`Standing::Valid`], since
    /// only expiry, never the clock alone, takes a licence away.
    #[must_use]
    pub fn standing(&self, now_ms: u64) -> Standing {
        if now_ms < self.expires_ms() {
            Standing::Valid
        } else if now_ms < self.grace_ends_ms() {
            Standing::Grace
        } else {
            Standing::Expired
        }
    }

    /// Whether `now_ms` is before `issued_at`: worth a warning in the log.
    #[must_use]
    pub fn before_issue(&self, now_ms: u64) -> bool {
        now_ms < self.issued_at.start_ms()
    }
}

/// Whether `pattern` is a domain pattern a licence may carry: a lowercase
/// host name (letters, digits and hyphens in dot-separated labels of 1 to 63
/// characters, not starting or ending with a hyphen, 253 characters at
/// most), optionally prefixed with `*.`.
#[must_use]
pub fn valid_domain(pattern: &str) -> bool {
    let host = pattern.strip_prefix("*.").unwrap_or(pattern);
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

#[derive(Deserialize)]
struct Envelope {
    cavea_licence: u64,
    key_id: String,
    payload: String,
    signature: String,
}

#[derive(Deserialize)]
struct Payload {
    licence_id: String,
    customer: String,
    plan: String,
    #[serde(default)]
    features: Vec<String>,
    #[serde(default)]
    domains: Vec<String>,
    #[serde(default)]
    max_workspaces: Option<u32>,
    issued_at: String,
    expires_at: String,
    #[serde(default)]
    grace_days: u32,
}

/// The envelope, read but not verified.
struct Opened {
    key_id: String,
    payload: Vec<u8>,
    signature: Vec<u8>,
}

/// Whether JSON `bytes` nest deeper than [`MAX_DEPTH`], by a scan that skips
/// strings: checked before parsing, so that hostile nesting costs a scan.
fn too_deep(bytes: &[u8]) -> bool {
    let (mut depth, mut in_string, mut escaped) = (0usize, false, false);
    for &b in bytes {
        if in_string {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return true;
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

fn open(file: &[u8]) -> Result<Opened, Invalid> {
    if file.len() > MAX_BYTES {
        return Err(Invalid::TooLarge);
    }
    let text = std::str::from_utf8(file).map_err(|_| Invalid::NotUtf8)?;
    if too_deep(text.as_bytes()) {
        return Err(Invalid::Malformed);
    }
    let envelope: Envelope = serde_json::from_str(text).map_err(|_| Invalid::Malformed)?;
    if envelope.cavea_licence != VERSION {
        return Err(Invalid::Version);
    }
    let payload = STANDARD
        .decode(&envelope.payload)
        .map_err(|_| Invalid::Malformed)?;
    let signature = STANDARD
        .decode(&envelope.signature)
        .map_err(|_| Invalid::Malformed)?;
    Ok(Opened {
        key_id: envelope.key_id,
        payload,
        signature,
    })
}

fn read_payload(bytes: &[u8], key_id: String) -> Result<Licence, Invalid> {
    if too_deep(bytes) {
        return Err(Invalid::Payload);
    }
    let p: Payload = serde_json::from_slice(bytes).map_err(|_| Invalid::Payload)?;
    let issued_at = Date::parse(&p.issued_at).ok_or(Invalid::Dates)?;
    let expires_at = Date::parse(&p.expires_at).ok_or(Invalid::Dates)?;
    if expires_at < issued_at {
        return Err(Invalid::Dates);
    }
    Ok(Licence {
        id: p.licence_id,
        customer: p.customer,
        plan: p.plan,
        features: p.features,
        domains: p
            .domains
            .iter()
            .map(|d| d.trim().to_ascii_lowercase())
            .collect(),
        max_workspaces: p.max_workspaces,
        issued_at,
        expires_at,
        grace_days: p.grace_days,
        key_id,
    })
}

/// The message a signature covers.
fn signed_message(payload: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(CONTEXT.len() + payload.len());
    message.extend_from_slice(CONTEXT);
    message.extend_from_slice(payload);
    message
}

/// Checks licence files against a set of trusted keys.
#[derive(Debug, Clone)]
pub struct Verifier {
    keys: Vec<PublicKey>,
}

impl Verifier {
    /// The keys compiled into this build ([`TRUSTED`]): what every server
    /// uses. An entry that does not decode is skipped (a test keeps the list
    /// well formed).
    #[must_use]
    pub fn production() -> Self {
        Self::new(TRUSTED.iter().filter_map(|k| PublicKey::from_base64(k)))
    }

    /// These keys only: for tests, and for the tool's `verify --public-key`.
    pub fn new(keys: impl IntoIterator<Item = PublicKey>) -> Self {
        Self {
            keys: keys.into_iter().collect(),
        }
    }

    /// The trusted keys' ids.
    #[must_use]
    pub fn key_ids(&self) -> Vec<String> {
        self.keys.iter().map(PublicKey::id).collect()
    }

    /// The licence in `file`, if a trusted key signed it and it reads.
    pub fn verify(&self, file: &[u8]) -> Result<Licence, Invalid> {
        let opened = open(file)?;
        let key = self
            .keys
            .iter()
            .find(|k| k.id() == opened.key_id)
            .ok_or(Invalid::UnknownKey)?;
        UnparsedPublicKey::new(&ED25519, key.0)
            .verify(&signed_message(&opened.payload), &opened.signature)
            .map_err(|_| Invalid::Signature)?;
        read_payload(&opened.payload, opened.key_id)
    }
}

/// The licence in `file` without checking its signature, for the tool's
/// `inspect`: never trust what it returns.
pub fn inspect_unverified(file: &[u8]) -> Result<Licence, Invalid> {
    let opened = open(file)?;
    read_payload(&opened.payload, opened.key_id)
}

#[cfg(test)]
mod tests;
