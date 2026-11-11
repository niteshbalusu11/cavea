//! The in-memory idempotency window (08 §6.1): the replies of recent keyed
//! commands, bounded in count, with records kept at least a minute.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::store::IdempotencyRecord;
use crate::types::{Idempotency, Reply, Timestamp, limits};

#[derive(Debug, Default)]
pub(crate) struct Window {
    records: HashMap<Arc<str>, IdempotencyRecord>,
    /// Insertion order. A key replaced after 24 hours appears twice; the stale
    /// entry no longer matches its record's `created_at`.
    order: VecDeque<(Timestamp, Arc<str>)>,
}

pub(crate) enum Lookup {
    /// The key's original reply.
    Replay(Reply),
    Mismatch,
    /// No room for a new record: every record is younger than the minimum
    /// retention.
    Full,
    Miss,
}

fn age_at_least(created: Timestamp, now: Timestamp, ms: u64) -> bool {
    now.0 >= created.0.saturating_add(ms)
}

impl Window {
    /// Records from the store, oldest first; only the newest that fit are kept.
    pub(crate) fn restore(mut records: Vec<IdempotencyRecord>) -> Self {
        records.sort_by_key(|r| r.created_at);
        let skip = records.len().saturating_sub(limits::IDEMPOTENCY_WINDOW);
        let mut window = Self::default();
        for record in records.into_iter().skip(skip) {
            window.insert(record);
        }
        window
    }

    pub(crate) fn check(&mut self, key: &Idempotency, now: Timestamp) -> Lookup {
        if let Some(r) = self.records.get(&key.key)
            && !age_at_least(r.created_at, now, limits::IDEMPOTENCY_TTL_MS)
        {
            return if r.fingerprint == key.fingerprint {
                Lookup::Replay(r.reply.clone())
            } else {
                Lookup::Mismatch
            };
        }
        self.evict(now);
        if self.order.len() >= limits::IDEMPOTENCY_WINDOW {
            Lookup::Full
        } else {
            Lookup::Miss
        }
    }

    pub(crate) fn insert(&mut self, record: IdempotencyRecord) {
        self.order
            .push_back((record.created_at, record.key.clone()));
        self.records.insert(record.key.clone(), record);
    }

    /// Drop expired records, then the oldest ones past the minimum retention
    /// while the window is full.
    fn evict(&mut self, now: Timestamp) {
        while let Some((created, key)) = self.order.front() {
            let expired = age_at_least(*created, now, limits::IDEMPOTENCY_TTL_MS);
            let make_room = self.order.len() >= limits::IDEMPOTENCY_WINDOW
                && age_at_least(*created, now, limits::IDEMPOTENCY_MIN_RETENTION_MS);
            if !expired && !make_room {
                break;
            }
            if self
                .records
                .get(key)
                .is_some_and(|r| r.created_at == *created)
            {
                self.records.remove(key);
            }
            self.order.pop_front();
        }
    }
}
