//! Rate limits on the public endpoints (08 §12): a token bucket per
//! (publishable key, client address, budget), in memory, per node; and on
//! console sign-in attempts per client address ([`Budget::SignIn`], 12 §3 T9).
//!
//! Each bucket is one number, its theoretical arrival time (GCRA): a bucket
//! with a rate of `n` a minute admits `n` at once from rest and then one every
//! `60 s / n`. A bucket whose time has passed is full and carries nothing, so
//! it can be dropped. The buckets live in [`SHARDS`] maps under their own
//! locks, never held across an await; the check hashes the key twice and
//! allocates only when the map grows.
//!
//! The memory is bounded: a shard holds at most [`SHARD_ENTRIES`] buckets
//! (about 11 MiB for the node). A full shard first drops its full buckets, at
//! most once a second; if it is still full, the request is admitted without a
//! bucket. Under a flood of addresses the limiter fails open rather than
//! refusing buyers it has no room to count.

use std::collections::HashMap;
use std::hash::{BuildHasher as _, RandomState};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

/// Buckets are spread over this many independently locked maps.
pub const SHARDS: usize = 16;
/// The most buckets in one shard: 7/8 of 16,384 slots, the most a hash map
/// of that many slots holds before it grows.
pub const SHARD_ENTRIES: usize = 14_336;
/// How often a full shard may be swept for full buckets, in microseconds.
const SWEEP_US: u64 = 1_000_000;
const MINUTE_US: u64 = 60_000_000;

/// Which budget a request draws on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Budget {
    /// Calls that take or give back seats: hold-token creation, hold, release.
    Holds,
    /// Everything else: event info, the snapshot and polling, token state,
    /// and each WebSocket or SSE connection attempt.
    Reads,
    /// Console sign-in, setup and password-change attempts, per client
    /// address alone (the key is 0).
    SignIn,
}

/// What a bucket is kept under: 32 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    /// The publishable key's id.
    api_key: [u64; 2],
    /// An IPv4 address, or an IPv6 address's /64 ([`crate::net::group`]).
    addr: u64,
    /// The budget, and whether `addr` is IPv4.
    kind: u8,
}

struct Shard {
    /// Each bucket's theoretical arrival time, in microseconds.
    buckets: HashMap<Key, u64>,
    swept_at: u64,
}

/// The node's buckets. One per server, in `AppState`.
pub struct Limiter {
    /// Microseconds between two admissions, per budget; `None` is unlimited.
    holds: Option<u64>,
    reads: Option<u64>,
    sign_in: Option<u64>,
    hasher: RandomState,
    shards: Box<[Mutex<Shard>]>,
    /// Refusals per [`Budget`], for `cavea_rate_limited_total`.
    refused: [AtomicU64; 3],
}

impl std::fmt::Debug for Limiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Limiter")
            .field("holds", &self.holds)
            .field("reads", &self.reads)
            .field("sign_in", &self.sign_in)
            .finish_non_exhaustive()
    }
}

fn interval(per_minute: u32) -> Option<u64> {
    (per_minute > 0).then(|| MINUTE_US / u64::from(per_minute))
}

/// Refused: retry after this many seconds, at least 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limited(pub u64);

impl Limiter {
    /// Holds and reads per minute; 0 turns a budget off. Sign-in attempts
    /// are off until [`Limiter::with_sign_in`].
    #[must_use]
    pub fn new(holds_per_minute: u32, reads_per_minute: u32) -> Self {
        let hasher = RandomState::new();
        let shards = (0..SHARDS)
            .map(|_| {
                Mutex::new(Shard {
                    buckets: HashMap::with_hasher(hasher.clone()),
                    swept_at: 0,
                })
            })
            .collect();
        Self {
            holds: interval(holds_per_minute),
            reads: interval(reads_per_minute),
            sign_in: None,
            hasher,
            shards,
            refused: [const { AtomicU64::new(0) }; 3],
        }
    }

    /// Sign-in attempts per minute per client address; 0 turns it off.
    #[must_use]
    pub fn with_sign_in(mut self, per_minute: u32) -> Self {
        self.sign_in = interval(per_minute);
        self
    }

    /// Requests refused for `budget` since start-up.
    #[must_use]
    pub fn refused(&self, budget: Budget) -> u64 {
        self.refused[budget as usize].load(Ordering::Relaxed)
    }

    /// Take one from the bucket of `api_key` (a key id's 128 bits) and `addr`
    /// (grouped; `None` when unknown) for `budget`, at `now_ms`.
    pub fn check(
        &self,
        budget: Budget,
        api_key: u128,
        addr: Option<IpAddr>,
        now_ms: u64,
    ) -> Result<(), Limited> {
        let Some(interval) = (match budget {
            Budget::Holds => self.holds,
            Budget::Reads => self.reads,
            Budget::SignIn => self.sign_in,
        }) else {
            return Ok(());
        };
        let (addr, v4) = match addr {
            Some(IpAddr::V4(v4)) => (u64::from(v4.to_bits()), true),
            Some(IpAddr::V6(v6)) => ((v6.to_bits() >> 64) as u64, false),
            None => (0, false),
        };
        let key = Key {
            api_key: [(api_key >> 64) as u64, api_key as u64],
            addr,
            kind: (budget as u8) << 1 | u8::from(v4),
        };
        // A full bucket admits a minute's worth at once.
        let burst = MINUTE_US - interval;
        let now = now_ms.saturating_mul(1000);
        let index = (self.hasher.hash_one(key) % SHARDS as u64) as usize;
        let Some(shard) = self.shards.get(index) else {
            return Ok(());
        };
        let mut shard = shard.lock().unwrap_or_else(PoisonError::into_inner);
        if !shard.buckets.contains_key(&key) && shard.buckets.len() >= SHARD_ENTRIES {
            if now.saturating_sub(shard.swept_at) < SWEEP_US {
                return Ok(());
            }
            shard.swept_at = now;
            shard.buckets.retain(|_, &mut tat| tat > now);
            if shard.buckets.len() >= SHARD_ENTRIES {
                return Ok(());
            }
        }
        let tat = shard.buckets.entry(key).or_insert(now);
        let start = (*tat).max(now);
        let ahead = start - now;
        if ahead > burst {
            self.refused[budget as usize].fetch_add(1, Ordering::Relaxed);
            return Err(Limited((ahead - burst).div_ceil(1_000_000).max(1)));
        }
        *tat = start + interval;
        Ok(())
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.shards
            .iter()
            .map(|s| {
                s.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .buckets
                    .len()
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]

    use super::*;

    const KEY: u128 = 0x0123_4567_89ab_cdef_0123_4567_89ab_cdef;

    fn ip(s: &str) -> Option<IpAddr> {
        Some(s.parse().unwrap())
    }

    #[test]
    fn a_bucket_admits_a_burst_then_refills() {
        let limiter = Limiter::new(120, 3_000);
        let a = ip("198.51.100.1");
        let t = 1_000_000;
        for _ in 0..120 {
            assert_eq!(limiter.check(Budget::Holds, KEY, a, t), Ok(()));
        }
        assert_eq!(limiter.check(Budget::Holds, KEY, a, t), Err(Limited(1)));
        // One more every 500 ms.
        assert_eq!(
            limiter.check(Budget::Holds, KEY, a, t + 499),
            Err(Limited(1))
        );
        assert_eq!(limiter.check(Budget::Holds, KEY, a, t + 500), Ok(()));
        assert!(limiter.check(Budget::Holds, KEY, a, t + 500).is_err());
        // Full again a minute after the burst.
        for _ in 0..120 {
            assert_eq!(limiter.check(Budget::Holds, KEY, a, t + 60_500), Ok(()));
        }
        assert!(limiter.check(Budget::Holds, KEY, a, t + 60_500).is_err());
        // A refused request takes nothing: a client that keeps retrying is
        // admitted as soon as it would have been anyway.
        for ms in 0..500 {
            let _ = limiter.check(Budget::Holds, KEY, a, t + 60_500 + ms);
        }
        assert_eq!(limiter.check(Budget::Holds, KEY, a, t + 61_000), Ok(()));
    }

    #[test]
    fn budgets_keys_and_addresses_are_separate() {
        let limiter = Limiter::new(2, 3);
        let t = 5_000;
        let (a, b) = (ip("198.51.100.1"), ip("198.51.100.2"));
        for _ in 0..2 {
            limiter.check(Budget::Holds, KEY, a, t).unwrap();
        }
        assert!(limiter.check(Budget::Holds, KEY, a, t).is_err());
        // A refusal after the burst says when the next one is admitted.
        assert_eq!(limiter.check(Budget::Holds, KEY, a, t), Err(Limited(30)));
        for _ in 0..3 {
            limiter.check(Budget::Reads, KEY, a, t).unwrap();
        }
        assert!(limiter.check(Budget::Reads, KEY, a, t).is_err());
        limiter.check(Budget::Holds, KEY, b, t).unwrap();
        limiter.check(Budget::Holds, KEY + 1, a, t).unwrap();
        // The same bits as IPv4 and as an IPv6 prefix are different clients.
        let v6 = ip("::c633:6401:0:0:0");
        limiter.check(Budget::Holds, KEY, v6, t).unwrap();
        // Without an address (no socket): one bucket per key.
        for _ in 0..2 {
            limiter.check(Budget::Holds, KEY, None, t).unwrap();
        }
        assert!(limiter.check(Budget::Holds, KEY, None, t).is_err());
    }

    #[test]
    fn a_zero_rate_turns_a_budget_off() {
        let limiter = Limiter::new(0, 1);
        for _ in 0..1_000 {
            limiter.check(Budget::Holds, KEY, None, 0).unwrap();
        }
        limiter.check(Budget::Reads, KEY, None, 0).unwrap();
        assert!(limiter.check(Budget::Reads, KEY, None, 0).is_err());
        assert_eq!(limiter.len(), 1);
    }

    #[test]
    fn the_buckets_are_bounded() {
        let limiter = Limiter::new(120, 3_000);
        let limit = SHARDS * SHARD_ENTRIES;
        let t = 1_000;
        // Many more addresses than fit: each is admitted, the map stops growing.
        for i in 0..(limit as u32 + 50_000) {
            let addr = Some(IpAddr::V4(i.into()));
            limiter.check(Budget::Reads, KEY, addr, t).unwrap();
        }
        assert_eq!(limiter.len(), limit);
        // An address with no bucket is not limited while the shards are full.
        let late = Some(IpAddr::V4(u32::MAX.into()));
        for _ in 0..4_000 {
            limiter.check(Budget::Reads, KEY, late, t).unwrap();
        }
        // Once the buckets have refilled (20 ms at 3,000 a minute), the next
        // sweep of a shard drops them and new clients are counted again.
        for _ in 0..3_000 {
            limiter.check(Budget::Reads, KEY, late, t + 2_000).unwrap();
        }
        assert!(limiter.check(Budget::Reads, KEY, late, t + 2_000).is_err());
        assert_eq!(limiter.len(), limit - SHARD_ENTRIES + 1);
        // The memory bound: the map's slots hold a key and a time each.
        assert_eq!(size_of::<(Key, u64)>(), 40);
    }
}
