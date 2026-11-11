//! Console passwords: argon2id (RFC 9106) with the parameters OWASP's
//! Password Storage Cheat Sheet lists first: 19 MiB of memory, two passes,
//! one lane. A hash is stored in the PHC string form,
//! `$argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>` (base64 without padding),
//! so that stronger parameters later still verify older hashes.
//!
//! Hashing is CPU and memory work (about 25 ms and 19 MiB each), so it runs on
//! tokio's blocking threads, at most [`MAX_HASHES`] at once per node: a burst
//! of sign-in attempts queues here and never occupies the request runtime's
//! workers, which serve holds (D-45). A request waits at most [`WAIT`] for its
//! turn, then is `503`.

use std::sync::OnceLock;
use std::time::Duration;

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use subtle::ConstantTimeEq as _;
use tokio::sync::Semaphore;

/// Memory in KiB, passes, lanes (OWASP: m=19456, t=2, p=1).
pub const MEMORY_KIB: u32 = 19_456;
pub const PASSES: u32 = 2;
pub const LANES: u32 = 1;
const SALT_BYTES: usize = 16;
const HASH_BYTES: usize = 32;

/// Hashes computed at once per node: 2 × 19 MiB of memory at most.
pub const MAX_HASHES: usize = 2;
/// The longest a request waits for its turn to hash.
pub const WAIT: Duration = Duration::from_secs(5);

static TURNS: Semaphore = Semaphore::const_new(MAX_HASHES);

/// No turn to hash within [`WAIT`], or the work could not run.
#[derive(Debug)]
pub enum Busy {
    Timeout,
    Failed(String),
}

/// The PHC string of `password` with a new random salt. Blocking: call
/// [`hash_bounded`] from a request.
pub fn hash(password: &str) -> Result<String, Busy> {
    let mut salt = [0; SALT_BYTES];
    getrandom::fill(&mut salt).map_err(|e| Busy::Failed(e.to_string()))?;
    let params = Params::new(MEMORY_KIB, PASSES, LANES, Some(HASH_BYTES))
        .map_err(|e| Busy::Failed(e.to_string()))?;
    let mut out = [0; HASH_BYTES];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), &salt, &mut out)
        .map_err(|e| Busy::Failed(e.to_string()))?;
    Ok(format!(
        "$argon2id$v=19$m={MEMORY_KIB},t={PASSES},p={LANES}${}${}",
        STANDARD_NO_PAD.encode(salt),
        STANDARD_NO_PAD.encode(out)
    ))
}

/// Whether `password` matches the PHC string `stored`, compared in constant
/// time. A string that does not parse, or names parameters outside sane
/// bounds, matches nothing. Blocking: call [`verify_bounded`].
#[must_use]
pub fn verify(password: &str, stored: &str) -> bool {
    let Some((params, salt, expected)) = parse(stored) else {
        return false;
    };
    let mut out = vec![0; expected.len()];
    let hashed = Argon2::new(Algorithm::Argon2id, Version::V0x13, params).hash_password_into(
        password.as_bytes(),
        &salt,
        &mut out,
    );
    hashed.is_ok() && bool::from(out.ct_eq(&expected))
}

/// `$argon2id$v=19$m=…,t=…,p=…$salt$hash`, with m up to 1 GiB, t up to 16,
/// p up to 8, a salt of 8 to 64 bytes and a hash of 16 to 64.
fn parse(stored: &str) -> Option<(Params, Vec<u8>, Vec<u8>)> {
    let mut parts = stored.split('$');
    let (Some(""), Some("argon2id"), Some("v=19"), Some(params), Some(salt), Some(hash), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return None;
    };
    let mut m_t_p = params.split(',').map(|p| p.split_once('='));
    let mut value = |name: &str, max: u32| -> Option<u32> {
        let (key, value) = m_t_p.next()??;
        let value: u32 = value.parse().ok()?;
        (key == name && (1..=max).contains(&value)).then_some(value)
    };
    let (m, t, p) = (value("m", 1 << 20)?, value("t", 16)?, value("p", 8)?);
    let salt = STANDARD_NO_PAD.decode(salt).ok()?;
    let hash = STANDARD_NO_PAD.decode(hash).ok()?;
    let lengths = (8..=64).contains(&salt.len()) && (16..=64).contains(&hash.len());
    let params = Params::new(m, t, p, Some(hash.len())).ok()?;
    lengths.then_some((params, salt, hash))
}

/// A hash of a password nobody knows, verified when the account does not
/// exist, so that "no such user" costs exactly what "wrong password" does.
fn dummy() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| hash("no account has this password").unwrap_or_default())
}

/// Compute the dummy hash now, at start-up, so that the first sign-in with
/// an unknown email costs what every later one does. Blocking.
pub fn warm() {
    let _ = dummy();
}

/// Run `work` on a blocking thread once a turn is free.
async fn bounded<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, Busy> {
    let turn = tokio::time::timeout(WAIT, TURNS.acquire())
        .await
        .map_err(|_| Busy::Timeout)?
        .map_err(|e| Busy::Failed(e.to_string()))?;
    let result = tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| Busy::Failed(e.to_string()));
    drop(turn);
    result
}

/// [`hash`], bounded.
pub async fn hash_bounded(password: String) -> Result<String, Busy> {
    bounded(move || hash(&password)).await?
}

/// [`verify`] against `stored`, or against a dummy hash when there is none
/// (no such user, or no password): the same work either way, and `false`.
pub async fn verify_bounded(password: String, stored: Option<String>) -> Result<bool, Busy> {
    bounded(move || match stored {
        Some(stored) => verify(&password, &stored),
        None => {
            let _ = verify(&password, dummy());
            false
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]

    use super::*;

    #[test]
    fn a_hash_verifies_its_password_only() {
        let stored = hash("correct horse battery staple").unwrap();
        assert!(stored.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert!(verify("correct horse battery staple", &stored));
        assert!(!verify("correct horse battery stapler", &stored));
        assert_ne!(stored, hash("correct horse battery staple").unwrap());
        for broken in [
            "",
            "$argon2i$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$aGFzaGhhc2hoYXNoaGFzaA",
            "$argon2id$v=19$m=4294967295,t=2,p=1$c2FsdHNhbHQ$aGFzaGhhc2hoYXNoaGFzaA",
            "$argon2id$v=19$t=2,m=19456,p=1$c2FsdHNhbHQ$aGFzaGhhc2hoYXNoaGFzaA",
            &format!("{stored}$"),
        ] {
            assert!(!verify("correct horse battery staple", broken), "{broken}");
        }
        assert!(verify("no account has this password", dummy()));
    }

    /// One verification's time, and the process's peak memory growth over
    /// the first hash. Run alone, in release: `cargo test --release -p cavea-server --lib
    /// measure_password -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement: run in release"]
    fn measure_password_verification() {
        let peak = || -> u64 {
            let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
            status
                .lines()
                .find_map(|l| l.strip_prefix("VmHWM:"))
                .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
                .unwrap_or(0)
        };
        let before = peak();
        let stored = hash("correct horse battery staple").unwrap();
        let mut times = Vec::new();
        for _ in 0..50 {
            let started = std::time::Instant::now();
            assert!(verify("correct horse battery staple", &stored));
            times.push(started.elapsed().as_secs_f64() * 1e3);
        }
        times.sort_by(f64::total_cmp);
        println!(
            "argon2id m={MEMORY_KIB} t={PASSES} p={LANES}: median {:.1} ms, max {:.1} ms; \
             peak RSS +{} KiB",
            times[25],
            times[49],
            peak().saturating_sub(before)
        );
    }
}
