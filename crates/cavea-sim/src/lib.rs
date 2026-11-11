//! The seat engine's reference model and model-based tests (13 §3.1).
//!
//! - [`model`]: a reference model of the seat state machine, written from the
//!   specs for obviousness (maps, one command at a time), and a replay of the
//!   committed log that checks every transition (INV-5, INV-6, INV-10).
//! - [`case`]: model-based property tests. A seeded generator writes command
//!   sequences, each runs against the model and the real `EventState`, and a
//!   failing one shrinks to a small case that can be saved as a text file and
//!   replayed.
//!
//! Every check returns a [`Violation`] rather than panicking, so that a failing
//! case can be shrunk.

pub mod case;
pub mod model;

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use cavea_engine::{ChartView, EventId, Idx, Oid};

/// The event every run uses.
pub const EVENT: EventId = EventId(7);

/// The `oid` of object `idx`: a permutation, so that ordering by `oid` and by
/// `idx` differ and a mix-up between them shows. Valid for `idx < 97`.
#[must_use]
pub fn oid(idx: Idx) -> Oid {
    1_000 + (idx * 7) % 97
}

/// The inverse of [`oid`].
#[must_use]
pub fn idx_of(oid: Oid) -> Option<Idx> {
    (0..97).find(|&i| self::oid(i) == oid)
}

/// The most objects a generated chart has.
pub const MAX_OBJECTS: u32 = 64;

/// A chart of `n` objects labelled `S-0` to `S-{n-1}`.
pub fn chart(n: u32) -> Result<Arc<ChartView>, Violation> {
    let view = ChartView::new((0..n.min(MAX_OBJECTS)).map(|i| (oid(i), format!("S-{i}"))))
        .map_err(|e| Violation::new("setup", format!("chart: {e}")))?;
    Ok(Arc::new(view))
}

/// A check that failed: a short stable kind (what shrinking preserves) and
/// what was seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub kind: &'static str,
    pub detail: String,
}

impl Violation {
    pub fn new(kind: &'static str, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for Violation {}

/// `Err(Violation)` unless `cond`.
macro_rules! ensure {
    ($cond:expr, $kind:expr, $($fmt:tt)+) => {
        if !$cond {
            return Err($crate::Violation::new($kind, format!($($fmt)+)));
        }
    };
}
pub(crate) use ensure;

/// SplitMix64: small, fast and the same everywhere.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`; 0 when `n` is 0.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }

    /// True with probability `pct` percent.
    pub fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }

    /// An index chosen with the given weights.
    pub fn weighted(&mut self, weights: &[u64]) -> usize {
        let mut roll = self.below(weights.iter().sum());
        for (i, &w) in weights.iter().enumerate() {
            if roll < w {
                return i;
            }
            roll -= w;
        }
        0
    }

    pub fn pick<T: Copy>(&mut self, items: &[T]) -> Option<T> {
        items.get(self.below(items.len() as u64) as usize).copied()
    }
}

/// How often each fault fired and each check ran, so that a run cannot pass
/// by not looking.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub steps: u64,
    pub runs: u64,
    pub counts: BTreeMap<&'static str, u64>,
}

impl Stats {
    pub fn count(&mut self, what: &'static str) {
        self.add(what, 1);
    }

    pub fn add(&mut self, what: &'static str, n: u64) {
        *self.counts.entry(what).or_default() += n;
    }

    #[must_use]
    pub fn get(&self, what: &str) -> u64 {
        self.counts.get(what).copied().unwrap_or(0)
    }

    pub fn merge(&mut self, other: &Self) {
        self.steps += other.steps;
        self.runs += other.runs;
        for (k, v) in &other.counts {
            self.add(k, *v);
        }
    }
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "runs {}, steps {}", self.runs, self.steps)?;
        for (k, v) in &self.counts {
            writeln!(f, "  {k:<28} {v}")?;
        }
        Ok(())
    }
}

/// Poll a future once with a waker that does nothing.
pub fn poll_now<F: Future>(f: F) -> Option<F::Output> {
    let mut f = std::pin::pin!(f);
    match f.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(v) => Some(v),
        Poll::Pending => None,
    }
}
