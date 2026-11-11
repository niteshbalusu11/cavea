//! Sign-in back-off (12 §3 T9): slows password guessing without letting a
//! stranger lock a member out.
//!
//! Three layers, all in memory and per node, all keyed alike whether or not
//! the account exists, so that none of them tells an attacker which emails
//! have accounts:
//!
//! - **Per client address**, every attempt (sign-in, setup, password change)
//!   draws on the rate limiter's `sign_in` budget (`CAVEA_RATE_LIMIT_SIGN_IN`,
//!   08 §12): spreading guesses over many accounts from one address is slow.
//! - **Per (account, address)**: after [`PAIR_FREE`] failures in a row, each
//!   further attempt from that address waits 1 s, then 2, 4, … up to
//!   [`MAX_DELAY`]. A success clears it. This is what stops one source
//!   guessing one account, and it never touches the member's own address.
//! - **Per account**, from every address: after [`ACCOUNT_FREE`] failures,
//!   the same doubling delay, against a distributed attack. Addresses where
//!   the account signed in successfully within [`KNOWN_FOR`] are exempt, so a
//!   member keeps signing in from where they usually do, however long an
//!   attack lasts; only new places wait.
//!
//! Failures are forgotten after [`FORGET`] without one, so every delay is
//! bounded and ends by itself. A refused attempt verifies nothing and is not
//! counted as a failure.

use std::collections::HashMap;
use std::hash::{BuildHasher as _, RandomState};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

/// Failures in a row from one address before its attempts on the account wait.
pub const PAIR_FREE: u32 = 5;
/// Failures on an account, from anywhere, before new addresses wait.
pub const ACCOUNT_FREE: u32 = 20;
/// The longest wait, in milliseconds: 15 minutes.
pub const MAX_DELAY: u64 = 15 * 60 * 1000;
/// Failures are forgotten this long after the last one: 1 hour.
pub const FORGET: u64 = 60 * 60 * 1000;
/// A successful sign-in exempts its address from the account's wait this
/// long: 30 days.
pub const KNOWN_FOR: u64 = 30 * 24 * 60 * 60 * 1000;
/// Entries in each map at most. When full, entries that no longer delay
/// anything are dropped; if it is still full, a new failure is not recorded
/// (the per-address budget still holds).
pub const MAX_ENTRIES: usize = 100_000;

#[derive(Debug, Clone, Copy)]
struct Strikes {
    failures: u32,
    /// Milliseconds since the epoch of the last failure.
    last: u64,
}

impl Strikes {
    /// When the next attempt may be made, given `free` failures without wait.
    fn open_at(self, free: u32, now: u64) -> u64 {
        if now.saturating_sub(self.last) >= FORGET || self.failures < free {
            return 0;
        }
        let doublings = (self.failures - free).min(20);
        self.last + (1000u64 << doublings).min(MAX_DELAY)
    }
}

#[derive(Default)]
struct Maps {
    pairs: HashMap<u64, Strikes>,
    accounts: HashMap<u64, Strikes>,
    /// (account, address) pairs that signed in, until when.
    known: HashMap<u64, u64>,
}

/// Wait this many seconds (at least 1) before trying again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wait(pub u64);

/// The node's sign-in back-off. One per server, in `AppState`.
pub struct Guard {
    maps: Mutex<Maps>,
    hasher: RandomState,
    /// Sign-in results, for `cavea_console_sign_ins_total`.
    counts: [AtomicU64; 3],
}

/// How a sign-in attempt ended, for the metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    /// Wrong email or password.
    Credentials,
    /// Refused by the back-off without checking the password.
    Backoff,
}

impl Outcome {
    pub const ALL: [Self; 3] = [Self::Ok, Self::Credentials, Self::Backoff];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Credentials => "credentials",
            Self::Backoff => "backoff",
        }
    }
}

impl std::fmt::Debug for Guard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Guard(..)")
    }
}

impl Default for Guard {
    fn default() -> Self {
        Self {
            maps: Mutex::default(),
            hasher: RandomState::new(),
            counts: [const { AtomicU64::new(0) }; 3],
        }
    }
}

impl Guard {
    fn keys(&self, account: &str, addr: Option<IpAddr>) -> (u64, u64) {
        (
            self.hasher.hash_one(("account", account)),
            self.hasher.hash_one(("pair", account, addr)),
        )
    }

    /// Whether `account` (a normalised email) may be tried from `addr` (the
    /// grouped client address) at `now` (milliseconds).
    pub fn check(&self, account: &str, addr: Option<IpAddr>, now: u64) -> Result<(), Wait> {
        let (account, pair) = self.keys(account, addr);
        let maps = self.maps.lock().unwrap_or_else(PoisonError::into_inner);
        let mut open_at = maps
            .pairs
            .get(&pair)
            .map_or(0, |s| s.open_at(PAIR_FREE, now));
        let known = maps.known.get(&pair).is_some_and(|&until| until > now);
        if !known {
            let account = maps
                .accounts
                .get(&account)
                .map_or(0, |s| s.open_at(ACCOUNT_FREE, now));
            open_at = open_at.max(account);
        }
        drop(maps);
        if open_at > now {
            self.count(Outcome::Backoff);
            return Err(Wait((open_at - now).div_ceil(1000).max(1)));
        }
        Ok(())
    }

    /// Record a wrong email or password.
    pub fn failed(&self, account: &str, addr: Option<IpAddr>, now: u64) {
        self.count(Outcome::Credentials);
        let (account, pair) = self.keys(account, addr);
        let mut maps = self.maps.lock().unwrap_or_else(PoisonError::into_inner);
        strike(&mut maps.pairs, pair, now);
        strike(&mut maps.accounts, account, now);
    }

    /// Record a sign-in: the address is known to the account from now on.
    /// The account's own failures are kept: a member signing in during an
    /// attack must not reopen the account to it.
    pub fn succeeded(&self, account: &str, addr: Option<IpAddr>, now: u64) {
        self.count(Outcome::Ok);
        let (_, pair) = self.keys(account, addr);
        let mut maps = self.maps.lock().unwrap_or_else(PoisonError::into_inner);
        maps.pairs.remove(&pair);
        if maps.known.len() >= MAX_ENTRIES {
            maps.known.retain(|_, until| *until > now);
        }
        if maps.known.len() < MAX_ENTRIES {
            maps.known.insert(pair, now + KNOWN_FOR);
        }
    }

    fn count(&self, outcome: Outcome) {
        if let Some(n) = self.counts.get(outcome as usize) {
            n.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Sign-ins that ended so since start-up.
    #[must_use]
    pub fn counted(&self, outcome: Outcome) -> u64 {
        self.counts
            .get(outcome as usize)
            .map_or(0, |n| n.load(Ordering::Relaxed))
    }
}

fn strike(map: &mut HashMap<u64, Strikes>, key: u64, now: u64) {
    if !map.contains_key(&key) && map.len() >= MAX_ENTRIES {
        map.retain(|_, s| now.saturating_sub(s.last) < FORGET);
        if map.len() >= MAX_ENTRIES {
            return;
        }
    }
    let s = map.entry(key).or_insert(Strikes {
        failures: 0,
        last: now,
    });
    if now.saturating_sub(s.last) >= FORGET {
        s.failures = 0;
    }
    s.failures = s.failures.saturating_add(1);
    s.last = now;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]

    use super::*;

    fn ip(s: &str) -> Option<IpAddr> {
        Some(s.parse().unwrap())
    }

    #[test]
    fn one_source_backs_off_exponentially_and_is_forgotten() {
        let guard = Guard::default();
        let (me, attacker) = (ip("198.51.100.7"), ip("203.0.113.9"));
        let t = 1_000_000;
        for _ in 0..PAIR_FREE {
            guard.check("ada@example.org", attacker, t).unwrap();
            guard.failed("ada@example.org", attacker, t);
        }
        // 1 s after the fifth failure, then 2 s after the sixth, …
        assert_eq!(guard.check("ada@example.org", attacker, t), Err(Wait(1)));
        guard.check("ada@example.org", attacker, t + 1_000).unwrap();
        guard.failed("ada@example.org", attacker, t + 1_000);
        assert_eq!(
            guard.check("ada@example.org", attacker, t + 1_000),
            Err(Wait(2))
        );
        // The member's own address, and other accounts, are untouched.
        guard.check("ada@example.org", me, t).unwrap();
        guard.check("bob@example.org", attacker, t).unwrap();
        // Capped, and forgotten after an hour without failures.
        let mut now = t + 1_000;
        for _ in 0..30 {
            now += MAX_DELAY;
            guard.failed("ada@example.org", attacker, now);
        }
        assert_eq!(
            guard.check("ada@example.org", attacker, now),
            Err(Wait(MAX_DELAY / 1000))
        );
        guard
            .check("ada@example.org", attacker, now + FORGET)
            .unwrap();
        assert_eq!(guard.counted(Outcome::Backoff), 3);
    }

    #[test]
    fn a_distributed_attack_never_locks_out_a_known_address() {
        let guard = Guard::default();
        let t = 1_000_000;
        let home = ip("198.51.100.7");
        guard.succeeded("ada@example.org", home, t);
        for i in 0..ACCOUNT_FREE {
            let bot = Some(IpAddr::from([203, 0, 113, u8::try_from(i).unwrap()]));
            guard.failed("ada@example.org", bot, t);
        }
        // New addresses wait; the known one never does.
        assert_eq!(
            guard.check("ada@example.org", ip("192.0.2.1"), t),
            Err(Wait(1))
        );
        guard.check("ada@example.org", home, t).unwrap();
        // The same for an email nobody has: nothing to tell them apart.
        for i in 0..ACCOUNT_FREE {
            let bot = Some(IpAddr::from([203, 0, 113, u8::try_from(i).unwrap()]));
            guard.failed("nobody@example.org", bot, t);
        }
        assert_eq!(
            guard.check("nobody@example.org", ip("192.0.2.1"), t),
            Err(Wait(1))
        );
    }
}
