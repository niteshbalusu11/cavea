//! An in-memory [`SeatStore`] with scriptable faults, for tests and the
//! simulation harness (13 §3.2).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard, PoisonError};

use tokio::sync::oneshot;

use crate::store::{Batch, HoldRow, LoadedEvent, ObjectRow, SeatStore, StoreError};
use crate::types::{EventId, HoldId, Oid};

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
}

#[derive(Debug, Default)]
struct StoredEvent {
    epoch: u64,
    seq: u64,
    holds: BTreeMap<HoldId, HoldRow>,
    objects: BTreeMap<Oid, ObjectRow>,
}

#[derive(Debug, Default)]
struct Inner {
    events: HashMap<EventId, StoredEvent>,
    faults: VecDeque<Fault>,
    committed: Vec<Batch>,
}

#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: Mutex<Inner>,
}

impl MemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Create an empty event owned at `epoch`.
    pub fn create_event(&self, event: EventId, epoch: u64) {
        self.lock().events.insert(
            event,
            StoredEvent {
                epoch,
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

    pub fn push_fault(&self, fault: Fault) {
        self.lock().faults.push_back(fault);
    }

    /// Every batch written, in commit order.
    #[must_use]
    pub fn committed(&self) -> Vec<Batch> {
        self.lock().committed.clone()
    }
}

impl SeatStore for MemoryStore {
    async fn load_event(&self, event: EventId) -> Result<LoadedEvent, StoreError> {
        let inner = self.lock();
        let e = inner.events.get(&event).ok_or(StoreError::UnknownEvent)?;
        Ok(LoadedEvent {
            epoch: e.epoch,
            seq: e.seq,
            holds: e.holds.values().cloned().collect(),
            objects: e.objects.values().cloned().collect(),
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
            None => false,
        };
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
        e.seq = batch.seq;
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
        inner.committed.push(batch);
        if lose_reply {
            return Err(StoreError::Unavailable("injected lost reply".into()));
        }
        Ok(())
    }
}

/// INV-5: the log continues the stored `seq` with no gap or repeat.
fn check_seqs(stored: u64, batch: &Batch) -> Result<(), StoreError> {
    let contiguous = batch
        .log
        .iter()
        .zip(stored + 1..)
        .all(|(record, expected)| record.seq == expected);
    let last = stored + batch.log.len() as u64;
    if batch.log.is_empty() || !contiguous || batch.seq != last {
        return Err(StoreError::Inconsistent(
            "log does not continue the stored seq".into(),
        ));
    }
    Ok(())
}
