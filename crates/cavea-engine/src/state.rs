//! The event's in-memory state and the synchronous state machine (08 §4.1,
//! §7). No I/O, no clock, no runtime: the caller supplies `now`, and
//! [`EventState::take_batch`] hands over what must be committed.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;

use cavea_format::wire::Change;

use crate::chart::ChartView;
use crate::feed::Sealed;
use crate::idempotency::{Lookup, Window};
use crate::store::{Batch, Booking, HoldRow, IdempotencyRecord, LoadedEvent, LogRecord, ObjectRow};
use crate::types::{
    BookDetail, Command, EventId, Failure, HoldId, Idx, InvalidReason, Missing, ObjectFailure,
    ObjectReason, ObjectRef, Oid, Op, Outcome, Reply, Request, Status, Timestamp, TokenHash,
    limits,
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
    /// Holds created, extended or deleted, with the slot each was in.
    holds: Vec<(HoldId, u32)>,
    /// Holds created in this batch.
    created: Vec<HoldId>,
    /// `(idx, seq, status code before the change)`, one per change.
    objects: Vec<(Idx, u64, u8)>,
    log: Vec<LogRecord>,
    idempotency: Vec<IdempotencyRecord>,
    /// Tokens whose hold a committed command changed, for PRIVATE frames.
    tokens: Vec<TokenHash>,
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
    /// By slot; a deleted hold leaves `None` until the slot is reused.
    holds: Vec<Option<Hold>>,
    free_slots: Vec<u32>,
    by_token: HashMap<TokenHash, u32>,
    /// `(expires_at, id, slot)`. Entries whose hold was deleted or extended
    /// are stale and skipped when they surface.
    expiry: BinaryHeap<Reverse<(Timestamp, HoldId, u32)>>,
    /// Booking data, only for booked objects: free seats cost nothing.
    booked: HashMap<Idx, Booking>,
    window: Window,
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

/// What a command does to one object it names.
enum Check {
    /// Already in the target state: a no-op for this object.
    Skip,
    Change,
    Fail(ObjectReason),
}

impl EventState {
    /// Build the state from what the store holds (08 §4.7 steps 3 and 4).
    /// Holds already past their expiry are scheduled and expire on the next
    /// [`Self::apply`] or [`Self::expire_due`].
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
            free_slots: Vec::new(),
            by_token: HashMap::with_capacity(loaded.holds.len()),
            expiry: BinaryHeap::with_capacity(loaded.holds.len()),
            booked: HashMap::new(),
            window: Window::restore(loaded.idempotency),
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
            state.expiry.push(Reverse((row.expires_at, row.id, slot)));
            state.holds.push(Some(Hold {
                id: row.id,
                token: row.token_hash,
                expires_at: row.expires_at,
                max_objects: row.max_objects,
                objects: Vec::new(),
            }));
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
                    if let (Some(h), Some(Some(hold))) =
                        (state.holder.get_mut(i), state.holds.get_mut(slot as usize))
                    {
                        *h = slot + 1;
                        hold.objects.push(idx);
                    }
                }
                (false, None) => {}
                _ => return Err(LoadError::Inconsistent("hold_id does not match status")),
            }
            match (row.status == Status::BOOKED, row.booking) {
                (true, Some(booking)) => {
                    state.booked.insert(idx, booking);
                }
                (false, None) => {}
                _ => return Err(LoadError::Inconsistent("booking does not match status")),
            }
            if let Some(s) = state.status.get_mut(i) {
                *s = row.status.code();
            }
        }
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

    /// The ownership epoch every batch of this state is fenced on.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The sequence number of the last command applied to memory.
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Every object's domain status code, by `idx`.
    pub(crate) fn statuses(&self) -> &[u8] {
        &self.status
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
        self.slot(slot).map(|h| h.id)
    }

    /// A booked object's order reference, ticket type and `extra`.
    #[must_use]
    pub fn booking(&self, idx: Idx) -> Option<&Booking> {
        self.booked.get(&idx)
    }

    #[must_use]
    pub fn hold(&self, token: &TokenHash) -> Option<HoldView<'_>> {
        self.slot(*self.by_token.get(token)?).map(view)
    }

    /// Live holds, committed or not.
    #[must_use]
    pub fn hold_count(&self) -> usize {
        self.by_token.len()
    }

    pub fn holds(&self) -> impl Iterator<Item = HoldView<'_>> {
        self.holds.iter().flatten().map(view)
    }

    /// When the earliest live hold expires. Drops stale schedule entries.
    pub fn next_expiry(&mut self) -> Option<Timestamp> {
        while let Some(&Reverse((at, id, slot))) = self.expiry.peek() {
            if self
                .slot(slot)
                .is_some_and(|h| h.id == id && h.expires_at == at)
            {
                return Some(at);
            }
            self.expiry.pop();
        }
        None
    }

    /// Expire every hold whose expiry is at or before `now`, in expiry order
    /// (ties by hold id). Each expiry is a command of its own: it takes a
    /// `seq`, frees the hold's objects and deletes the hold. Returns how many
    /// expired.
    pub fn expire_due(&mut self, now: Timestamp) -> Result<usize, InvariantViolation> {
        let mut expired = 0;
        while let Some(at) = self.next_expiry() {
            if at > now {
                break;
            }
            let Some(Reverse((_, _, slot))) = self.expiry.pop() else {
                break;
            };
            self.expire(slot)?;
            expired += 1;
        }
        Ok(expired)
    }

    /// Apply the expiries due at `now`, then validate the request against
    /// memory and, if it is accepted, apply it and record it for the next
    /// batch. A request whose idempotency key is already known is answered
    /// from its record. An `Err` means memory is corrupt and the state must be
    /// discarded.
    pub fn apply(
        &mut self,
        request: &Request,
        now: Timestamp,
    ) -> Result<Reply, InvariantViolation> {
        self.expire_due(now)?;
        if let Some(key) = &request.idempotency {
            if let Err(refused) = check_text(
                &key.key,
                limits::MAX_IDEMPOTENCY_KEY_CHARS,
                InvalidReason::IdempotencyKeyLength,
            ) {
                return Ok(Err(refused));
            }
            match self.window.check(key, now) {
                Lookup::Replay(reply) => return Ok(reply),
                Lookup::Mismatch => return Ok(Err(Failure::IdempotencyMismatch)),
                Lookup::Full => return Ok(Err(Failure::Overloaded)),
                Lookup::Miss => {}
            }
        }
        let reply = if request.command.permits(request.authority) {
            match self.execute(&request.actor, &request.command, now) {
                Ok(outcome) => Ok(outcome),
                Err(Error::Refused(failure)) => Err(failure),
                Err(Error::Corrupt(v)) => return Err(v),
            }
        } else {
            Err(Failure::Forbidden)
        };
        if let Some(key) = &request.idempotency {
            let record = IdempotencyRecord {
                key: key.key.clone(),
                fingerprint: key.fingerprint,
                created_at: now,
                reply: reply.clone(),
            };
            self.window.insert(record.clone());
            self.pending.idempotency.push(record);
        }
        Ok(reply)
    }

    fn execute(
        &mut self,
        actor: &Arc<str>,
        command: &Command,
        now: Timestamp,
    ) -> Result<Outcome, Error> {
        match command {
            Command::CreateHold {
                token,
                ttl_secs,
                max_objects,
            } => self.create_hold(actor, *token, *ttl_secs, *max_objects, now),
            Command::Hold { token, objects } => self.hold_objects(actor, token, objects, now),
            Command::Release { token, objects } => self.release(actor, token, objects, now),
            Command::ExtendHold { token, ttl_secs } => self.extend(actor, token, *ttl_secs, now),
            Command::Book {
                token,
                objects,
                order_ref,
                details,
            } => self.book(actor, token.as_ref(), objects, order_ref, details, now),
            Command::ReleaseBooked { objects } => self.release_booked(actor, objects),
            Command::ChangeStatus { objects, status } => {
                self.change_status(actor, objects, *status)
            }
        }
    }

    /// Seal everything applied since the last call into one batch, or `None`
    /// if nothing needs writing.
    pub fn take_batch(&mut self) -> Result<Option<Batch>, InvariantViolation> {
        self.seal(None)
    }

    /// [`Self::take_batch`], also writing into `sealed` what the batch changes
    /// as realtime subscribers see it: each changed object's final status and
    /// each touched token's hold, as of the batch's last command. Memory is
    /// exactly that state now, since nothing after the batch is applied yet.
    pub(crate) fn take_batch_into(
        &mut self,
        sealed: &mut Sealed,
    ) -> Result<Option<Batch>, InvariantViolation> {
        sealed.clear();
        self.seal(Some(sealed))
    }

    fn seal(
        &mut self,
        mut sealed: Option<&mut Sealed>,
    ) -> Result<Option<Batch>, InvariantViolation> {
        let pending = std::mem::take(&mut self.pending);
        if pending.log.is_empty() && pending.idempotency.is_empty() {
            return Ok(None);
        }

        let mut objects = pending.objects;
        // By object, then in command order (a stable sort): an object's first
        // change holds its status before the batch, its last the `seq` that
        // set the final one.
        objects.sort_by_key(|&(idx, seq, _)| (idx, seq));
        let mut objects_set = Vec::new();
        let mut objects_freed = Vec::new();
        let mut counts = [0i32; 16];
        let mut before = 0;
        for (n, &(idx, obj_seq, old)) in objects.iter().enumerate() {
            if n == 0 || objects.get(n - 1).is_none_or(|prev| prev.0 != idx) {
                before = old;
            }
            if objects.get(n + 1).is_some_and(|next| next.0 == idx) {
                continue; // keep the last change per object
            }
            let oid = self.chart.oid(idx).ok_or(corrupt("idx outside chart"))?;
            let status = self.status(idx).ok_or(corrupt("bad status"))?;
            if before != status.code() {
                if let Some(was) = counts.get_mut(usize::from(before)) {
                    *was -= 1;
                }
                if let Some(now) = counts.get_mut(usize::from(status.code())) {
                    *now += 1;
                }
            }
            if let Some(sealed) = sealed.as_deref_mut() {
                sealed.changes.push(Change {
                    idx,
                    code: status.code(),
                });
            }
            if status == Status::FREE {
                objects_freed.push(oid);
            } else {
                objects_set.push(ObjectRow {
                    oid,
                    status,
                    hold_id: self.holder(idx),
                    booking: self.booked.get(&idx).cloned(),
                    seq: obj_seq,
                });
            }
        }

        let mut created = pending.created;
        created.sort_unstable();
        let mut touched = pending.holds;
        touched.sort_unstable();
        touched.dedup_by_key(|(id, _)| *id);
        let (mut holds_created, mut holds_extended, mut holds_deleted) = (vec![], vec![], vec![]);
        for (id, slot) in touched {
            let live = self.slot(slot).filter(|h| h.id == id);
            match (created.binary_search(&id).is_ok(), live) {
                (true, Some(h)) => holds_created.push(HoldRow {
                    id,
                    token_hash: h.token,
                    expires_at: h.expires_at,
                    max_objects: h.max_objects,
                }),
                (false, Some(h)) => holds_extended.push((id, h.expires_at)),
                (false, None) => holds_deleted.push(id),
                (true, None) => {} // created and deleted in this batch
            }
        }

        if let Some(sealed) = sealed {
            sealed.seq = self.seq;
            let mut tokens = pending.tokens;
            tokens.sort_unstable_by_key(|t| t.0);
            tokens.dedup();
            for token in tokens {
                let hold = self.hold(&token).map(|h| {
                    let start = sealed.held.len();
                    sealed.held.extend_from_slice(h.objects);
                    sealed.held[start..].sort_unstable();
                    (h.expires_at, start..sealed.held.len())
                });
                sealed.holds.push((token, hold));
            }
        }

        Ok(Some(Batch {
            event: self.event,
            epoch: self.epoch,
            seq: self.seq,
            holds_created,
            holds_extended,
            holds_deleted,
            objects_set,
            objects_freed,
            counts,
            log: pending.log,
            idempotency: pending.idempotency,
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

    fn slot(&self, slot: u32) -> Option<&Hold> {
        self.holds.get(slot as usize).and_then(Option::as_ref)
    }

    /// The slot of the token's hold, if it exists and has not expired.
    fn live_hold(&self, token: &TokenHash, now: Timestamp) -> Result<u32, Error> {
        let slot = *self
            .by_token
            .get(token)
            .ok_or(Failure::NotFound(Missing::HoldToken))?;
        let hold = self.slot(slot).ok_or(corrupt("token maps to no hold"))?;
        // Unreachable after `expire_due(now)`; kept for a caller that skips it.
        if now >= hold.expires_at {
            return Err(Failure::HoldTokenExpired.into());
        }
        Ok(slot)
    }

    fn hold_mut(&mut self, slot: u32) -> Result<&mut Hold, InvariantViolation> {
        self.holds
            .get_mut(slot as usize)
            .and_then(Option::as_mut)
            .ok_or(corrupt("bad hold slot"))
    }

    /// Resolve references in order, listing every unknown one together.
    fn resolve_each<'a>(
        &self,
        objects: impl Iterator<Item = &'a ObjectRef>,
    ) -> Result<Vec<Idx>, Failure> {
        let mut missing = Vec::new();
        let mut idxs = Vec::new();
        for object in objects {
            match self.chart.resolve(object) {
                Some(idx) => idxs.push(idx),
                None => missing.push(object.clone()),
            }
        }
        if !missing.is_empty() {
            return Err(Failure::NotFound(Missing::Objects(missing)));
        }
        Ok(idxs)
    }

    /// Resolve references to unique `idx`, ascending.
    fn resolve(&self, objects: &[ObjectRef]) -> Result<Vec<Idx>, Failure> {
        let mut idxs = self.resolve_each(objects.iter())?;
        idxs.sort_unstable();
        idxs.dedup();
        Ok(idxs)
    }

    /// All-or-nothing validation (02 §4.3): classify every object by its status
    /// and holder slot (+1, or 0). Returns the objects to change, or a refusal
    /// listing every failing object.
    fn check(&self, idxs: &[Idx], rule: impl Fn(Status, u32) -> Check) -> Result<Vec<Idx>, Error> {
        let mut failed = Vec::new();
        let mut change = Vec::new();
        for &idx in idxs {
            let i = idx as usize;
            let status = self.status(idx).ok_or(corrupt("idx outside state"))?;
            let holder = *self.holder.get(i).ok_or(corrupt("idx outside state"))?;
            if (holder != 0) != (status == Status::HELD) {
                return Err(corrupt("holder does not match status").into());
            }
            match rule(status, holder) {
                Check::Skip => {}
                Check::Change => change.push(idx),
                Check::Fail(reason) => failed.push(ObjectFailure { idx, reason }),
            }
        }
        match failed.first() {
            Some(first) => Err(first.reason.failure(failed).into()),
            None => Ok(change),
        }
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
        let slot = match self.free_slots.pop() {
            Some(slot) => slot,
            None => {
                self.holds.push(None);
                u32::try_from(self.holds.len() - 1).map_err(|_| corrupt("too many holds"))?
            }
        };
        let seq = self.next_seq();
        let id = HoldId(seq);
        let expires_at = now.plus_secs(ttl_secs);
        *self
            .holds
            .get_mut(slot as usize)
            .ok_or(corrupt("bad hold slot"))? = Some(Hold {
            id,
            token,
            expires_at,
            max_objects,
            objects: Vec::new(),
        });
        self.by_token.insert(token, slot);
        self.expiry.push(Reverse((expires_at, id, slot)));
        self.pending.holds.push((id, slot));
        self.pending.created.push(id);
        self.pending.tokens.push(token);
        self.log(LogRecord {
            expires_at: Some(expires_at),
            max_objects,
            ..record(seq, Op::CreateHold, actor, Some(id), Vec::new())
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
        let new = self.check(&idxs, |status, holder| {
            if holder == slot + 1 {
                Check::Skip
            } else if status == Status::FREE {
                Check::Change
            } else {
                Check::Fail(ObjectReason::NotFree)
            }
        })?;
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
            self.set_object(idx, Status::HELD, slot + 1, None, seq)?;
        }
        self.pending.tokens.push(*token);
        self.log(record(seq, Op::Hold, actor, Some(id), oids));
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
            self.objects_of(slot)?
        } else {
            let idxs = self.resolve(objects)?;
            self.check(&idxs, |_, holder| held_by(holder, slot))?
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
            self.set_object(idx, Status::FREE, 0, None, seq)?;
        }
        let hold = self.hold_mut(slot)?;
        hold.objects.retain(|i| idxs.binary_search(i).is_err());
        let id = hold.id;
        self.pending.tokens.push(*token);
        self.log(record(seq, Op::Release, actor, Some(id), oids));
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
        // The old entry is now stale and is skipped when it surfaces.
        self.expiry.push(Reverse((expires_at, id, slot)));
        self.pending.holds.push((id, slot));
        self.pending.tokens.push(*token);
        self.log(LogRecord {
            expires_at: Some(expires_at),
            ..record(seq, Op::ExtendHold, actor, Some(id), Vec::new())
        });
        Ok(Outcome::Extended { seq, expires_at })
    }

    fn book(
        &mut self,
        actor: &Arc<str>,
        token: Option<&TokenHash>,
        objects: &[ObjectRef],
        order_ref: &Arc<str>,
        details: &[BookDetail],
        now: Timestamp,
    ) -> Result<Outcome, Error> {
        if objects.len() > limits::MAX_OBJECTS_PER_COMMAND
            || details.len() > limits::MAX_OBJECTS_PER_COMMAND
        {
            return Err(Failure::InvalidRequest(InvalidReason::TooManyObjects).into());
        }
        if token.is_none() && objects.is_empty() {
            return Err(Failure::InvalidRequest(InvalidReason::NoObjects).into());
        }
        check_text(
            order_ref,
            limits::MAX_ORDER_REF_CHARS,
            InvalidReason::OrderRefLength,
        )?;
        for d in details {
            if let Some(t) = &d.ticket_type {
                check_text(
                    t,
                    limits::MAX_TICKET_TYPE_CHARS,
                    InvalidReason::TicketTypeLength,
                )?;
            }
            if d.extra
                .as_ref()
                .is_some_and(|e| e.len() > limits::MAX_EXTRA_BYTES)
            {
                return Err(Failure::InvalidRequest(InvalidReason::ExtraTooLarge).into());
            }
        }
        let slot = token.map(|t| self.live_hold(t, now)).transpose()?;
        let refs = objects.iter().chain(details.iter().map(|d| &d.object));
        let mut named = self.resolve_each(refs)?;
        let detailed = named.split_off(objects.len());
        named.sort_unstable();
        named.dedup();
        let idxs = match slot {
            Some(slot) if named.is_empty() => self.objects_of(slot)?,
            _ => named,
        };
        if idxs.is_empty() {
            return Err(Failure::InvalidRequest(InvalidReason::NoObjects).into());
        }
        let mut detail_of = HashMap::with_capacity(detailed.len());
        for (&idx, d) in detailed.iter().zip(details) {
            if idxs.binary_search(&idx).is_err() || detail_of.insert(idx, d).is_some() {
                return Err(Failure::InvalidRequest(InvalidReason::DetailMismatch).into());
            }
        }
        match slot {
            Some(slot) => self.check(&idxs, |_, holder| held_by(holder, slot))?,
            None => self.check(&idxs, |status, _| {
                if status == Status::FREE {
                    Check::Change
                } else {
                    Check::Fail(ObjectReason::NotFree)
                }
            })?,
        };
        let oids = self.oids(&idxs)?;
        let seq = self.next_seq();
        let mut logged = Vec::with_capacity(detail_of.len());
        for (&idx, &oid) in idxs.iter().zip(&oids) {
            let detail = detail_of.get(&idx);
            if let Some(d) = detail {
                logged.push((oid, d.ticket_type.clone(), d.extra.clone()));
            }
            let booking = Booking {
                order_ref: order_ref.clone(),
                ticket_type: detail.and_then(|d| d.ticket_type.clone()),
                extra: detail.and_then(|d| d.extra.clone()),
            };
            self.set_object(idx, Status::BOOKED, 0, Some(booking), seq)?;
        }
        let mut hold_id = None;
        if let Some(slot) = slot {
            self.pending.tokens.extend(token);
            let hold = self.hold_mut(slot)?;
            hold.objects.retain(|i| idxs.binary_search(i).is_err());
            hold_id = Some(hold.id);
            if hold.objects.is_empty() {
                self.delete_hold(slot)?;
            }
        }
        self.log(LogRecord {
            order_ref: Some(order_ref.clone()),
            details: logged,
            ..record(seq, Op::Book, actor, hold_id, oids)
        });
        Ok(Outcome::Booked { seq, objects: idxs })
    }

    fn release_booked(
        &mut self,
        actor: &Arc<str>,
        objects: &[ObjectRef],
    ) -> Result<Outcome, Error> {
        check_count(objects)?;
        let idxs = self.resolve(objects)?;
        self.check(&idxs, |status, _| {
            if status == Status::BOOKED {
                Check::Change
            } else {
                Check::Fail(ObjectReason::NotBooked)
            }
        })?;
        let oids = self.oids(&idxs)?;
        let seq = self.next_seq();
        for &idx in &idxs {
            self.set_object(idx, Status::FREE, 0, None, seq)?;
        }
        self.log(record(seq, Op::Release, actor, None, oids));
        Ok(Outcome::Released { seq, objects: idxs })
    }

    /// Free → blocked or custom, or blocked or custom → free. Any other change
    /// (blocked → custom directly, or anything involving held or booked) is
    /// refused per object.
    fn change_status(
        &mut self,
        actor: &Arc<str>,
        objects: &[ObjectRef],
        target: Status,
    ) -> Result<Outcome, Error> {
        check_count(objects)?;
        if target == Status::HELD || target == Status::BOOKED {
            return Err(Failure::InvalidRequest(InvalidReason::BadStatus).into());
        }
        let idxs = self.resolve(objects)?;
        let change = self.check(&idxs, |status, _| {
            if status == target {
                Check::Skip
            } else if target == Status::FREE {
                if status == Status::HELD || status == Status::BOOKED {
                    Check::Fail(ObjectReason::NotBlocked)
                } else {
                    Check::Change
                }
            } else if status == Status::FREE {
                Check::Change
            } else {
                Check::Fail(ObjectReason::NotFree)
            }
        })?;
        if change.is_empty() {
            return Ok(Outcome::StatusChanged {
                seq: self.seq,
                objects: idxs,
            });
        }
        let oids = self.oids(&change)?;
        let seq = self.next_seq();
        for &idx in &change {
            self.set_object(idx, target, 0, None, seq)?;
        }
        self.log(LogRecord {
            status: Some(target),
            ..record(seq, Op::ChangeStatus, actor, None, oids)
        });
        Ok(Outcome::StatusChanged { seq, objects: idxs })
    }

    /// The internal `Expire` command (08 §7.3) for the hold in `slot`.
    fn expire(&mut self, slot: u32) -> Result<(), InvariantViolation> {
        let hold = self.hold_mut(slot)?;
        let (id, token) = (hold.id, hold.token);
        let mut idxs = std::mem::take(&mut hold.objects);
        self.pending.tokens.push(token);
        idxs.sort_unstable();
        let oids = self.oids(&idxs)?;
        let seq = self.next_seq();
        for &idx in &idxs {
            self.set_object(idx, Status::FREE, 0, None, seq)?;
        }
        self.delete_hold(slot)?;
        // The `hold.expired` webhook outbox row (statement 7) belongs here.
        self.log(record(
            seq,
            Op::Expire,
            &Arc::from("system"),
            Some(id),
            oids,
        ));
        Ok(())
    }

    /// The hold's objects, ascending.
    fn objects_of(&mut self, slot: u32) -> Result<Vec<Idx>, InvariantViolation> {
        let mut all = self.hold_mut(slot)?.objects.clone();
        all.sort_unstable();
        Ok(all)
    }

    /// Remove a hold whose objects have all left it.
    fn delete_hold(&mut self, slot: u32) -> Result<(), InvariantViolation> {
        let hold = self
            .holds
            .get_mut(slot as usize)
            .and_then(Option::take)
            .ok_or(corrupt("bad hold slot"))?;
        if !hold.objects.is_empty() {
            return Err(corrupt("deleted a hold that still has objects"));
        }
        self.by_token.remove(&hold.token);
        self.free_slots.push(slot);
        self.pending.holds.push((hold.id, slot));
        Ok(())
    }

    fn set_object(
        &mut self,
        idx: Idx,
        status: Status,
        holder: u32,
        booking: Option<Booking>,
        seq: u64,
    ) -> Result<(), InvariantViolation> {
        let i = idx as usize;
        let (Some(s), Some(h)) = (self.status.get_mut(i), self.holder.get_mut(i)) else {
            return Err(corrupt("idx outside state"));
        };
        let was = *s;
        let was_booked = was == Status::BOOKED.code();
        *s = status.code();
        *h = holder;
        match booking {
            Some(b) => {
                self.booked.insert(idx, b);
            }
            None if was_booked => {
                self.booked.remove(&idx);
            }
            None => {}
        }
        self.pending.objects.push((idx, seq, was));
        Ok(())
    }
}

fn record(
    seq: u64,
    op: Op,
    actor: &Arc<str>,
    hold_id: Option<HoldId>,
    oids: Vec<Oid>,
) -> LogRecord {
    LogRecord {
        seq,
        op,
        actor: actor.clone(),
        hold_id,
        oids,
        expires_at: None,
        max_objects: None,
        order_ref: None,
        status: None,
        details: Vec::new(),
    }
}

fn held_by(holder: u32, slot: u32) -> Check {
    if holder == slot + 1 {
        Check::Change
    } else {
        Check::Fail(ObjectReason::NotHeldByToken)
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

/// 1 to `max_chars` characters, none of them U+0000: the store writes text
/// to PostgreSQL `text` columns, which cannot hold it, and a rejected value
/// would fail the whole batch it is committed in.
fn check_text(text: &str, max_chars: usize, reason: InvalidReason) -> Result<(), Failure> {
    let chars = text.chars().count();
    if chars == 0 || chars > max_chars || text.contains('\0') {
        Err(Failure::InvalidRequest(reason))
    } else {
        Ok(())
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
