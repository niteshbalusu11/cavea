//! An event's state rebuilt from its committed log alone (INV-6), and its
//! comparison with the stored rows: the reconciler's core (13 §3.3), shared
//! with the model-based tests and the simulation (13 §3.1, §3.2), which run it
//! on every batch they commit.
//!
//! It shares nothing with [`crate::EventState`] beyond the log and row types.
//! Each record must be a legal transition from the state before it (02 §4.3):
//! a booking of a booked object is INV-10, a `seq` out of turn INV-5, anything
//! else that could not have happened INV-6.
//!
//! The log carries everything `object_state` and `holds` hold except a hold's
//! token hash (08 §4.4): a hold is rebuilt by id, expiry, object limit and
//! objects.

use std::collections::{BTreeMap, BTreeSet};

use crate::store::{Booking, LogRecord, ObjectRow};
use crate::types::{HoldId, Oid, Op, Status, Timestamp};

/// An object not in the default state: status, holder, booking, and the `seq`
/// of the last command that changed it.
pub type Rebuilt = (Status, Option<HoldId>, Option<Booking>, u64);

/// A hold as the log describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayHold {
    pub expires_at: Timestamp,
    /// `None`: unlimited.
    pub max_objects: Option<u32>,
    pub objects: BTreeSet<Oid>,
}

/// A log record that is not a legal transition from the state before it.
/// The detail names objects and holds, never order references or `extra`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayError {
    pub seq: u64,
    /// `INV-5`, `INV-6` or `INV-10`.
    pub invariant: &'static str,
    pub detail: String,
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: seq {}: {}", self.invariant, self.seq, self.detail)
    }
}

impl std::error::Error for ReplayError {}

/// A field of an `object_state` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectField {
    Status,
    HoldId,
    OrderRef,
    TicketType,
    Extra,
    Seq,
}

/// A field of a `holds` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldField {
    ExpiresAt,
    MaxObjects,
}

/// One way the stored rows differ from the replayed log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Difference {
    /// `events.seq` is not the log's last `seq`.
    Seq {
        stored: u64,
        log: u64,
    },
    /// The log leaves the object free, or never mentions it, yet it has a row.
    ObjectUnexpected(Oid),
    /// The log leaves the object not free, but it has no row.
    ObjectMissing(Oid),
    ObjectRowTwice(Oid),
    Object(Oid, ObjectField),
    HoldUnexpected(HoldId),
    HoldMissing(HoldId),
    HoldRowTwice(HoldId),
    Hold(HoldId, HoldField),
}

/// State rebuilt from the committed log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Replay {
    /// The last `seq` applied.
    pub seq: u64,
    pub objects: BTreeMap<Oid, Rebuilt>,
    pub holds: BTreeMap<HoldId, ReplayHold>,
}

fn refuse(seq: u64, invariant: &'static str, detail: String) -> ReplayError {
    ReplayError {
        seq,
        invariant,
        detail,
    }
}

macro_rules! ensure {
    ($cond:expr, $seq:expr, $inv:expr, $($fmt:tt)+) => {
        if !$cond {
            return Err(refuse($seq, $inv, format!($($fmt)+)));
        }
    };
}

impl Replay {
    /// Apply the next record. After an `Err` the replay is partly applied and
    /// must not be used further.
    pub fn apply(&mut self, r: &LogRecord) -> Result<(), ReplayError> {
        let seq = r.seq;
        ensure!(
            seq == self.seq + 1,
            seq,
            "INV-5",
            "follows seq {}",
            self.seq
        );
        let hold = r.hold_id;
        let unique: BTreeSet<Oid> = r.oids.iter().copied().collect();
        ensure!(
            unique.len() == r.oids.len(),
            seq,
            "INV-6",
            "an object twice"
        );
        let changes_objects = !matches!(r.op, Op::CreateHold | Op::ExtendHold | Op::Expire);
        ensure!(
            !changes_objects || !unique.is_empty(),
            seq,
            "INV-6",
            "{} changes no object",
            r.op.name()
        );
        ensure!(
            changes_objects || r.op == Op::Expire || unique.is_empty(),
            seq,
            "INV-6",
            "{} names objects",
            r.op.name()
        );
        let status = |objects: &BTreeMap<Oid, Rebuilt>, o: &Oid| {
            objects.get(o).map_or(Status::FREE, |x| x.0)
        };
        let held_by = |objects: &BTreeMap<Oid, Rebuilt>, o: &Oid| objects.get(o).and_then(|x| x.1);
        match r.op {
            Op::CreateHold => {
                ensure!(hold == Some(HoldId(seq)), seq, "INV-6", "hold id {hold:?}");
                let Some(expires_at) = r.expires_at else {
                    return Err(refuse(seq, "INV-6", "hold without an expiry".into()));
                };
                ensure!(
                    r.max_objects != Some(0),
                    seq,
                    "INV-6",
                    "hold limited to no objects"
                );
                let h = ReplayHold {
                    expires_at,
                    max_objects: r.max_objects,
                    objects: BTreeSet::new(),
                };
                self.holds.insert(HoldId(seq), h);
            }
            Op::ExtendHold => {
                let h = hold.and_then(|id| self.holds.get_mut(&id));
                match (h, r.expires_at) {
                    (Some(h), Some(at)) => h.expires_at = at,
                    _ => return Err(refuse(seq, "INV-6", format!("extends {hold:?}"))),
                }
            }
            Op::Expire => {
                let Some(id) = hold else {
                    return Err(refuse(seq, "INV-6", "expires no hold".into()));
                };
                let Some(h) = self.holds.remove(&id) else {
                    return Err(refuse(seq, "INV-6", format!("expires unknown {id:?}")));
                };
                ensure!(
                    h.objects == unique,
                    seq,
                    "INV-6",
                    "expiry frees {unique:?}, the hold had {:?}",
                    h.objects
                );
                for o in &h.objects {
                    self.objects.remove(o);
                }
            }
            Op::Hold => {
                let Some(h) = hold.and_then(|id| self.holds.get(&id)) else {
                    return Err(refuse(seq, "INV-6", format!("holds for {hold:?}")));
                };
                let total = h.objects.len() + unique.len();
                ensure!(
                    h.max_objects.is_none_or(|m| total <= m as usize),
                    seq,
                    "INV-6",
                    "{hold:?} holds {total} objects, over its limit"
                );
                for o in &unique {
                    let s = status(&self.objects, o);
                    ensure!(s == Status::FREE, seq, "INV-6", "holds {o} from {s:?}");
                    self.objects.insert(*o, (Status::HELD, hold, None, seq));
                }
                if let Some(h) = hold.and_then(|id| self.holds.get_mut(&id)) {
                    h.objects.extend(&unique);
                }
            }
            Op::Release => {
                for o in &unique {
                    match hold {
                        Some(id) => ensure!(
                            held_by(&self.objects, o) == Some(id),
                            seq,
                            "INV-6",
                            "releases {o}, not held by {id:?}"
                        ),
                        None => ensure!(
                            status(&self.objects, o) == Status::BOOKED,
                            seq,
                            "INV-6",
                            "unbooks {o}, which is not booked"
                        ),
                    }
                    self.objects.remove(o);
                }
                if let Some(h) = hold.and_then(|id| self.holds.get_mut(&id)) {
                    h.objects.retain(|o| !unique.contains(o));
                }
            }
            Op::Book => {
                let Some(order_ref) = r.order_ref.clone() else {
                    return Err(refuse(seq, "INV-6", "books without an order".into()));
                };
                // 02 §6: details name only objects being booked, each once.
                let detailed: BTreeSet<Oid> = r.details.iter().map(|d| d.0).collect();
                ensure!(
                    detailed.len() == r.details.len() && detailed.is_subset(&unique),
                    seq,
                    "INV-6",
                    "booking details name {detailed:?}"
                );
                for o in &unique {
                    let s = status(&self.objects, o);
                    ensure!(
                        s != Status::BOOKED,
                        seq,
                        "INV-10",
                        "books {o} again without a release"
                    );
                    match hold {
                        Some(id) => ensure!(
                            held_by(&self.objects, o) == Some(id),
                            seq,
                            "INV-6",
                            "books {o}, not held by {id:?}"
                        ),
                        None => ensure!(s == Status::FREE, seq, "INV-6", "books {o} from {s:?}"),
                    }
                    let d = r.details.iter().find(|d| d.0 == *o);
                    let booking = Booking {
                        order_ref: order_ref.clone(),
                        ticket_type: d.and_then(|d| d.1.clone()),
                        extra: d.and_then(|d| d.2.clone()),
                    };
                    self.objects
                        .insert(*o, (Status::BOOKED, None, Some(booking), seq));
                }
                if let Some(id) = hold {
                    let Some(h) = self.holds.get_mut(&id) else {
                        return Err(refuse(seq, "INV-6", format!("books for {id:?}")));
                    };
                    h.objects.retain(|o| !unique.contains(o));
                    if h.objects.is_empty() {
                        self.holds.remove(&id);
                    }
                }
            }
            Op::ChangeStatus => {
                let target = r.status.unwrap_or(Status::HELD);
                ensure!(
                    target != Status::HELD && target != Status::BOOKED && hold.is_none(),
                    seq,
                    "INV-6",
                    "change-status to {target:?}"
                );
                for o in &unique {
                    let s = status(&self.objects, o);
                    let legal = if target == Status::FREE {
                        s != Status::FREE && s != Status::HELD && s != Status::BOOKED
                    } else {
                        s == Status::FREE
                    };
                    ensure!(legal, seq, "INV-6", "{o} from {s:?} to {target:?}");
                    if target == Status::FREE {
                        self.objects.remove(o);
                    } else {
                        self.objects.insert(*o, (target, None, None, seq));
                    }
                }
            }
        }
        self.seq = seq;
        Ok(())
    }

    /// INV-6: every way the stored rows differ from the replayed log, in
    /// order: `events.seq`, then objects by `oid`, then holds by id. Empty
    /// when they agree. A hold's token hash is not in the log and is not
    /// compared.
    pub fn compare(
        &self,
        stored_seq: u64,
        holds: impl IntoIterator<Item = (HoldId, Timestamp, Option<u32>)>,
        objects: &[ObjectRow],
    ) -> Vec<Difference> {
        let mut out = Vec::new();
        if stored_seq != self.seq {
            out.push(Difference::Seq {
                stored: stored_seq,
                log: self.seq,
            });
        }
        let mut rows: BTreeMap<Oid, &ObjectRow> = BTreeMap::new();
        for o in objects {
            if rows.insert(o.oid, o).is_some() {
                out.push(Difference::ObjectRowTwice(o.oid));
            }
        }
        let oids: BTreeSet<Oid> = rows.keys().chain(self.objects.keys()).copied().collect();
        for oid in oids {
            match (rows.get(&oid), self.objects.get(&oid)) {
                (Some(_), None) => out.push(Difference::ObjectUnexpected(oid)),
                (None, Some(_)) => out.push(Difference::ObjectMissing(oid)),
                (Some(row), Some((status, hold, booking, seq))) => {
                    let mut field = |differs: bool, f| {
                        if differs {
                            out.push(Difference::Object(oid, f));
                        }
                    };
                    let part = |b: Option<&Booking>| {
                        (
                            b.map(|b| b.order_ref.clone()),
                            b.and_then(|b| b.ticket_type.clone()),
                            b.and_then(|b| b.extra.clone()),
                        )
                    };
                    let (stored, log) = (part(row.booking.as_ref()), part(booking.as_ref()));
                    field(row.status != *status, ObjectField::Status);
                    field(row.hold_id != *hold, ObjectField::HoldId);
                    field(stored.0 != log.0, ObjectField::OrderRef);
                    field(stored.1 != log.1, ObjectField::TicketType);
                    field(stored.2 != log.2, ObjectField::Extra);
                    field(row.seq != *seq, ObjectField::Seq);
                }
                (None, None) => {}
            }
        }
        let mut stored: BTreeMap<HoldId, (Timestamp, Option<u32>)> = BTreeMap::new();
        for (id, at, max) in holds {
            if stored.insert(id, (at, max)).is_some() {
                out.push(Difference::HoldRowTwice(id));
            }
        }
        let ids: BTreeSet<HoldId> = stored.keys().chain(self.holds.keys()).copied().collect();
        for id in ids {
            match (stored.get(&id), self.holds.get(&id)) {
                (Some(_), None) => out.push(Difference::HoldUnexpected(id)),
                (None, Some(_)) => out.push(Difference::HoldMissing(id)),
                (Some((at, max)), Some(h)) => {
                    if *at != h.expires_at {
                        out.push(Difference::Hold(id, HoldField::ExpiresAt));
                    }
                    if *max != h.max_objects {
                        out.push(Difference::Hold(id, HoldField::MaxObjects));
                    }
                }
                (None, None) => {}
            }
        }
        out
    }
}
