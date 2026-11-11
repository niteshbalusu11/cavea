//! The reference model (13 §3.1): the seat state machine of 02 §4 to §6 and
//! 08 §6.1, §7, written for obviousness from the specs, one command at a time,
//! with maps instead of arrays and no batching. It shares nothing with
//! `EventState` beyond the public request, reply and log types.
//!
//! Where the specs leave the order of checks open, the model fixes it as 08
//! §7 now states it: the idempotency key, the authority, the request's shape,
//! the hold token, the object references, then the per-object conditions and
//! last the token's object limit.
//!
//! The log replay ([`Replay`]) lives in `cavea-engine`, where the reconciler
//! uses it too; here are the comparisons only tests make with it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use cavea_engine::{
    Authority, BookDetail, Booking, Command, EventState, Failure, HoldId, Idx, InvalidReason,
    LoadedEvent, LogRecord, Missing, ObjectFailure, ObjectReason, ObjectRef, Oid, Op, Outcome,
    Replay, ReplayError, ReplayHold, Reply, Request, Status, Timestamp, TokenHash, limits,
};

use crate::{Violation, ensure, idx_of, oid};

/// An object that is not free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seat {
    Held(HoldId),
    Booked(Booking),
    /// Blocked or a custom status.
    Other(Status),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelHold {
    pub id: HoldId,
    pub expires_at: Timestamp,
    pub max_objects: Option<u32>,
    pub objects: BTreeSet<Idx>,
}

#[derive(Debug, Clone)]
struct Record {
    fingerprint: [u8; 32],
    created_at: Timestamp,
    reply: Reply,
    /// Insertion order: the window drops the oldest first.
    order: u64,
}

#[derive(Debug, Clone)]
pub struct Model {
    objects: u32,
    seq: u64,
    /// Objects not in the default state.
    seats: HashMap<Idx, Seat>,
    holds: HashMap<TokenHash, ModelHold>,
    records: HashMap<Arc<str>, Record>,
    inserted: u64,
    /// Log records of commands applied since [`Self::take_log`].
    log: Vec<LogRecord>,
}

type Step<T> = Result<T, Failure>;

fn invalid<T>(reason: InvalidReason) -> Step<T> {
    Err(Failure::InvalidRequest(reason))
}

/// 1 to `max` characters, none of them U+0000.
fn text_ok(text: &str, max: usize) -> bool {
    let n = text.chars().count();
    n >= 1 && n <= max && !text.contains('\0')
}

fn age_reached(created: Timestamp, now: Timestamp, ms: u64) -> bool {
    now.0 >= created.0.saturating_add(ms)
}

impl Model {
    #[must_use]
    pub fn new(objects: u32) -> Self {
        Self {
            objects,
            seq: 0,
            seats: HashMap::new(),
            holds: HashMap::new(),
            records: HashMap::new(),
            inserted: 0,
            log: Vec::new(),
        }
    }

    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq
    }

    pub fn holds(&self) -> impl Iterator<Item = (&TokenHash, &ModelHold)> {
        self.holds.iter()
    }

    #[must_use]
    pub fn next_expiry(&self) -> Option<Timestamp> {
        self.holds.values().map(|h| h.expires_at).min()
    }

    /// The log records of everything applied since the last call.
    pub fn take_log(&mut self) -> Vec<LogRecord> {
        std::mem::take(&mut self.log)
    }

    /// Expire every hold due at `now` (02 §5: from its expiry instant on),
    /// earliest first, ties by hold id; each is a command with a `seq`.
    pub fn expire_due(&mut self, now: Timestamp) {
        let mut due: Vec<(Timestamp, HoldId)> = self
            .holds
            .values()
            .filter(|h| now >= h.expires_at)
            .map(|h| (h.expires_at, h.id))
            .collect();
        due.sort();
        for (_, id) in due {
            self.expire(id);
        }
    }

    /// The internal `Expire` command for hold `id` (08 §7.3). False if there
    /// is no such hold.
    pub fn expire(&mut self, id: HoldId) -> bool {
        let Some(token) = self.holds.iter().find(|(_, h)| h.id == id).map(|(t, _)| *t) else {
            return false;
        };
        let Some(hold) = self.holds.remove(&token) else {
            return false;
        };
        for idx in &hold.objects {
            self.seats.remove(idx);
        }
        self.seq += 1;
        let oids = hold.objects.iter().map(|&i| oid(i)).collect();
        self.push_log(Op::Expire, "system".into(), Some(id), oids);
        true
    }

    /// One request at `now`: expiries due first, then the idempotency window,
    /// then the command (08 §4.3, §6.1).
    pub fn apply(&mut self, request: &Request, now: Timestamp) -> Reply {
        self.expire_due(now);
        let Some(key) = &request.idempotency else {
            return self.command(request, now);
        };
        if !text_ok(&key.key, limits::MAX_IDEMPOTENCY_KEY_CHARS) {
            return invalid(InvalidReason::IdempotencyKeyLength);
        }
        if let Some(r) = self.records.get(&key.key)
            && !age_reached(r.created_at, now, limits::IDEMPOTENCY_TTL_MS)
        {
            return if r.fingerprint == key.fingerprint {
                r.reply.clone()
            } else {
                Err(Failure::IdempotencyMismatch)
            };
        }
        if !self.make_room(now, key.checked_at.min(now)) {
            return Err(Failure::Overloaded);
        }
        let reply = self.command(request, now);
        self.inserted += 1;
        let record = Record {
            fingerprint: key.fingerprint,
            created_at: now,
            reply: reply.clone(),
            order: self.inserted,
        };
        self.records.insert(key.key.clone(), record);
        reply
    }

    /// While the window is full, forget records 24 hours old, then drop the
    /// oldest if it was a minute old at the caller's lookup. False if still
    /// full.
    fn make_room(&mut self, now: Timestamp, checked_at: Timestamp) -> bool {
        if self.records.len() >= limits::IDEMPOTENCY_WINDOW {
            self.records
                .retain(|_, r| !age_reached(r.created_at, now, limits::IDEMPOTENCY_TTL_MS));
        }
        while self.records.len() >= limits::IDEMPOTENCY_WINDOW {
            let oldest = self.records.iter().min_by_key(|(_, r)| r.order);
            let Some((key, r)) = oldest else { break };
            if !age_reached(
                r.created_at,
                checked_at,
                limits::IDEMPOTENCY_MIN_RETENTION_MS,
            ) {
                return false;
            }
            let key = key.clone();
            self.records.remove(&key);
        }
        true
    }

    /// Whether some idempotency record stops matching in `(from, to]`: a
    /// clock jumping back across that instant makes a record the engine has
    /// already dropped from memory match again once the event reloads from
    /// the store (08 §6.1), which no model of memory alone can follow.
    #[must_use]
    pub fn record_expires_within(&self, from: Timestamp, to: Timestamp) -> bool {
        self.records.values().any(|r| {
            let end = r.created_at.0.saturating_add(limits::IDEMPOTENCY_TTL_MS);
            from.0 < end && end <= to.0
        })
    }

    /// The command alone, without expiries or the idempotency window: for
    /// replaying committed commands in log order, whose expiries the log
    /// itself carries.
    pub fn execute(&mut self, request: &Request, now: Timestamp) -> Reply {
        self.command(request, now)
    }

    /// The hold's expiry, if it exists.
    #[must_use]
    pub fn expiry_of(&self, id: HoldId) -> Option<Timestamp> {
        self.holds
            .values()
            .find(|h| h.id == id)
            .map(|h| h.expires_at)
    }

    /// The authority check, then the command itself.
    fn command(&mut self, request: &Request, now: Timestamp) -> Reply {
        let secret = request.authority == Authority::Secret;
        let actor = request.actor.clone();
        match &request.command {
            Command::CreateHold {
                token,
                ttl_secs,
                max_objects,
            } => self.create_hold(actor, *token, *ttl_secs, *max_objects, now),
            Command::Hold { token, objects } => self.hold(actor, token, objects),
            Command::Release { token, objects } => self.release(actor, token, objects),
            _ if !secret => Err(Failure::Forbidden),
            Command::ExtendHold { token, ttl_secs } => self.extend(actor, token, *ttl_secs, now),
            Command::Book {
                token,
                objects,
                order_ref,
                details,
            } => self.book(actor, token.as_ref(), objects, order_ref, details),
            Command::ReleaseBooked { objects } => self.release_booked(actor, objects),
            Command::ChangeStatus { objects, status } => {
                self.change_status(actor, objects, *status)
            }
        }
    }

    fn resolve_one(&self, r: &ObjectRef) -> Option<Idx> {
        match r {
            ObjectRef::Idx(i) => (*i < self.objects).then_some(*i),
            ObjectRef::Label(l) => {
                let i: Idx = l.strip_prefix("S-")?.parse().ok()?;
                // `S-01` is not the label of object 1.
                (i < self.objects && format!("S-{i}") == **l).then_some(i)
            }
        }
    }

    /// Every reference resolved, or `not_found` listing each one that is not,
    /// in request order.
    fn resolve<'a>(&self, refs: impl Iterator<Item = &'a ObjectRef>) -> Step<Vec<Idx>> {
        let mut found = Vec::new();
        let mut missing = Vec::new();
        for r in refs {
            match self.resolve_one(r) {
                Some(i) => found.push(i),
                None => missing.push(r.clone()),
            }
        }
        if missing.is_empty() {
            Ok(found)
        } else {
            Err(Failure::NotFound(Missing::Objects(missing)))
        }
    }

    fn live(&self, token: &TokenHash) -> Step<&ModelHold> {
        self.holds
            .get(token)
            .ok_or(Failure::NotFound(Missing::HoldToken))
    }

    fn count(objects: &[ObjectRef]) -> Step<()> {
        if objects.is_empty() {
            invalid(InvalidReason::NoObjects)
        } else if objects.len() > limits::MAX_OBJECTS_PER_COMMAND {
            invalid(InvalidReason::TooManyObjects)
        } else {
            Ok(())
        }
    }

    fn ttl(ttl_secs: u32) -> Step<()> {
        if (limits::MIN_HOLD_TTL_SECS..=limits::MAX_HOLD_TTL_SECS).contains(&ttl_secs) {
            Ok(())
        } else {
            invalid(InvalidReason::TtlOutOfRange)
        }
    }

    /// All-or-nothing: the objects for which `fails` gives a reason, ascending,
    /// as one refusal.
    fn all_or_nothing(
        &self,
        idxs: &BTreeSet<Idx>,
        fails: impl Fn(Option<&Seat>) -> Option<ObjectReason>,
    ) -> Step<()> {
        let failed: Vec<ObjectFailure> = idxs
            .iter()
            .filter_map(|&idx| {
                fails(self.seats.get(&idx)).map(|reason| ObjectFailure { idx, reason })
            })
            .collect();
        match failed.first() {
            None => Ok(()),
            Some(f) => Err(match f.reason {
                ObjectReason::NotFree => Failure::ObjectNotFree(failed),
                ObjectReason::NotHeldByToken => Failure::ObjectNotHeldByToken(failed),
                ObjectReason::NotBooked | ObjectReason::NotBlocked => {
                    Failure::ObjectStatusConflict(failed)
                }
            }),
        }
    }

    fn push_log(&mut self, op: Op, actor: Arc<str>, hold_id: Option<HoldId>, oids: Vec<Oid>) {
        self.log.push(LogRecord {
            seq: self.seq,
            op,
            actor,
            hold_id,
            oids,
            expires_at: None,
            max_objects: None,
            order_ref: None,
            status: None,
            details: Vec::new(),
        });
    }

    fn last_log(&mut self) -> Option<&mut LogRecord> {
        self.log.last_mut()
    }

    fn create_hold(
        &mut self,
        actor: Arc<str>,
        token: TokenHash,
        ttl_secs: u32,
        max_objects: Option<u32>,
        now: Timestamp,
    ) -> Reply {
        Self::ttl(ttl_secs)?;
        if max_objects == Some(0) {
            return invalid(InvalidReason::ZeroObjectLimit);
        }
        if self.holds.contains_key(&token) {
            return invalid(InvalidReason::TokenAlreadyExists);
        }
        self.seq += 1;
        let id = HoldId(self.seq);
        let expires_at = now.plus_secs(ttl_secs);
        let hold = ModelHold {
            id,
            expires_at,
            max_objects,
            objects: BTreeSet::new(),
        };
        self.holds.insert(token, hold);
        self.push_log(Op::CreateHold, actor, Some(id), Vec::new());
        if let Some(r) = self.last_log() {
            r.expires_at = Some(expires_at);
            r.max_objects = max_objects;
        }
        Ok(Outcome::HoldCreated {
            seq: self.seq,
            hold: id,
            expires_at,
        })
    }

    fn hold(&mut self, actor: Arc<str>, token: &TokenHash, objects: &[ObjectRef]) -> Reply {
        Self::count(objects)?;
        let hold = self.live(token)?.clone();
        let named: BTreeSet<Idx> = self.resolve(objects.iter())?.into_iter().collect();
        self.all_or_nothing(&named, |seat| match seat {
            None => None,
            Some(Seat::Held(id)) if *id == hold.id => None,
            Some(_) => Some(ObjectReason::NotFree),
        })?;
        let new: Vec<Idx> = named.difference(&hold.objects).copied().collect();
        if let Some(limit) = hold.max_objects {
            let requested = (hold.objects.len() + new.len()) as u64;
            if requested > u64::from(limit) {
                return Err(Failure::HoldLimitExceeded { limit, requested });
            }
        }
        let objects = named.into_iter().collect();
        if new.is_empty() {
            return Ok(Outcome::Held {
                seq: self.seq,
                expires_at: hold.expires_at,
                objects,
            });
        }
        self.seq += 1;
        for &idx in &new {
            self.seats.insert(idx, Seat::Held(hold.id));
        }
        if let Some(h) = self.holds.get_mut(token) {
            h.objects.extend(&new);
        }
        self.push_log(
            Op::Hold,
            actor,
            Some(hold.id),
            new.iter().map(|&i| oid(i)).collect(),
        );
        Ok(Outcome::Held {
            seq: self.seq,
            expires_at: hold.expires_at,
            objects,
        })
    }

    fn release(&mut self, actor: Arc<str>, token: &TokenHash, objects: &[ObjectRef]) -> Reply {
        if objects.len() > limits::MAX_OBJECTS_PER_COMMAND {
            return invalid(InvalidReason::TooManyObjects);
        }
        let hold = self.live(token)?.clone();
        let release: BTreeSet<Idx> = if objects.is_empty() {
            hold.objects.clone()
        } else {
            let named = self.resolve(objects.iter())?.into_iter().collect();
            self.all_or_nothing(&named, |seat| match seat {
                Some(Seat::Held(id)) if *id == hold.id => None,
                _ => Some(ObjectReason::NotHeldByToken),
            })?;
            named
        };
        if release.is_empty() {
            return Ok(Outcome::Released {
                seq: self.seq,
                objects: Vec::new(),
            });
        }
        self.seq += 1;
        for idx in &release {
            self.seats.remove(idx);
        }
        if let Some(h) = self.holds.get_mut(token) {
            h.objects.retain(|i| !release.contains(i));
        }
        let oids = release.iter().map(|&i| oid(i)).collect();
        self.push_log(Op::Release, actor, Some(hold.id), oids);
        Ok(Outcome::Released {
            seq: self.seq,
            objects: release.into_iter().collect(),
        })
    }

    fn extend(
        &mut self,
        actor: Arc<str>,
        token: &TokenHash,
        ttl_secs: u32,
        now: Timestamp,
    ) -> Reply {
        Self::ttl(ttl_secs)?;
        let id = self.live(token)?.id;
        let expires_at = now.plus_secs(ttl_secs);
        self.seq += 1;
        if let Some(h) = self.holds.get_mut(token) {
            h.expires_at = expires_at;
        }
        self.push_log(Op::ExtendHold, actor, Some(id), Vec::new());
        if let Some(r) = self.last_log() {
            r.expires_at = Some(expires_at);
        }
        Ok(Outcome::Extended {
            seq: self.seq,
            expires_at,
        })
    }

    fn book(
        &mut self,
        actor: Arc<str>,
        token: Option<&TokenHash>,
        objects: &[ObjectRef],
        order_ref: &Arc<str>,
        details: &[BookDetail],
    ) -> Reply {
        let max = limits::MAX_OBJECTS_PER_COMMAND;
        if objects.len() > max || details.len() > max {
            return invalid(InvalidReason::TooManyObjects);
        }
        if token.is_none() && objects.is_empty() {
            return invalid(InvalidReason::NoObjects);
        }
        if !text_ok(order_ref, limits::MAX_ORDER_REF_CHARS) {
            return invalid(InvalidReason::OrderRefLength);
        }
        for d in details {
            if d.ticket_type
                .as_ref()
                .is_some_and(|t| !text_ok(t, limits::MAX_TICKET_TYPE_CHARS))
            {
                return invalid(InvalidReason::TicketTypeLength);
            }
            if d.extra
                .as_ref()
                .is_some_and(|e| e.len() > limits::MAX_EXTRA_BYTES)
            {
                return invalid(InvalidReason::ExtraTooLarge);
            }
        }
        let hold = match token {
            Some(t) => Some(self.live(t)?.clone()),
            None => None,
        };
        let refs = objects.iter().chain(details.iter().map(|d| &d.object));
        let mut resolved = self.resolve(refs)?;
        let detailed = resolved.split_off(objects.len());
        let named: BTreeSet<Idx> = resolved.into_iter().collect();
        let booking: BTreeSet<Idx> = match &hold {
            Some(h) if named.is_empty() => h.objects.clone(),
            _ => named,
        };
        if booking.is_empty() {
            return invalid(InvalidReason::NoObjects);
        }
        let mut detail_of: BTreeMap<Idx, &BookDetail> = BTreeMap::new();
        for (idx, d) in detailed.iter().zip(details) {
            if !booking.contains(idx) || detail_of.insert(*idx, d).is_some() {
                return invalid(InvalidReason::DetailMismatch);
            }
        }
        match &hold {
            Some(h) => self.all_or_nothing(&booking, |seat| match seat {
                Some(Seat::Held(id)) if *id == h.id => None,
                _ => Some(ObjectReason::NotHeldByToken),
            })?,
            None => self.all_or_nothing(&booking, |seat| seat.map(|_| ObjectReason::NotFree))?,
        }
        self.seq += 1;
        let mut logged = Vec::new();
        for &idx in &booking {
            let d = detail_of.get(&idx);
            if let Some(d) = d {
                logged.push((oid(idx), d.ticket_type.clone(), d.extra.clone()));
            }
            let b = Booking {
                order_ref: order_ref.clone(),
                ticket_type: d.and_then(|d| d.ticket_type.clone()),
                extra: d.and_then(|d| d.extra.clone()),
            };
            self.seats.insert(idx, Seat::Booked(b));
        }
        if let (Some(t), Some(_)) = (token, &hold)
            && let Some(h) = self.holds.get_mut(t)
        {
            h.objects.retain(|i| !booking.contains(i));
            if h.objects.is_empty() {
                self.holds.remove(t);
            }
        }
        let oids = booking.iter().map(|&i| oid(i)).collect();
        self.push_log(Op::Book, actor, hold.map(|h| h.id), oids);
        if let Some(r) = self.last_log() {
            r.order_ref = Some(order_ref.clone());
            r.details = logged;
        }
        Ok(Outcome::Booked {
            seq: self.seq,
            objects: booking.into_iter().collect(),
        })
    }

    fn release_booked(&mut self, actor: Arc<str>, objects: &[ObjectRef]) -> Reply {
        Self::count(objects)?;
        let named: BTreeSet<Idx> = self.resolve(objects.iter())?.into_iter().collect();
        self.all_or_nothing(&named, |seat| match seat {
            Some(Seat::Booked(_)) => None,
            _ => Some(ObjectReason::NotBooked),
        })?;
        self.seq += 1;
        for idx in &named {
            self.seats.remove(idx);
        }
        self.push_log(
            Op::Release,
            actor,
            None,
            named.iter().map(|&i| oid(i)).collect(),
        );
        Ok(Outcome::Released {
            seq: self.seq,
            objects: named.into_iter().collect(),
        })
    }

    fn change_status(&mut self, actor: Arc<str>, objects: &[ObjectRef], target: Status) -> Reply {
        Self::count(objects)?;
        if target == Status::HELD || target == Status::BOOKED {
            return invalid(InvalidReason::BadStatus);
        }
        let named: BTreeSet<Idx> = self.resolve(objects.iter())?.into_iter().collect();
        let status = |seat: Option<&Seat>| match seat {
            None => Status::FREE,
            Some(Seat::Held(_)) => Status::HELD,
            Some(Seat::Booked(_)) => Status::BOOKED,
            Some(Seat::Other(s)) => *s,
        };
        // 02 §4.3: free ↔ blocked or custom only; the same status is a no-op.
        self.all_or_nothing(&named, |seat| {
            let s = status(seat);
            if s == target {
                None
            } else if target == Status::FREE {
                matches!(seat, Some(Seat::Held(_) | Seat::Booked(_)))
                    .then_some(ObjectReason::NotBlocked)
            } else {
                (s != Status::FREE).then_some(ObjectReason::NotFree)
            }
        })?;
        let change: Vec<Idx> = named
            .iter()
            .copied()
            .filter(|i| status(self.seats.get(i)) != target)
            .collect();
        let objects = named.into_iter().collect();
        if change.is_empty() {
            return Ok(Outcome::StatusChanged {
                seq: self.seq,
                objects,
            });
        }
        self.seq += 1;
        for &idx in &change {
            if target == Status::FREE {
                self.seats.remove(&idx);
            } else {
                self.seats.insert(idx, Seat::Other(target));
            }
        }
        self.push_log(
            Op::ChangeStatus,
            actor,
            None,
            change.iter().map(|&i| oid(i)).collect(),
        );
        if let Some(r) = self.last_log() {
            r.status = Some(target);
        }
        Ok(Outcome::StatusChanged {
            seq: self.seq,
            objects,
        })
    }

    /// The model's state against the engine's: every object's status, holder
    /// and booking, every hold, `seq` and the next expiry.
    pub fn diff(&self, state: &mut EventState) -> Result<(), Violation> {
        ensure!(
            state.seq() == self.seq,
            "state",
            "seq {} != model {}",
            state.seq(),
            self.seq
        );
        ensure!(
            state.chart().len() == self.objects as usize,
            "state",
            "object count {}",
            state.chart().len()
        );
        for idx in 0..self.objects {
            let (status, holder, booking) = match self.seats.get(&idx) {
                None => (Status::FREE, None, None),
                Some(Seat::Held(id)) => (Status::HELD, Some(*id), None),
                Some(Seat::Booked(b)) => (Status::BOOKED, None, Some(b)),
                Some(Seat::Other(s)) => (*s, None, None),
            };
            let real = (state.status(idx), state.holder(idx), state.booking(idx));
            ensure!(
                real == (Some(status), holder, booking),
                "state",
                "object {idx}: engine {real:?}, model {:?}",
                (status, holder, booking)
            );
        }
        let real = state.holds().count();
        ensure!(
            real == self.holds.len(),
            "state",
            "{real} holds, model {}",
            self.holds.len()
        );
        for (token, h) in &self.holds {
            let Some(r) = state.hold(token) else {
                return Err(Violation::new("state", format!("hold {:?} missing", h.id)));
            };
            let objects: BTreeSet<Idx> = r.objects.iter().copied().collect();
            let real = (r.id, r.expires_at, r.max_objects, objects);
            let model = (h.id, h.expires_at, h.max_objects, h.objects.clone());
            ensure!(
                real == model,
                "state",
                "hold: engine {real:?}, model {model:?}"
            );
            ensure!(
                r.objects.len() == h.objects.len(),
                "state",
                "hold {:?} lists an object twice",
                h.id
            );
        }
        let next = state.next_expiry();
        ensure!(
            next == self.next_expiry(),
            "state",
            "next expiry {next:?} != {:?}",
            self.next_expiry()
        );
        Ok(())
    }
}

impl From<ReplayError> for Violation {
    fn from(e: ReplayError) -> Self {
        Violation::new(e.invariant, format!("seq {}: {}", e.seq, e.detail))
    }
}

/// The public code of every object of a replayed log, by `idx` (02 §4.2).
#[must_use]
pub fn public(replay: &Replay, objects: u32) -> Vec<u8> {
    (0..objects)
        .map(|i| u8::from(replay.objects.contains_key(&oid(i))))
        .collect()
}

/// INV-6 against memory: the same objects and holds (id, expiry, limit,
/// objects) as the replayed log.
pub fn diff_state(replay: &Replay, state: &EventState) -> Result<(), Violation> {
    ensure!(
        replay.seq == state.seq(),
        "INV-6",
        "log ends at {}, memory at {}",
        replay.seq,
        state.seq()
    );
    for idx in 0..state.chart().len() as Idx {
        let real = (state.status(idx), state.holder(idx), state.booking(idx));
        let log = replay.objects.get(&oid(idx));
        let log = log.map_or((Some(Status::FREE), None, None), |o| {
            (Some(o.0), o.1, o.2.as_ref())
        });
        ensure!(
            real == log,
            "INV-6",
            "object {idx}: memory {real:?}, log {log:?}"
        );
    }
    let real: BTreeMap<HoldId, ReplayHold> = state
        .holds()
        .map(|h| {
            let hold = ReplayHold {
                expires_at: h.expires_at,
                max_objects: h.max_objects,
                objects: h.objects.iter().map(|&i| oid(i)).collect(),
            };
            (h.id, hold)
        })
        .collect();
    ensure!(
        real == replay.holds,
        "INV-6",
        "holds: memory {real:?}, log {:?}",
        replay.holds
    );
    Ok(())
}

/// INV-6 against the store's rows, by the reconciler's own comparison
/// ([`Replay::compare`]), and every row naming an object of the chart.
pub fn diff_loaded(replay: &Replay, loaded: &LoadedEvent) -> Result<(), Violation> {
    let holds = loaded
        .holds
        .iter()
        .map(|h| (h.id, h.expires_at, h.max_objects));
    let differences = replay.compare(loaded.seq, holds, &loaded.objects);
    ensure!(
        differences.is_empty(),
        "INV-6",
        "store differs from the log: {differences:?}"
    );
    for o in &loaded.objects {
        ensure!(
            idx_of(o.oid).is_some(),
            "INV-6",
            "row for unknown oid {}",
            o.oid
        );
    }
    Ok(())
}
