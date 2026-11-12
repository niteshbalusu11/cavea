//! The second factor (09 §2, 12 §3 T9, D-89): time-based one-time passwords
//! (RFC 6238 over RFC 4226's HOTP) with HMAC-SHA-1, 30-second steps and six
//! digits, the parameters every authenticator app reads from an
//! `otpauth://` URI; and one-time recovery codes.
//!
//! The secret is 160 random bits (RFC 4226 §4's recommendation). The server
//! must compute codes from it, so it is stored as it is, not hashed: what
//! protects it at rest is the database's own access control (12 §3 T9).
//! Recovery codes are random enough (50 bits each) to be stored as SHA-256
//! and looked up by that hash, like API keys.
//!
//! Time is the server's injected clock (`ServerClock`), so tests move it.

use ring::hmac;
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;

/// Bytes of a secret: 160 bits.
pub const SECRET_BYTES: usize = 20;
/// A step, in milliseconds.
pub const STEP_MS: u64 = 30_000;
/// Digits of a code.
pub const DIGITS: usize = 6;
/// Steps either side of now that are accepted: a clock one step off, or a
/// code typed as its step ends.
pub const SKEW: u64 = 1;
/// Recovery codes made at enrolment.
pub const RECOVERY_CODES: usize = 10;

/// RFC 4648 base32, upper case: what authenticator apps take.
const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// A new secret from the operating system's random source.
pub fn secret() -> Result<[u8; SECRET_BYTES], getrandom::Error> {
    let mut bytes = [0; SECRET_BYTES];
    getrandom::fill(&mut bytes)?;
    Ok(bytes)
}

/// Base32 without padding.
#[must_use]
pub fn base32(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(ALPHABET[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(ALPHABET[((buffer << (5 - bits)) & 31) as usize]));
    }
    out
}

/// Percent-encode everything but RFC 3986's unreserved characters.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The URI an authenticator app reads (as a QR code or a link): the issuer
/// and the account name label the entry in the app.
#[must_use]
pub fn uri(issuer: &str, account: &str, secret: &[u8]) -> String {
    let issuer = encode(issuer);
    format!(
        "otpauth://totp/{issuer}:{}?secret={}&issuer={issuer}&algorithm=SHA1&digits={DIGITS}&period={}",
        encode(account),
        base32(secret),
        STEP_MS / 1000
    )
}

/// RFC 4226's HOTP value of `counter`: HMAC-SHA-1, dynamic truncation,
/// `DIGITS` digits.
#[must_use]
pub fn hotp(secret: &[u8], counter: u64) -> u32 {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let tag = hmac::sign(&key, &counter.to_be_bytes());
    let mac = tag.as_ref();
    let offset = usize::from(mac.last().copied().unwrap_or(0) & 0x0f);
    let word = mac
        .get(offset..offset + 4)
        .and_then(|w| <[u8; 4]>::try_from(w).ok())
        .map_or(0, u32::from_be_bytes);
    (word & 0x7fff_ffff) % 10u32.pow(DIGITS as u32)
}

/// The step of `now_ms`.
#[must_use]
pub fn step(now_ms: u64) -> u64 {
    now_ms / STEP_MS
}

/// The code of `step`, zero-padded.
#[must_use]
pub fn code(secret: &[u8], step: u64) -> String {
    format!("{:0width$}", hotp(secret, step), width = DIGITS)
}

/// The step whose code `code` is, among the steps within [`SKEW`] of
/// `now_ms` that are later than `after` (the last step used: a code is
/// accepted once). Every candidate is compared in constant time.
#[must_use]
pub fn verify(secret: &[u8], code: &str, now_ms: u64, after: Option<u64>) -> Option<u64> {
    if code.len() != DIGITS || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let now = step(now_ms);
    let mut found = None;
    for candidate in now.saturating_sub(SKEW)..=now.saturating_add(SKEW) {
        let matches = bool::from(
            self::code(secret, candidate)
                .as_bytes()
                .ct_eq(code.as_bytes()),
        );
        if matches && after.is_none_or(|last| candidate > last) && found.is_none() {
            found = Some(candidate);
        }
    }
    found
}

/// A code as typed, without spaces or dashes, lower case: six digits are a
/// TOTP code, anything else a recovery code.
#[must_use]
pub fn normalise(code: &str) -> String {
    code.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether a normalised code is a TOTP code rather than a recovery code.
#[must_use]
pub fn is_totp(code: &str) -> bool {
    code.len() == DIGITS && code.bytes().all(|b| b.is_ascii_digit())
}

/// New recovery codes, as shown once: ten characters of lower-case base32
/// in two groups, `abcde-fghij` (50 bits each).
pub fn recovery_codes() -> Result<Vec<String>, getrandom::Error> {
    let mut codes = Vec::with_capacity(RECOVERY_CODES);
    for _ in 0..RECOVERY_CODES {
        let mut bytes = [0u8; 10];
        getrandom::fill(&mut bytes)?;
        let text: String = bytes
            .iter()
            .map(|b| char::from(ALPHABET[usize::from(b & 31)]).to_ascii_lowercase())
            .collect();
        let (first, second) = text.split_at(5);
        codes.push(format!("{first}-{second}"));
    }
    Ok(codes)
}

/// What is stored of a recovery code: the SHA-256 of its normalised form.
#[must_use]
pub fn recovery_hash(code: &str) -> [u8; 32] {
    Sha256::digest(normalise(code).as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 Appendix B, SHA-1: the secret is the ASCII of
    /// "12345678901234567890"; eight digits there, the last six here.
    #[test]
    fn rfc_6238_test_vectors() {
        let secret = b"12345678901234567890";
        for (seconds, eight) in [
            (59u64, "94287082"),
            (1_111_111_109, "07081804"),
            (1_111_111_111, "14050471"),
            (1_234_567_890, "89005924"),
            (2_000_000_000, "69279037"),
            (20_000_000_000, "65353130"),
        ] {
            let at = seconds * 1000;
            assert_eq!(code(secret, step(at)), eight[2..], "{seconds}");
            assert_eq!(verify(secret, &eight[2..], at, None), Some(step(at)));
        }
    }

    /// RFC 4226 Appendix D: HOTP of counters 0 to 9.
    #[test]
    fn rfc_4226_test_vectors() {
        let secret = b"12345678901234567890";
        let expected = [
            755_224, 287_082, 359_152, 969_429, 338_314, 254_676, 287_922, 162_583, 399_871,
            520_489,
        ];
        for (counter, value) in expected.into_iter().enumerate() {
            assert_eq!(hotp(secret, counter as u64), value);
        }
    }

    #[test]
    fn one_step_of_drift_and_no_replay() {
        let secret = b"12345678901234567890";
        let now = 1_000_000 * STEP_MS + 12_000;
        let at = |s: u64| code(secret, s);
        let current = step(now);
        assert_eq!(
            verify(secret, &at(current - 1), now, None),
            Some(current - 1)
        );
        assert_eq!(
            verify(secret, &at(current + 1), now, None),
            Some(current + 1)
        );
        assert_eq!(verify(secret, &at(current - 2), now, None), None);
        assert_eq!(verify(secret, &at(current + 2), now, None), None);
        // Once a step is used, it and every earlier one are refused.
        assert_eq!(verify(secret, &at(current), now, Some(current)), None);
        assert_eq!(verify(secret, &at(current - 1), now, Some(current)), None);
        assert_eq!(
            verify(secret, &at(current + 1), now, Some(current)),
            Some(current + 1)
        );
        for bad in ["", "12345", "1234567", "12a456", " 12345"] {
            assert_eq!(verify(secret, bad, now, None), None);
        }
    }

    #[test]
    fn base32_and_the_uri() {
        // RFC 4648 §10.
        for (input, output) in [
            ("", ""),
            ("f", "MY"),
            ("fo", "MZXQ"),
            ("foo", "MZXW6"),
            ("foob", "MZXW6YQ"),
            ("fooba", "MZXW6YTB"),
            ("foobar", "MZXW6YTBOI"),
        ] {
            assert_eq!(base32(input.as_bytes()), output);
        }
        let uri = uri("Cavea", "ada@example.org", b"foobar");
        assert_eq!(
            uri,
            "otpauth://totp/Cavea:ada%40example.org?secret=MZXW6YTBOI&issuer=Cavea\
&algorithm=SHA1&digits=6&period=30"
        );
    }

    #[test]
    fn recovery_codes_are_distinct_and_normalised() {
        let codes = recovery_codes().unwrap_or_default();
        assert_eq!(codes.len(), RECOVERY_CODES);
        let hashes: std::collections::BTreeSet<_> =
            codes.iter().map(|c| recovery_hash(c)).collect();
        assert_eq!(hashes.len(), RECOVERY_CODES);
        let code = &codes[0];
        assert_eq!(code.len(), 11);
        assert_eq!(
            recovery_hash(code),
            recovery_hash(&code.to_uppercase().replace('-', " "))
        );
        assert!(!is_totp(&normalise(code)));
        assert!(is_totp(&normalise("123 456")));
    }
}
