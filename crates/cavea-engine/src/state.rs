//! The event's in-memory state and the synchronous state machine (08 §4.1,
//! §7.1). No I/O, no clock, no runtime: the caller supplies `now`, and
//! [`EventState::take_batch`] hands over what must be committed.

use std::collections::HashMap;
use std::sync::Arc;

use crate::chart::ChartView;
use crate::store::{Batch, HoldRow, LoadedEvent, LogRecord, ObjectRow};
use crate::types::{
    Command, EventId, Failure, HoldId, Idx, InvalidReason, Missing, ObjectFailure, ObjectReason,
    ObjectRef, Oid, Op, Outcome, Reply, Status, Timestamp, TokenHash, limits,
};

/// Memory no longer matches what the engine guarantees. The actor must stop
/// and the event be reloaded from the store (08 §4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvariantViolation(pub &'static str);

impl std::fmt::Display for InvariantViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "engine invariant violated: {}", self.0)
    }
}

impl std::error::Error for InvariantViolation {}

/// Stored state that cannot be mapped onto the chart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    UnknownOid(Oid),
    Inconsistent(&'static str),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownOid(oid) => write!(f, "stored object {oid} is not in the chart"),
            Self::Inconsistent(why) => write!(f, "stored state inconsistent: {why}"),
        }
    }
}

impl std::error::Error for LoadError {}

#[derive(Debug)]
struct Hold {
    id: HoldId,
    token: TokenHash,
    expires_at: Timestamp,
    max_objects: Option<u32>,
    objects: Vec<Idx>,
}

/// A read-only view of one hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoldView<'a> {
    pub id: HoldId,
    pub token: TokenHash,
    pub expires_at: Timestamp,
    pub max_objects: Option<u32>,
    /// Unordered.
    pub objects: &'a [Idx],
}

/// Changes applied to memory since the last [`EventState::take_batch`].
#[derive(Debug, Default)]
struct Pending {
    /// `holds.len()` when this batch opened; later slots were created in it.
    holds_before: usize,
    extended: Vec<u32>,
    objects: Vec<(Idx, u64)>,
    log: Vec<LogRecord>,
}

/// One event's seat state.
#[derive(Debug)]
pub struct EventState {
    event: EventId,
    chart: Arc<ChartView>,
    /// Per `idx`: domain status code.
    status: Vec<u8>,
    /// Per `idx`: hold slot + 1, or 0.
    holder: Vec<u32>,
    /// By slot. Holds are not deleted until expiry exists (ENG-008).
    holds: Vec<Hold>,
    by_token: HashMap<TokenHash, u32>,
    seq: u64,
    epoch: u64,
    pending: Pending,
}

enum Error {
    Refused(Failure),
    Corrupt(InvariantViolation),
}

impl From<Failure> for Error {
    fn from(f: Failure) -> Self {
        Self::Refused(f)
    }
}

impl From<InvariantViolation> for Error {
    fn from(v: InvariantViolation) -> Self {
        Self::Corrupt(v)
    }
}

fn corrupt(why: &'static str) -> InvariantViolation {
    InvariantViolation(why)
}

impl EventState {
    /// Build the state from what the store holds (08 §4.7 step 3).
    pub fn restore(
        event: EventId,
        chart: Arc<ChartView>,
        loaded: LoadedEvent,
    ) -> Result<Self, LoadError> {
        let n = chart.len();
        let mut state = Self {
            event,
            chart,
            status: vec![Status::FREE.code(); n],
            holder: vec![0; n],
            holds: Vec::with_capacity(loaded.holds.len()),
            by_token: HashMap::with_capacity(loaded.holds.len()),
            seq: loaded.seq,
            epoch: loaded.epoch,
            pending: Pending::default(),
        };
        let mut rows = loaded.holds;
        rows.sort_by_key(|h| h.id);
        let mut slot_of = HashMap::with_capacity(rows.len());
        for row in rows {
            let slot = u32::try_from(state.holds.len())
                .map_err(|_| LoadError::Inconsistent("too many holds"))?;
            if state.by_token.insert(row.token_hash, slot).is_some()
                || slot_of.insert(row.id, slot).is_some()
            {
                return Err(LoadError::Inconsistent("duplicate hold"));
            }
            state.holds.push(Hold {
                id: row.id,
                token: row.token_hash,
                expires_at: row.expires_at,
                max_objects: row.max_objects,
                objects: Vec::new(),
            });
        }
        for row in loaded.objects {
            let idx = state
                .chart
                .idx_of_oid(row.oid)
                .ok_or(LoadError::UnknownOid(row.oid))?;
            let i = idx as usize;
            match (row.status == Status::HELD, row.hold_id) {
                (true, Some(id)) => {
                    let slot = *slot_of
                        .get(&id)
                        .ok_or(LoadError::Inconsistent("object held by unknown hold"))?;
                    if let (Some(h), Some(hold)) =
                        (state.holder.get_mut(i), state.holds.get_mut(slot as usize))
                    {
                        *h = slot + 1;
                        hold.objects.push(idx);
                    }
                }
                (false, None) => {}
                _ => return Err(LoadError::Inconsistent("hold_id does not match status")),
            }
            if let Some(s) = state.status.get_mut(i) {
                *s = row.status.code();
            }
        }
        state.pending.holds_before = state.holds.len();
        Ok(state)
    }

    #[must_use]
    pub fn event(&self) -> EventId {
        self.event
    }

    #[must_use]
    pub fn chart(&self) -> &Arc<ChartView> {
        &self.chart
    }

    /// The sequence number of the last command applied to memory.
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq
    }

    #[must_use]
    pub fn status(&self, idx: Idx) -> Option<Status> {
        self.status
            .get(idx as usize)
            .and_then(|&c| Status::from_code(c))
    }

    /// The hold an object is held under.
    #[must_use]
    pub fn holder(&self, idx: Idx) -> Option<HoldId> {
        let slot = self.holder.get(idx as usize)?.checked_sub(1)?;
        self.holds.get(slot as usize).map(|h| h.id)
    }

    #[must_use]
    pub fn hold(&self, token: &TokenHash) -> Option<HoldView<'_>> {
        let slot = *self.by_token.get(token)?;
        self.holds.get(slot as usize).map(view)
    }

    pub fn holds(&self) -> impl Iterator<Item = HoldView<'_>> {
        self.holds.iter().map(view)
    }

    /// Validate `command` against memory at time `now` and, if it is accepted,
    /// apply it to memory and record it for the next batch. An `Err` means
    /// memory is corrupt and the state must be discarded.
    pub fn apply(
        &mut self,
        actor: &Arc<str>,
        command: &Command,
        now: Timestamp,
    ) -> Result<Reply, InvariantViolation> {
        let result = match command {
            Command::CreateHold {
                token,
                ttl_secs,
                max_objects,
            } => self.create_hold(actor, *token, *ttl_secs, *max_objects, now),
            Command::Hold { token, objects } => self.hold_objects(actor, token, objects, now),
            Command::Release { token, objects } => self.release(actor, token, objects, now),
            Command::ExtendHold { token, ttl_secs } => self.extend(actor, token, *ttl_secs, now),
        };
        match result {
            Ok(outcome) => Ok(Ok(outcome)),
            Err(Error::Refused(failure)) => Ok(Err(failure)),
            Err(Error::Corrupt(v)) => Err(v),
        }
    }

    /// Seal everything applied since the last call into one batch, or `None`
    /// if no command changed anything.
    pub fn take_batch(&mut self) -> Result<Option<Batch>, InvariantViolation> {
        let pending = std::mem::take(&mut self.pending);
        self.pending.holds_before = self.holds.len();
        let Some(last) = pending.log.last() else {
            return Ok(None);
        };
        let seq = last.seq;

        let mut objects = pending.objects;
        objects.sort_unstable();
        let mut objects_set = Vec::new();
        let mut objects_freed = Vec::new();
        for (n, &(idx, obj_seq)) in objects.iter().enumerate() {
            if objects.get(n + 1).is_some_and(|next| next.0 == idx) {
                continue; // keep the last change per object
            }
            let oid = self.chart.oid(idx).ok_or(corrupt("idx outside chart"))?;
            let status = self.status(idx).ok_or(corrupt("bad status"))?;
            if status == Status::FREE {
                objects_freed.push(oid);
            } else {
                objects_set.push(ObjectRow {
                    oid,
                    status,
                    hold_id: self.holder(idx),
                    seq: obj_seq,
                });
            }
        }

        let holds_created = self
            .holds
            .get(pending.holds_before..)
            .ok_or(corrupt("holds shrank"))?
            .iter()
            .map(|h| HoldRow {
                id: h.id,
                token_hash: h.token,
                expires_at: h.expires_at,
                max_objects: h.max_objects,
            })
            .collect();
        let mut extended = pending.extended;
        extended.sort_unstable();
        extended.dedup();
        let mut holds_extended = Vec::new();
        for slot in extended {
            if (slot as usize) < pending.holds_before {
                let h = self.holds.get(slot as usize).ok_or(corrupt("bad slot"))?;
                holds_extended.push((h.id, h.expires_at));
            }
        }

        Ok(Some(Batch {
            event: self.event,
            epoch: self.epoch,
            seq,
            holds_created,
            holds_extended,
            holds_deleted: Vec::new(),
            objects_set,
            objects_freed,
            log: pending.log,
        }))
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    fn log(&mut self, record: LogRecord) {
        self.pending.log.push(record);
    }

    fn oids(&self, idxs: &[Idx]) -> Result<Vec<Oid>, InvariantViolation> {
        idxs.iter()
            .map(|&i| self.chart.oid(i).ok_or(corrupt("idx outside chart")))
            .collect()
    }

    /// The slot of the token's hold, if it exists and has not expired.
    fn live_hold(&self, token: &TokenHash, now: Timestamp) -> Result<u32, Error> {
        let slot = *self
            .by_token
            .get(token)
            .ok_or(Failure::NotFound(Missing::HoldToken))?;
        let hold = self
            .holds
            .get(slot as usize)
            .ok_or(corrupt("token maps to no hold"))?;
        if now >= hold.expires_at {
            return Err(Failure::HoldTokenExpired.into());
        }
        Ok(slot)
    }

    fn hold_mut(&mut self, slot: u32) -> Result<&mut Hold, InvariantViolation> {
        self.holds
            .get_mut(slot as usize)
            .ok_or(corrupt("bad hold slot"))
    }

    /// Resolve references to unique `idx`, ascending.
    fn resolve(&self, objects: &[ObjectRef]) -> Result<Vec<Idx>, Failure> {
        let mut missing = Vec::new();
        let mut idxs = Vec::with_capacity(objects.len());
        for object in objects {
            match self.chart.resolve(object) {
                Some(idx) => idxs.push(idx),
                None => missing.push(object.clone()),
            }
        }
        if !missing.is_empty() {
            return Err(Failure::NotFound(Missing::Objects(missing)));
        }
        idxs.sort_unstable();
        idxs.dedup();
        Ok(idxs)
    }

    fn create_hold(
        &mut self,
        actor: &Arc<str>,
        token: TokenHash,
        ttl_secs: u32,
        max_objects: Option<u32>,
        now: Timestamp,
    ) -> Result<Outcome, Error> {
        check_ttl(ttl_secs)?;
        if max_objects == Some(0) {
            return Err(Failure::InvalidRequest(InvalidReason::ZeroObjectLimit).into());
        }
        if self.by_token.contains_key(&token) {
            return Err(Failure::InvalidRequest(InvalidReason::TokenAlreadyExists).into());
        }
        let slot = u32::try_from(self.holds.len()).map_err(|_| corrupt("too many holds"))?;
        let seq = self.next_seq();
        let id = HoldId(seq);
        let expires_at = now.plus_secs(ttl_secs);
        self.holds.push(Hold {
            id,
            token,
            expires_at,
            max_objects,
            objects: Vec::new(),
        });
        self.by_token.insert(token, slot);
        self.log(LogRecord {
            seq,
            op: Op::CreateHold,
            actor: actor.clone(),
            hold_id: Some(id),
            oids: Vec::new(),
            expires_at: Some(expires_at),
        });
        Ok(Outcome::HoldCreated {
            seq,
            hold: id,
            expires_at,
        })
    }

    fn hold_objects(
        &mut self,
        actor: &Arc<str>,
        token: &TokenHash,
        objects: &[ObjectRef],
        now: Timestamp,
    ) -> Result<Outcome, Error> {
        check_count(objects)?;
        let slot = self.live_hold(token, now)?;
        let idxs = self.resolve(objects)?;
        let mut failed = Vec::new();
        let mut new = Vec::new();
        for &idx in &idxs {
            let i = idx as usize;
            let status = *self.status.get(i).ok_or(corrupt("idx outside state"))?;
            let holder = *self.holder.get(i).ok_or(corrupt("idx outside state"))?;
            if holder == slot + 1 {
                if status != Status::HELD.code() {
                    return Err(corrupt("holder set on an object that is not held").into());
                }
            } else if status != Status::FREE.code() {
                failed.push(ObjectFailure {
                    idx,
                    reason: ObjectReason::NotFree,
                });
            } else {
                new.push(idx);
            }
        }
        if !failed.is_empty() {
            return Err(Failure::ObjectNotFree(failed).into());
        }
        let hold = self.hold_mut(slot)?;
        let (id, expires_at) = (hold.id, hold.expires_at);
        if let Some(limit) = hold.max_objects {
            let requested = (hold.objects.len() + new.len()) as u64;
            if requested > u64::from(limit) {
                return Err(Failure::HoldLimitExceeded { limit, requested }.into());
            }
        }
        if new.is_empty() {
            return Ok(Outcome::Held {
                seq: self.seq,
                expires_at,
                objects: idxs,
            });
        }
        hold.objects.extend_from_slice(&new);
        let oids = self.oids(&new)?;
        let seq = self.next_seq();
        for &idx in &new {
            self.set_object(idx, Status::HELD, slot + 1, seq)?;
        }
        self.log(LogRecord {
            seq,
            op: Op::Hold,
            actor: actor.clone(),
            hold_id: Some(id),
            oids,
            expires_at: None,
        });
        Ok(Outcome::Held {
            seq,
            expires_at,
            objects: idxs,
        })
    }

    fn release(
        &mut self,
        actor: &Arc<str>,
        token: &TokenHash,
        objects: &[ObjectRef],
        now: Timestamp,
    ) -> Result<Outcome, Error> {
        if objects.len() > limits::MAX_OBJECTS_PER_COMMAND {
            return Err(Failure::InvalidRequest(InvalidReason::TooManyObjects).into());
        }
        let slot = self.live_hold(token, now)?;
        let idxs = if objects.is_empty() {
            let mut all = self.hold_mut(slot)?.objects.clone();
            all.sort_unstable();
            all
        } else {
            let idxs = self.resolve(objects)?;
            let failed: Vec<_> = idxs
                .iter()
                .filter(|&&idx| self.holder.get(idx as usize) != Some(&(slot + 1)))
                .map(|&idx| ObjectFailure {
                    idx,
                    reason: ObjectReason::NotHeldByToken,
                })
                .collect();
            if !failed.is_empty() {
                return Err(Failure::ObjectNotHeldByToken(failed).into());
            }
            idxs
        };
        if idxs.is_empty() {
            return Ok(Outcome::Released {
                seq: self.seq,
                objects: idxs,
            });
        }
        let oids = self.oids(&idxs)?;
        let seq = self.next_seq();
        for &idx in &idxs {
            self.set_object(idx, Status::FREE, 0, seq)?;
        }
        let hold = self.hold_mut(slot)?;
        hold.objects.retain(|i| idxs.binary_search(i).is_err());
        let id = hold.id;
        self.log(LogRecord {
            seq,
            op: Op::Release,
            actor: actor.clone(),
            hold_id: Some(id),
            oids,
            expires_at: None,
        });
        Ok(Outcome::Released { seq, objects: idxs })
    }

    fn extend(
        &mut self,
        actor: &Arc<str>,
        token: &TokenHash,
        ttl_secs: u32,
        now: Timestamp,
    ) -> Result<Outcome, Error> {
        check_ttl(ttl_secs)?;
        let slot = self.live_hold(token, now)?;
        let seq = self.next_seq();
        let expires_at = now.plus_secs(ttl_secs);
        let hold = self.hold_mut(slot)?;
        hold.expires_at = expires_at;
        let id = hold.id;
        self.pending.extended.push(slot);
        self.log(LogRecord {
            seq,
            op: Op::ExtendHold,
            actor: actor.clone(),
            hold_id: Some(id),
            oids: Vec::new(),
            expires_at: Some(expires_at),
        });
        Ok(Outcome::Extended { seq, expires_at })
    }

    fn set_object(
        &mut self,
        idx: Idx,
        status: Status,
        holder: u32,
        seq: u64,
    ) -> Result<(), InvariantViolation> {
        let i = idx as usize;
        let (Some(s), Some(h)) = (self.status.get_mut(i), self.holder.get_mut(i)) else {
            return Err(corrupt("idx outside state"));
        };
        *s = status.code();
        *h = holder;
        self.pending.objects.push((idx, seq));
        Ok(())
    }
}

fn view(h: &Hold) -> HoldView<'_> {
    HoldView {
        id: h.id,
        token: h.token,
        expires_at: h.expires_at,
        max_objects: h.max_objects,
        objects: &h.objects,
    }
}

fn check_ttl(ttl_secs: u32) -> Result<(), Failure> {
    if (limits::MIN_HOLD_TTL_SECS..=limits::MAX_HOLD_TTL_SECS).contains(&ttl_secs) {
        Ok(())
    } else {
        Err(Failure::InvalidRequest(InvalidReason::TtlOutOfRange))
    }
}

fn check_count(objects: &[ObjectRef]) -> Result<(), Failure> {
    if objects.is_empty() {
        Err(Failure::InvalidRequest(InvalidReason::NoObjects))
    } else if objects.len() > limits::MAX_OBJECTS_PER_COMMAND {
        Err(Failure::InvalidRequest(InvalidReason::TooManyObjects))
    } else {
        Ok(())
    }
}
