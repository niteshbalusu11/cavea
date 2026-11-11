//! An in-memory [`SeatStore`] and [`ChartProvider`] with scriptable faults, for
//! tests and the simulation harness (13 §3.2).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::oneshot;

use crate::chart::ChartView;
use crate::store::{
    Acquired, Batch, ChartProvider, HoldRow, IdempotencyRecord, LoadedEvent, ObjectRow, SeatStore,
    StoreError,
};
use crate::types::{ChartVersionId, Clock, EventId, HoldId, ManualClock, Oid, Timestamp, limits};

/// The lease a [`MemoryStore`] grants, as PostgreSQL's default (08 §4.8).
pub const MEMORY_LEASE_MS: u64 = 15_000;

/// A fault applied to one commit. Faults are consumed in order, one per
/// `commit_batch` call.
#[derive(Debug)]
pub enum Fault {
    /// Write nothing and fail.
    Fail,
    /// Write the batch, then report failure (crash after commit).
    LoseReply,
    /// Wait until the sender fires or is dropped, then commit normally.
    Wait(oneshot::Receiver<()>),
    /// Panic inside the commit, as a bug in the actor's task would.
    Panic,
}

#[derive(Debug, Default)]
struct StoredEvent {
    epoch: u64,
    seq: u64,
    chart_version: Option<ChartVersionId>,
    owner: Option<Arc<str>>,
    lease_until: Option<Timestamp>,
    holds: BTreeMap<HoldId, HoldRow>,
    objects: BTreeMap<Oid, ObjectRow>,
    idempotency: HashMap<Arc<str>, IdempotencyRecord>,
}

#[derive(Debug, Default)]
struct Inner {
    events: HashMap<EventId, StoredEvent>,
    faults: VecDeque<Fault>,
    committed: Vec<Batch>,
}

/// One node's connection to a shared in-memory database. [`Self::peer`] opens
/// another node's connection to the same data. Leases follow the store's
/// [`ManualClock`], which plays the database clock.
#[derive(Debug)]
pub struct MemoryStore {
    inner: Arc<Mutex<Inner>>,
    node: Arc<str>,
    clock: ManualClock,
    /// While set, every call from this node fails as `unavailable`: the
    /// database is down or this node is cut off from it.
    down: AtomicBool,
    acquisitions: AtomicU64,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::with_clock(ManualClock::default())
    }
}

impl MemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A store for node `node-1` whose leases follow `clock`.
    #[must_use]
    pub fn with_clock(clock: ManualClock) -> Self {
        Self {
            inner: Arc::default(),
            node: "node-1".into(),
            clock,
            down: AtomicBool::new(false),
            acquisitions: AtomicU64::new(0),
        }
    }

    /// Another node's store on the same data and clock.
    #[must_use]
    pub fn peer(&self, node: &str) -> Self {
        Self {
            inner: self.inner.clone(),
            node: node.into(),
            clock: self.clock.clone(),
            down: AtomicBool::new(false),
            acquisitions: AtomicU64::new(0),
        }
    }

    /// The clock that plays the database's.
    #[must_use]
    pub fn clock(&self) -> &ManualClock {
        &self.clock
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Create an empty, unowned event at `epoch`, pinned to `chart_version`.
    pub fn create_event(&self, event: EventId, epoch: u64, chart_version: ChartVersionId) {
        self.lock().events.insert(
            event,
            StoredEvent {
                epoch,
                chart_version: Some(chart_version),
                ..StoredEvent::default()
            },
        );
    }

    /// Change the owner epoch, as another node acquiring the event would.
    pub fn set_owner_epoch(&self, event: EventId, epoch: u64) {
        if let Some(e) = self.lock().events.get_mut(&event) {
            e.epoch = epoch;
        }
    }

    /// The owner and lease expiry stored for the event.
    #[must_use]
    pub fn lease(&self, event: EventId) -> Option<(Option<Arc<str>>, Option<Timestamp>)> {
        let inner = self.lock();
        let e = inner.events.get(&event)?;
        Some((e.owner.clone(), e.lease_until))
    }

    pub fn push_fault(&self, fault: Fault) {
        self.lock().faults.push_back(fault);
    }

    pub fn set_down(&self, down: bool) {
        self.down.store(down, Ordering::SeqCst);
    }

    /// How many times this node has tried to acquire an event.
    #[must_use]
    pub fn acquisitions(&self) -> u64 {
        self.acquisitions.load(Ordering::SeqCst)
    }

    /// Every batch written, in commit order.
    #[must_use]
    pub fn committed(&self) -> Vec<Batch> {
        self.lock().committed.clone()
    }

    fn check_up(&self) -> Result<(), StoreError> {
        if self.down.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable("injected outage".into()));
        }
        Ok(())
    }

    fn lease_until(&self) -> Timestamp {
        Timestamp(self.clock.now().0.saturating_add(MEMORY_LEASE_MS))
    }

    /// Run `f` on the event if it is owned at `epoch`.
    fn owned(
        &self,
        event: EventId,
        epoch: u64,
        f: impl FnOnce(&mut StoredEvent),
    ) -> Result<(), StoreError> {
        self.check_up()?;
        let mut inner = self.lock();
        let e = inner
            .events
            .get_mut(&event)
            .ok_or(StoreError::UnknownEvent)?;
        if e.epoch != epoch {
            return Err(StoreError::Fenced);
        }
        f(e);
        Ok(())
    }
}

impl SeatStore for MemoryStore {
    async fn acquire(&self, event: EventId) -> Result<Acquired, StoreError> {
        self.acquisitions.fetch_add(1, Ordering::SeqCst);
        self.check_up()?;
        let now = self.clock.now();
        let lease_until = self.lease_until();
        let mut inner = self.lock();
        let e = inner
            .events
            .get_mut(&event)
            .ok_or(StoreError::UnknownEvent)?;
        let free = e.owner.as_deref() == Some(&*self.node)
            || e.lease_until.is_none_or(|until| until < now);
        if !free {
            return Err(StoreError::Leased);
        }
        e.epoch += 1;
        e.owner = Some(self.node.clone());
        e.lease_until = Some(lease_until);
        let chart_version = e
            .chart_version
            .ok_or_else(|| StoreError::Inconsistent("event has no chart version".into()))?;
        Ok(Acquired {
            epoch: e.epoch,
            chart_version,
        })
    }

    async fn load_event(&self, event: EventId) -> Result<LoadedEvent, StoreError> {
        self.check_up()?;
        let inner = self.lock();
        let e = inner.events.get(&event).ok_or(StoreError::UnknownEvent)?;
        Ok(LoadedEvent {
            epoch: e.epoch,
            seq: e.seq,
            holds: e.holds.values().cloned().collect(),
            objects: e.objects.values().cloned().collect(),
            idempotency: e.idempotency.values().cloned().collect(),
        })
    }

    async fn commit_batch(&self, batch: Batch) -> Result<(), StoreError> {
        let fault = self.lock().faults.pop_front();
        let lose_reply = match fault {
            Some(Fault::Fail) => return Err(StoreError::Unavailable("injected failure".into())),
            Some(Fault::Wait(gate)) => {
                let _ = gate.await;
                false
            }
            Some(Fault::LoseReply) => true,
            Some(Fault::Panic) => panic!("injected panic in commit"),
            None => false,
        };
        self.check_up()?;
        let lease_until = self.lease_until();
        let mut inner = self.lock();
        let e = inner
            .events
            .get_mut(&batch.event)
            .ok_or(StoreError::UnknownEvent)?;
        if e.epoch != batch.epoch {
            return Err(StoreError::Fenced);
        }
        check_seqs(e.seq, &batch)?;
        // Validate everything before the first write: a commit is all or nothing.
        let known =
            |id| e.holds.contains_key(id) || batch.holds_created.iter().any(|h| h.id == *id);
        if !batch.holds_extended.iter().all(|(id, _)| known(id)) {
            return Err(StoreError::Inconsistent("extended an unknown hold".into()));
        }
        if !batch
            .holds_deleted
            .iter()
            .all(|id| e.holds.contains_key(id))
        {
            return Err(StoreError::Inconsistent("deleted an unknown hold".into()));
        }
        // The counts must follow the rows, as `events.counts` does in
        // PostgreSQL: an engine that miscounted is caught here.
        let mut counts = [0i32; 16];
        let set = batch.objects_set.iter().map(|o| (o.oid, o.status.code()));
        for (oid, now) in set.chain(batch.objects_freed.iter().map(|&oid| (oid, 0))) {
            let was = e.objects.get(&oid).map_or(0, |o| o.status.code());
            if was != now {
                if let Some(c) = counts.get_mut(usize::from(was)) {
                    *c -= 1;
                }
                if let Some(c) = counts.get_mut(usize::from(now)) {
                    *c += 1;
                }
            }
        }
        if counts != batch.counts {
            return Err(StoreError::Inconsistent(
                "status counts do not follow the objects".into(),
            ));
        }
        e.seq = batch.seq;
        e.lease_until = Some(lease_until);
        for h in &batch.holds_created {
            e.holds.insert(h.id, h.clone());
        }
        for (id, expires_at) in &batch.holds_extended {
            if let Some(h) = e.holds.get_mut(id) {
                h.expires_at = *expires_at;
            }
        }
        for id in &batch.holds_deleted {
            e.holds.remove(id);
        }
        for o in &batch.objects_set {
            e.objects.insert(o.oid, o.clone());
        }
        for oid in &batch.objects_freed {
            e.objects.remove(oid);
        }
        for r in &batch.idempotency {
            // As `ON CONFLICT DO UPDATE ... WHERE` the stored record is over 24
            // hours older: a live record is never overwritten.
            let replace = e.idempotency.get(&r.key).is_none_or(|old| {
                r.created_at.0 >= old.created_at.0.saturating_add(limits::IDEMPOTENCY_TTL_MS)
            });
            if replace {
                e.idempotency.insert(r.key.clone(), r.clone());
            }
        }
        inner.committed.push(batch);
        if lose_reply {
            return Err(StoreError::Unavailable("injected lost reply".into()));
        }
        Ok(())
    }

    async fn heartbeat(&self, event: EventId, epoch: u64) -> Result<(), StoreError> {
        let lease_until = self.lease_until();
        self.owned(event, epoch, |e| e.lease_until = Some(lease_until))
    }

    async fn release(&self, event: EventId, epoch: u64) -> Result<(), StoreError> {
        self.owned(event, epoch, |e| e.lease_until = None)
    }
}

/// INV-5: the log continues the stored `seq` with no gap or repeat. A batch
/// without log rows carries only idempotency records and keeps the `seq`.
fn check_seqs(stored: u64, batch: &Batch) -> Result<(), StoreError> {
    let contiguous = batch
        .log
        .iter()
        .zip(stored + 1..)
        .all(|(record, expected)| record.seq == expected);
    let last = stored + batch.log.len() as u64;
    let empty = batch.log.is_empty() && batch.idempotency.is_empty();
    if empty || !contiguous || batch.seq != last {
        return Err(StoreError::Inconsistent(
            "log does not continue the stored seq".into(),
        ));
    }
    Ok(())
}

/// Charts by version, all in memory.
#[derive(Debug, Default)]
pub struct MemoryCharts {
    charts: Mutex<HashMap<ChartVersionId, Arc<ChartView>>>,
}

impl MemoryCharts {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, version: ChartVersionId, chart: Arc<ChartView>) {
        self.charts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(version, chart);
    }
}

impl ChartProvider for MemoryCharts {
    async fn chart(&self, version: ChartVersionId) -> Result<Arc<ChartView>, StoreError> {
        self.charts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&version)
            .cloned()
            .ok_or_else(|| StoreError::Inconsistent("unknown chart version".into()))
    }
}
