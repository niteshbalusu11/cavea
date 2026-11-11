//! The engine's metrics (08 §15), and the two things every metric of the
//! server is made of: a fixed-bucket [`Histogram`] of atomics, and the
//! Prometheus text format ([`Exposition`]). Recording is a few relaxed atomic
//! additions and never allocates, locks or fails. The engine records into one
//! process-wide [`Metrics`]; the server renders it with its own.

use std::fmt::{self, Write as _};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use crate::types::{Failure, Op};

/// Bucket bounds of every duration histogram, in microseconds: 0.5 ms to
/// 10 s, with the hold budget's 15 ms among them (01 §5).
pub const DURATION_BUCKETS_US: &[u64] = &[
    500, 1_000, 2_500, 5_000, 10_000, 15_000, 25_000, 50_000, 100_000, 250_000, 500_000, 1_000_000,
    2_500_000, 5_000_000, 10_000_000,
];

/// Bucket bounds of the commit batch size histogram, in commands; a batch
/// holds at most 2,000 (08 §4.3).
pub const BATCH_BUCKETS: &[u64] = &[1, 2, 5, 10, 20, 50, 100, 200, 500, 1_000, 2_000];

/// Buckets a histogram may have, `+Inf` included.
const MAX_BUCKETS: usize = 16;

/// A Prometheus histogram with fixed buckets, recorded with atomics.
#[derive(Debug)]
pub struct Histogram {
    /// Upper bounds, ascending, in the unit observed.
    bounds: &'static [u64],
    /// Units observed per unit exposed: `1e6` for microseconds as seconds.
    per_unit: f64,
    /// Per bucket, not cumulative; the last is `+Inf`.
    counts: [AtomicU64; MAX_BUCKETS],
    sum: AtomicU64,
}

impl Histogram {
    /// Durations, exposed in seconds, with [`DURATION_BUCKETS_US`].
    #[must_use]
    pub const fn seconds() -> Self {
        Self::new(DURATION_BUCKETS_US, 1e6)
    }

    /// Observations in `bounds` (at most 15), exposed divided by `per_unit`.
    #[must_use]
    pub const fn new(bounds: &'static [u64], per_unit: f64) -> Self {
        assert!(bounds.len() < MAX_BUCKETS);
        Self {
            bounds,
            per_unit,
            counts: [const { AtomicU64::new(0) }; MAX_BUCKETS],
            sum: AtomicU64::new(0),
        }
    }

    pub fn observe(&self, value: u64) {
        let bucket = self
            .bounds
            .iter()
            .position(|&bound| value <= bound)
            .unwrap_or(self.bounds.len());
        if let Some(count) = self.counts.get(bucket) {
            count.fetch_add(1, Ordering::Relaxed);
        }
        self.sum.fetch_add(value, Ordering::Relaxed);
    }

    /// A duration histogram's observation.
    pub fn observe_duration(&self, elapsed: Duration) {
        self.observe(u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX));
    }

    /// Observations so far.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.counts.iter().map(|c| c.load(Ordering::Relaxed)).sum()
    }
}

/// Every result of a command: `ok`, or a failure's code (08 §6.4).
const RESULTS: [&str; 12] = [
    "ok",
    "invalid_request",
    "forbidden",
    "not_found",
    "object_not_free",
    "object_not_held_by_token",
    "object_status_conflict",
    "idempotency_mismatch",
    "hold_token_expired",
    "hold_limit_exceeded",
    "overloaded",
    "unavailable",
];

const OPS: [Op; 7] = [
    Op::CreateHold,
    Op::Hold,
    Op::Release,
    Op::ExtendHold,
    Op::Book,
    Op::ChangeStatus,
    Op::Expire,
];

fn op_index(op: Op) -> usize {
    op as usize - 1
}

/// The index in [`RESULTS`].
fn result_index(result: Result<(), &Failure>) -> usize {
    match result {
        Ok(()) => 0,
        Err(Failure::InvalidRequest(_)) => 1,
        Err(Failure::Forbidden) => 2,
        Err(Failure::NotFound(_)) => 3,
        Err(Failure::ObjectNotFree(_)) => 4,
        Err(Failure::ObjectNotHeldByToken(_)) => 5,
        Err(Failure::ObjectStatusConflict(_)) => 6,
        Err(Failure::IdempotencyMismatch) => 7,
        Err(Failure::HoldTokenExpired) => 8,
        Err(Failure::HoldLimitExceeded { .. }) => 9,
        Err(Failure::Overloaded) => 10,
        Err(Failure::Unavailable) => 11,
    }
}

/// What the engine records. One per process: [`METRICS`].
#[derive(Debug)]
pub struct Metrics {
    commands: [[AtomicU64; RESULTS.len()]; OPS.len()],
    /// From submission to the reply's release, per op.
    command_seconds: [Histogram; OPS.len()],
    /// From submission until the actor takes the command.
    mailbox_wait: Histogram,
    pub(crate) commit_seconds: Histogram,
    pub(crate) commit_batch: Histogram,
    pub(crate) holds: AtomicI64,
    /// From a publication's broadcast until nine in ten of the subscribers it
    /// went to have taken it.
    pub(crate) fanout: Histogram,
    pub(crate) publications: AtomicU64,
}

/// The process's engine metrics.
pub static METRICS: Metrics = Metrics {
    commands: [const { [const { AtomicU64::new(0) }; RESULTS.len()] }; OPS.len()],
    command_seconds: [const { Histogram::seconds() }; OPS.len()],
    mailbox_wait: Histogram::seconds(),
    commit_seconds: Histogram::seconds(),
    commit_batch: Histogram::new(BATCH_BUCKETS, 1.0),
    holds: AtomicI64::new(0),
    fanout: Histogram::seconds(),
    publications: AtomicU64::new(0),
};

impl Metrics {
    /// A command's outcome, and how long it took if it was submitted.
    pub(crate) fn command(&self, op: Op, result: Result<(), &Failure>, took: Option<Duration>) {
        let (o, r) = (op_index(op), result_index(result));
        if let Some(count) = self.commands.get(o).and_then(|row| row.get(r)) {
            count.fetch_add(1, Ordering::Relaxed);
        }
        if let (Some(took), Some(h)) = (took, self.command_seconds.get(o)) {
            h.observe_duration(took);
        }
    }

    pub(crate) fn mailbox_wait(&self, waited: Duration) {
        self.mailbox_wait.observe_duration(waited);
    }

    /// Write every engine metric, with the registry's loaded actors and the
    /// commands waiting in their mailboxes ([`crate::Registry::load`]).
    pub fn render(&self, out: &mut Exposition, (actors_loaded, mailbox_depth): (usize, usize)) {
        out.family(
            "cavea_commands_total",
            "counter",
            "Commands by op and result: ok, or the error code.",
        );
        for (op, row) in OPS.iter().zip(&self.commands) {
            for (result, count) in RESULTS.iter().zip(row) {
                let n = count.load(Ordering::Relaxed);
                if n > 0 {
                    out.sample(
                        "cavea_commands_total",
                        &[("op", op.name()), ("result", result)],
                        n,
                    );
                }
            }
        }
        out.family(
            "cavea_command_duration_seconds",
            "histogram",
            "From a command's submission to its reply, after its commit.",
        );
        for (op, h) in OPS.iter().zip(&self.command_seconds) {
            if h.count() > 0 {
                out.histogram("cavea_command_duration_seconds", &[("op", op.name())], h);
            }
        }
        out.histogram_family(
            "cavea_actor_mailbox_wait_seconds",
            "From a command's submission until its event's actor takes it.",
            &self.mailbox_wait,
        );
        out.histogram_family(
            "cavea_commit_duration_seconds",
            "One batch commit, from its start to the store's answer.",
            &self.commit_seconds,
        );
        out.histogram_family(
            "cavea_commit_batch_size",
            "Commands written by one batch commit.",
            &self.commit_batch,
        );
        out.gauge(
            "cavea_actor_mailbox_depth",
            "Commands waiting in event actors' mailboxes.",
            mailbox_depth,
        );
        out.gauge(
            "cavea_actors_loaded",
            "Event actors loaded on this node.",
            actors_loaded,
        );
        out.gauge(
            "cavea_holds_active",
            "Live holds in loaded events.",
            self.holds.load(Ordering::Relaxed).max(0),
        );
        out.counter(
            "cavea_rt_publications_total",
            "Publications of deltas to realtime subscribers.",
            self.publications.load(Ordering::Relaxed),
        );
        out.histogram_family(
            "cavea_rt_fanout_seconds",
            "From a publication's broadcast until nine in ten of its subscribers had taken it.",
            &self.fanout,
        );
    }
}

/// A Prometheus text exposition (format 0.0.4), written into a string.
#[derive(Debug, Default)]
pub struct Exposition {
    out: String,
}

impl Exposition {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.out
    }

    /// `# HELP` and `# TYPE` for a metric whose samples follow.
    pub fn family(&mut self, name: &str, kind: &str, help: &str) {
        let _ = writeln!(self.out, "# HELP {name} {help}\n# TYPE {name} {kind}");
    }

    /// One sample. Label values are escaped.
    pub fn sample(&mut self, name: &str, labels: &[(&str, &str)], value: impl fmt::Display) {
        self.out.push_str(name);
        self.labels(labels, None);
        let _ = writeln!(self.out, " {value}");
    }

    pub fn counter(&mut self, name: &str, help: &str, value: impl fmt::Display) {
        self.family(name, "counter", help);
        self.sample(name, &[], value);
    }

    pub fn gauge(&mut self, name: &str, help: &str, value: impl fmt::Display) {
        self.family(name, "gauge", help);
        self.sample(name, &[], value);
    }

    pub fn histogram_family(&mut self, name: &str, help: &str, h: &Histogram) {
        self.family(name, "histogram", help);
        self.histogram(name, &[], h);
    }

    /// One histogram's `_bucket` (cumulative), `_sum` and `_count` samples.
    pub fn histogram(&mut self, name: &str, labels: &[(&str, &str)], h: &Histogram) {
        let mut cumulative = 0;
        let bounds = h.bounds.iter().map(Some).chain([None]);
        for (bound, count) in bounds.zip(&h.counts) {
            cumulative += count.load(Ordering::Relaxed);
            let le = match bound {
                Some(&b) => format!("{}", b as f64 / h.per_unit),
                None => "+Inf".to_owned(),
            };
            let _ = write!(self.out, "{name}_bucket");
            self.labels(labels, Some(&le));
            let _ = writeln!(self.out, " {cumulative}");
        }
        let sum = h.sum.load(Ordering::Relaxed) as f64 / h.per_unit;
        let _ = write!(self.out, "{name}_sum");
        self.labels(labels, None);
        let _ = writeln!(self.out, " {sum}");
        let _ = write!(self.out, "{name}_count");
        self.labels(labels, None);
        let _ = writeln!(self.out, " {cumulative}");
    }

    fn labels(&mut self, labels: &[(&str, &str)], le: Option<&str>) {
        let le = le.map(|le| ("le", le));
        let mut all = labels.iter().copied().chain(le).peekable();
        if all.peek().is_none() {
            return;
        }
        self.out.push('{');
        for (i, (key, value)) in all.enumerate() {
            if i > 0 {
                self.out.push(',');
            }
            let _ = write!(self.out, "{key}=\"");
            for c in value.chars() {
                match c {
                    '\\' => self.out.push_str("\\\\"),
                    '"' => self.out.push_str("\\\""),
                    '\n' => self.out.push_str("\\n"),
                    c => self.out.push(c),
                }
            }
            self.out.push('"');
        }
        self.out.push('}');
    }
}
