//! The generator's own correctness checks, independent of the reconciler: it
//! sees every reply, so after a run it replays the acknowledgements in `seq`
//! order and asks whether two parties were ever both told they hold one seat,
//! and whether every refusal could have been right; then it compares what it
//! was told with what the server has committed (the audit).
//!
//! A request whose reply never arrived (the connection broke after it was
//! written, it timed out, or the server answered `503 unavailable`, which a
//! commit that failed or lost its reply also gives) has an **unknown** outcome:
//! it may or may not have taken effect. The checks that accuse the server use
//! only replies the generator received; an unknown outcome only ever excuses
//! (a seat a party may have released, a refusal another party's unknown hold
//! may explain). The audit then resolves unknowns from the committed state.
//!
//! Times are wall-clock milliseconds on the generator; the server's clock is
//! bounded from token creations, whose `expires_at` minus the lifetime is a
//! server time taken between the request and its response. The server gives
//! `expires_at` to the second, so the true instant is up to 999 ms later.

use std::collections::{BTreeMap, HashMap, HashSet};

/// A buyer (its index, one hold token each) or the backend.
pub type Party = u32;
pub const BACKEND: Party = u32::MAX;

/// How late an expiry may be processed after its instant, and the error
/// allowed between the two clocks beyond the bound: slack in the checks'
/// favour, so that a slow server is never reported as a wrong one.
pub const SLACK_MS: i64 = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A hold token created (a command with a `seq`, no objects).
    CreateHold,
    Hold,
    Release,
    Book,
    Block,
    Unblock,
    /// The backend releasing a booking (a cancellation).
    Unbook,
}

impl Kind {
    /// The `op` the server logs for it (`event_log.op`, 08 §5).
    pub fn op(self) -> u8 {
        match self {
            Self::CreateHold => 1,
            Self::Hold => 2,
            Self::Release | Self::Unbook => 3,
            Self::Book => 5,
            Self::Block | Self::Unblock => 6,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::CreateHold => "create_hold",
            Self::Hold => "hold",
            Self::Release => "release",
            Self::Book => "book",
            Self::Block => "block",
            Self::Unblock => "unblock",
            Self::Unbook => "unbook",
        }
    }
}

/// One acknowledged command.
#[derive(Debug, Clone)]
pub struct Ack {
    pub party: Party,
    pub kind: Kind,
    pub seq: u64,
    pub objects: Vec<u32>,
    /// When the request was first sent (an earlier attempt's, if retried).
    pub send_ms: i64,
    pub recv_ms: i64,
    /// An earlier attempt at this request had an unknown outcome, so this
    /// success may be a no-op that names the `seq` it observed, not its own.
    pub retried: bool,
    /// A booking's number (its order reference), or the booking a
    /// cancellation cancels.
    pub order: Option<u64>,
}

/// A request whose outcome the generator never learned: every attempt it
/// made broke, timed out or was answered `unavailable`, and it gave up.
#[derive(Debug, Clone)]
pub struct Unknown {
    pub party: Party,
    pub kind: Kind,
    pub objects: Vec<u32>,
    pub send_ms: i64,
    pub order: Option<u64>,
}

/// A request answered with a refusal or error: it must not have taken effect.
#[derive(Debug, Clone)]
pub struct Failed {
    pub party: Party,
    pub kind: Kind,
    pub objects: Vec<u32>,
    pub order: Option<u64>,
}

/// A hold refused with `object_not_free`, naming these objects.
#[derive(Debug, Clone)]
pub struct Refusal {
    pub party: Party,
    pub objects: Vec<u32>,
    pub send_ms: i64,
    pub recv_ms: i64,
}

/// The offset of the server's clock from the generator's (server − local),
/// bounded from below and above.
#[derive(Debug, Clone, Copy)]
pub struct ClockBound {
    pub lo: i64,
    pub hi: i64,
}

impl Default for ClockBound {
    fn default() -> Self {
        Self {
            lo: i64::MIN,
            hi: i64::MAX,
        }
    }
}

impl ClockBound {
    /// A server time, known to lie in `server_lo..=server_hi`, taken between
    /// `send` and `recv`.
    pub fn observe(&mut self, server_lo: i64, server_hi: i64, send_ms: i64, recv_ms: i64) {
        self.lo = self.lo.max(server_lo - recv_ms);
        self.hi = self.hi.min(server_hi - send_ms);
    }
}

#[derive(Debug, Default)]
pub struct History {
    pub acks: Vec<Ack>,
    pub refusals: Vec<Refusal>,
    pub unknown: Vec<Unknown>,
    pub failed: Vec<Failed>,
    /// Each buyer's token expiry, server milliseconds, to the second (the
    /// true instant is at most 999 ms later).
    pub expiry: HashMap<Party, i64>,
    /// Objects unavailable when the run started.
    pub before: HashSet<u32>,
    pub clock: ClockBound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub check: &'static str,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Held(Party),
    /// The party and the booking's number.
    Booked(Party, Option<u64>),
    Blocked,
}

/// Where the replay of the acknowledgements ends.
struct Replayed {
    violations: Vec<Violation>,
    /// Each object's owner after the last acknowledgement, the `seq` it was
    /// acknowledged at.
    owner: HashMap<u32, (Owner, u64)>,
    /// Every interval in which another party may have owned an object.
    owned: BTreeMap<u32, Vec<(i64, i64, Party)>>,
    /// Objects whose last acknowledgement freed them, and how (release,
    /// cancellation, unblock), with the party that had them.
    freed: HashMap<u32, (Kind, Party)>,
}

impl History {
    /// The latest local time `party`'s hold may still be live.
    pub fn live_until(&self, party: Party) -> i64 {
        match (self.expiry.get(&party), self.clock.lo) {
            (Some(e), lo) if lo != i64::MIN => e.saturating_add(999 - lo).saturating_add(SLACK_MS),
            _ => i64::MAX,
        }
    }

    /// True if `party`'s hold was certainly unexpired at local time `t`.
    pub fn certainly_live(&self, party: Party, t: i64) -> bool {
        match (self.expiry.get(&party), self.clock.hi) {
            (Some(e), hi) if hi != i64::MAX => t.saturating_add(hi).saturating_add(SLACK_MS) < *e,
            _ => false,
        }
    }

    /// Whether a request with an unknown outcome may have ended `current`'s
    /// hold on `o` before local time `t`.
    fn may_have_ended(&self, current: Owner, o: u32, t: i64) -> bool {
        self.unknown.iter().any(|u| {
            u.send_ms < t
                && u.objects.contains(&o)
                && match current {
                    Owner::Held(p) => u.kind == Kind::Release && u.party == p,
                    Owner::Booked(_, n) => u.kind == Kind::Unbook && u.order == n,
                    Owner::Blocked => u.kind == Kind::Unblock,
                }
        })
    }

    fn replay(&self) -> Replayed {
        let mut out = Vec::new();
        let mut bad = |check, detail: String| out.push(Violation { check, detail });
        let mut acks: Vec<&Ack> = self.acks.iter().collect();
        acks.sort_by_key(|a| a.seq);
        // A retried success may name a `seq` it observed, so only first
        // attempts' `seq`s must be their own.
        let own: Vec<&&Ack> = acks.iter().filter(|a| !a.retried).collect();
        for w in own.windows(2) {
            if w[0].seq == w[1].seq {
                bad("seq", format!("two acknowledgements name seq {}", w[0].seq));
            }
        }
        // Per object: who owns it, at which `seq`, and since when (local send
        // time of the ack).
        let mut owner: HashMap<u32, (Owner, u64, i64)> = HashMap::new();
        let mut owned: BTreeMap<u32, Vec<(i64, i64, Party)>> = BTreeMap::new();
        let mut freed = HashMap::new();
        let mut close = |o: u32, party: Party, start: i64, end: i64| {
            owned.entry(o).or_default().push((start, end, party));
        };
        for a in acks {
            for &o in &a.objects {
                let mut current = owner.get(&o).copied();
                // What the acknowledgement needs to find, if it is not free.
                let compatible = match (a.kind, current) {
                    (Kind::Hold | Kind::Release | Kind::Book, Some((Owner::Held(p), ..))) => {
                        p == a.party
                    }
                    (Kind::Unbook, Some((Owner::Booked(..), ..)))
                    | (Kind::Unblock, Some((Owner::Blocked, ..))) => true,
                    _ => current.is_none(),
                };
                if !compatible
                    && let Some((c, _, start)) = current
                    && self.may_have_ended(c, o, a.recv_ms)
                {
                    // An unknown release, cancellation or unblock may have
                    // freed it: no accusation, but the owner may have had it
                    // until now.
                    let p = match c {
                        Owner::Held(p) | Owner::Booked(p, _) => p,
                        Owner::Blocked => BACKEND,
                    };
                    close(o, p, start, a.recv_ms);
                    owner.remove(&o);
                    current = None;
                }
                let next = match (a.kind, current) {
                    (Kind::CreateHold, _) => None,
                    (Kind::Hold, None) => Some(Owner::Held(a.party)),
                    (Kind::Hold, Some((Owner::Held(p), seq, start))) if p != a.party => {
                        if self.certainly_live(p, a.recv_ms) {
                            bad(
                                "double_hold",
                                format!(
                                    "idx {o}: held by party {p} at seq {seq} and by party {} at seq {}",
                                    a.party, a.seq
                                ),
                            );
                        }
                        close(o, p, start, self.live_until(p));
                        Some(Owner::Held(a.party))
                    }
                    (Kind::Hold, Some((Owner::Held(_), ..))) => None,
                    (Kind::Hold, Some((other, seq, _))) => {
                        bad(
                            "double_hold",
                            format!(
                                "idx {o}: {other:?} at seq {seq}, held by party {} at seq {}",
                                a.party, a.seq
                            ),
                        );
                        None
                    }
                    (Kind::Release, Some((Owner::Held(p), _, start))) if p == a.party => {
                        close(o, p, start, a.recv_ms);
                        owner.remove(&o);
                        freed.insert(o, (Kind::Release, p));
                        continue;
                    }
                    (Kind::Book, Some((Owner::Held(p), ..))) if p == a.party => {
                        Some(Owner::Booked(p, a.order))
                    }
                    (Kind::Block, None) => Some(Owner::Blocked),
                    (Kind::Block, Some((Owner::Held(p), _, start)))
                        if !self.certainly_live(p, a.recv_ms) =>
                    {
                        close(o, p, start, self.live_until(p));
                        Some(Owner::Blocked)
                    }
                    (Kind::Unblock, Some((Owner::Blocked, _, start))) => {
                        close(o, BACKEND, start, a.recv_ms);
                        owner.remove(&o);
                        freed.insert(o, (Kind::Unblock, BACKEND));
                        continue;
                    }
                    (Kind::Unbook, Some((Owner::Booked(p, _), _, start))) => {
                        close(o, p, start, a.recv_ms);
                        owner.remove(&o);
                        freed.insert(o, (Kind::Unbook, p));
                        continue;
                    }
                    (kind, current) => {
                        bad(
                            "transition",
                            format!(
                                "idx {o}: {kind:?} by party {} at seq {} while {current:?}",
                                a.party, a.seq
                            ),
                        );
                        None
                    }
                };
                if let Some(n) = next {
                    let start = match (n, current) {
                        (Owner::Booked(..), Some((_, _, s))) => s,
                        _ => a.send_ms,
                    };
                    owner.insert(o, (n, a.seq, start));
                    freed.remove(&o);
                }
            }
        }
        let mut end = HashMap::new();
        for (&o, &(who, seq, start)) in &owner {
            match who {
                Owner::Held(p) => close(o, p, start, self.live_until(p)),
                Owner::Booked(p, _) => close(o, p, start, i64::MAX),
                Owner::Blocked => close(o, BACKEND, start, i64::MAX),
            }
            end.insert(o, (who, seq));
        }
        // Unknown outcomes that may have made a seat unavailable.
        for u in &self.unknown {
            let until = match u.kind {
                Kind::Hold => self.live_until(u.party),
                Kind::Book | Kind::Block => i64::MAX,
                _ => continue,
            };
            for &o in &u.objects {
                close(o, u.party, u.send_ms, until);
            }
        }
        Replayed {
            violations: out,
            owner: end,
            owned,
            freed,
        }
    }

    /// The parties whose tokens the audit must read: every party that may
    /// hold something at local time `t` by an acknowledgement or an unknown
    /// outcome, or was refused a hold (which must not have taken effect).
    pub fn parties_to_read(&self, t: i64) -> Vec<Party> {
        let r = self.replay();
        let mut parties: HashSet<Party> = r
            .owner
            .values()
            .filter_map(|(o, _)| match o {
                Owner::Held(p) => Some(*p),
                _ => None,
            })
            .collect();
        parties.extend(
            self.unknown
                .iter()
                .filter(|u| matches!(u.kind, Kind::Hold | Kind::Release | Kind::Book))
                .map(|u| u.party),
        );
        parties.extend(
            self.failed
                .iter()
                .filter(|f| f.kind == Kind::Hold)
                .map(|f| f.party),
        );
        let mut v: Vec<Party> = parties
            .into_iter()
            .filter(|&p| p != BACKEND && self.live_until(p) >= t)
            .collect();
        v.sort_unstable();
        v
    }
}

/// Every violation in `h`'s replies.
pub fn check(h: &History) -> Vec<Violation> {
    let r = h.replay();
    let mut out = r.violations;
    // A refusal is right if someone else may have owned the object then.
    for f in &h.refusals {
        for &o in &f.objects {
            let explained = h.before.contains(&o)
                || r.owned.get(&o).is_some_and(|v| {
                    v.iter().any(|&(start, end, p)| {
                        p != f.party && start < f.recv_ms && end > f.send_ms
                    })
                });
            if !explained {
                out.push(Violation {
                    check: "wrong_refusal",
                    detail: format!(
                        "idx {o}: refused to party {} as not free, but no one else held it",
                        f.party
                    ),
                });
            }
        }
    }
    out
}

/// An object the server has committed as not free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen {
    /// Its hold's expiry, Unix milliseconds to the second.
    Held(i64),
    /// The booking's number, if its order reference is one of the generator's.
    Booked(Option<u64>),
    Blocked,
    Other,
}

/// The committed state, read after the run.
#[derive(Debug, Default)]
pub struct Observed {
    /// Every object not free.
    pub objects: HashMap<u32, Seen>,
    /// What each party read holds; `None`: no live token.
    pub tokens: HashMap<Party, Option<Vec<u32>>>,
    /// The local times the reads started and ended.
    pub start_ms: i64,
    pub end_ms: i64,
}

/// How the unknown outcomes resolved, by kind: applied, not applied, or not
/// decidable from the state now (the hold has expired since, or another
/// command has changed the seat).
pub type Resolution = BTreeMap<&'static str, [u64; 3]>;

/// What the server has committed against what the generator was told: every
/// acknowledged command still in effect is there (`lost_ack`), nothing it was
/// told failed has taken effect (`failed_took_effect`), nothing is there that
/// no request explains (`unexplained`), and no hold outlived its expiry by
/// more than the slack (`expiry_late`).
pub fn audit(h: &History, seen: &Observed) -> (Vec<Violation>, Resolution) {
    let r = h.replay();
    let mut out = Vec::new();
    let mut bad = |check, detail: String| out.push(Violation { check, detail });
    let unknown = |o: u32, kind: Kind, party: Option<Party>| {
        h.unknown
            .iter()
            .any(|u| u.kind == kind && u.objects.contains(&o) && party.is_none_or(|p| p == u.party))
    };
    let failed = |o: u32, kind: Kind, party: Party| {
        h.failed
            .iter()
            .any(|f| f.kind == kind && f.party == party && f.objects.contains(&o))
    };
    let reads = |o: u32| -> Vec<Party> {
        seen.tokens
            .iter()
            .filter(|(_, t)| t.as_ref().is_some_and(|t| t.contains(&o)))
            .map(|(&p, _)| p)
            .collect()
    };
    let acked = |o: u32, kind: Kind, party: Party| {
        h.acks
            .iter()
            .any(|a| a.kind == kind && a.party == party && a.objects.contains(&o))
    };
    // What should still be there.
    for (&o, &(who, seq)) in &r.owner {
        if h.before.contains(&o) {
            continue;
        }
        let now = seen.objects.get(&o).copied();
        match who {
            Owner::Booked(_, n) => {
                if now != Some(Seen::Booked(n)) && !unknown(o, Kind::Unbook, None) {
                    bad(
                        "lost_ack",
                        format!("idx {o}: booking acknowledged at seq {seq} missing; now {now:?}"),
                    );
                }
            }
            Owner::Blocked => {
                if now != Some(Seen::Blocked) && !unknown(o, Kind::Unblock, None) {
                    bad(
                        "lost_ack",
                        format!("idx {o}: block acknowledged at seq {seq} missing; now {now:?}"),
                    );
                }
            }
            Owner::Held(p) => {
                let has = seen
                    .tokens
                    .get(&p)
                    .and_then(Option::as_ref)
                    .is_some_and(|t| t.contains(&o));
                if h.certainly_live(p, seen.end_ms)
                    && !has
                    && !unknown(o, Kind::Release, Some(p))
                    && !unknown(o, Kind::Book, Some(p))
                {
                    bad(
                        "lost_ack",
                        format!(
                            "idx {o}: hold by party {p} acknowledged at seq {seq} and live, missing; now {now:?}"
                        ),
                    );
                }
            }
        }
    }
    // What is there must be explained by something acknowledged or unknown.
    for (&o, &now) in &seen.objects {
        if h.before.contains(&o) {
            continue;
        }
        let end = r.owner.get(&o).map(|e| e.0);
        match now {
            Seen::Booked(n) => {
                let ok = end.is_some_and(|e| matches!(e, Owner::Booked(_, m) if m == n))
                    || h.unknown
                        .iter()
                        .any(|u| u.kind == Kind::Book && u.order == n && u.objects.contains(&o));
                if !ok {
                    let check = if h.failed.iter().any(|f| {
                        f.kind == Kind::Book && f.order == n && f.objects.contains(&o)
                    }) {
                        "failed_took_effect"
                    } else if r.freed.get(&o).is_some_and(|f| f.0 == Kind::Unbook) {
                        "lost_ack"
                    } else {
                        "unexplained"
                    };
                    bad(check, format!("idx {o}: booked, as {end:?} was expected"));
                }
            }
            Seen::Blocked => {
                if end != Some(Owner::Blocked) && !unknown(o, Kind::Block, None) {
                    let check = if r.freed.get(&o).is_some_and(|f| f.0 == Kind::Unblock) {
                        "lost_ack"
                    } else {
                        "unexplained"
                    };
                    bad(check, format!("idx {o}: blocked, as {end:?} was expected"));
                }
            }
            Seen::Held(expires) => {
                if !reads(o).is_empty() {
                    continue;
                }
                // No live token read holds it: an expiry not processed, or a
                // hold the generator never made or was told was released.
                let server_start = seen.start_ms.saturating_add(h.clock.lo);
                let late = h.clock.lo != i64::MIN && expires + 999 + SLACK_MS < server_start;
                // Released by a party whose token has this expiry.
                let released = r.freed.get(&o).is_some_and(|f| {
                    f.0 == Kind::Release && h.expiry.get(&f.1) == Some(&expires)
                });
                // A party not read, or whose token had expired by the time
                // it was read (the objects are listed first).
                let possible = h.expiry.iter().any(|(p, e)| {
                    *e == expires
                        && (acked(o, Kind::Hold, *p) || unknown(o, Kind::Hold, Some(*p)))
                        && seen.tokens.get(p).is_none_or(Option::is_none)
                });
                if late {
                    bad(
                        "expiry_late",
                        format!("idx {o}: still held more than {SLACK_MS} ms after its expiry"),
                    );
                } else if released {
                    bad(
                        "lost_ack",
                        format!("idx {o}: held, though its release was acknowledged"),
                    );
                } else if !possible {
                    bad(
                        "unexplained",
                        format!("idx {o}: held by no token the generator knows"),
                    );
                }
            }
            Seen::Other => bad("unexplained", format!("idx {o}: a status no request sets")),
        }
    }
    // Every object a token read holds.
    for (&p, t) in &seen.tokens {
        for &o in t.iter().flatten() {
            if acked(o, Kind::Hold, p) {
                if r.freed.get(&o) == Some(&(Kind::Release, p))
                    && !matches!(r.owner.get(&o), Some((Owner::Held(q), _)) if *q == p)
                {
                    bad(
                        "lost_ack",
                        format!("idx {o}: party {p} still holds it, though its release was acknowledged"),
                    );
                }
            } else if !unknown(o, Kind::Hold, Some(p)) {
                let check = if failed(o, Kind::Hold, p) {
                    "failed_took_effect"
                } else {
                    "unexplained"
                };
                bad(
                    check,
                    format!("idx {o}: party {p} holds it without an acknowledged hold"),
                );
            }
        }
    }
    // How each unknown outcome resolved.
    let mut res: Resolution = BTreeMap::new();
    for u in &h.unknown {
        let token = seen.tokens.get(&u.party).cloned().flatten();
        let all = |s: &Seen| u.objects.iter().all(|o| seen.objects.get(o) == Some(s));
        let none = |s: &Seen| u.objects.iter().all(|o| seen.objects.get(o) != Some(s));
        let holds = |t: &Option<Vec<u32>>, all: bool| {
            t.as_ref().is_some_and(|t| {
                if all {
                    u.objects.iter().all(|o| t.contains(o))
                } else {
                    u.objects.iter().any(|o| t.contains(o))
                }
            })
        };
        let live = h.certainly_live(u.party, seen.end_ms);
        let outcome = match u.kind {
            Kind::Hold if holds(&token, true) => 0,
            Kind::Hold if live && !holds(&token, false) => 1,
            Kind::Release if live && !holds(&token, false) => 0,
            Kind::Release if holds(&token, true) => 1,
            Kind::Book if all(&Seen::Booked(u.order)) => 0,
            Kind::Book if live && holds(&token, true) => 1,
            Kind::Unbook if all(&Seen::Booked(u.order)) => 1,
            Kind::Unbook if none(&Seen::Booked(u.order)) => 0,
            Kind::Block if all(&Seen::Blocked) => 0,
            Kind::Unblock if none(&Seen::Blocked) => 0,
            Kind::Unblock if all(&Seen::Blocked) => 1,
            _ => 2,
        };
        if let Some(c) = res.entry(u.kind.name()).or_default().get_mut(outcome) {
            *c += 1;
        }
    }
    (out, res)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ack(party: Party, kind: Kind, seq: u64, objects: &[u32], at: i64) -> Ack {
        Ack {
            party,
            kind,
            seq,
            objects: objects.to_vec(),
            send_ms: at,
            recv_ms: at + 2,
            retried: false,
            order: None,
        }
    }

    fn booking(party: Party, seq: u64, objects: &[u32], at: i64, n: u64) -> Ack {
        Ack {
            order: Some(n),
            ..ack(party, Kind::Book, seq, objects, at)
        }
    }

    fn unknown(party: Party, kind: Kind, objects: &[u32], at: i64) -> Unknown {
        Unknown {
            party,
            kind,
            objects: objects.to_vec(),
            send_ms: at,
            order: None,
        }
    }

    /// Clocks agree to the millisecond; buyers 1 and 2 hold tokens that expire
    /// at 60,000 and 200,000.
    fn history(acks: Vec<Ack>) -> History {
        let mut clock = ClockBound::default();
        clock.observe(1_000, 1_000, 999, 1_001);
        History {
            acks,
            expiry: HashMap::from([(1, 60_000), (2, 200_000), (3, 200_000)]),
            clock,
            ..History::default()
        }
    }

    fn checks(h: &History) -> Vec<&'static str> {
        check(h).iter().map(|v| v.check).collect()
    }

    #[test]
    fn a_double_hold_is_found_and_legal_reuse_is_not() {
        // Buyer 2 told it holds seat 7 while buyer 1's live hold has it.
        let h = history(vec![
            ack(1, Kind::Hold, 3, &[7, 8], 1_000),
            ack(2, Kind::Hold, 5, &[7], 2_000),
        ]);
        assert_eq!(checks(&h), ["double_hold"]);
        // The same, acknowledged out of order: `seq` decides.
        let h = history(vec![
            ack(2, Kind::Hold, 5, &[7], 2_000),
            ack(1, Kind::Hold, 3, &[7], 1_000),
        ]);
        assert_eq!(checks(&h), ["double_hold"]);
        // Released, then held by another: fine.
        let h = history(vec![
            ack(1, Kind::Hold, 3, &[7], 1_000),
            ack(1, Kind::Release, 4, &[7], 1_500),
            ack(2, Kind::Hold, 5, &[7], 2_000),
        ]);
        assert!(checks(&h).is_empty());
        // Expired, then held by another: fine; but not a second before expiry.
        let h = history(vec![
            ack(1, Kind::Hold, 3, &[7], 1_000),
            ack(2, Kind::Hold, 5, &[7], 61_000),
        ]);
        assert!(checks(&h).is_empty());
        let h = history(vec![
            ack(1, Kind::Hold, 3, &[7], 1_000),
            ack(2, Kind::Hold, 5, &[7], 50_000),
        ]);
        assert_eq!(checks(&h), ["double_hold"]);
        // Booked: nobody may hold it again, ever.
        let h = history(vec![
            ack(1, Kind::Hold, 3, &[7], 1_000),
            ack(BACKEND, Kind::Book, 4, &[], 1_200),
            ack(1, Kind::Book, 6, &[7], 1_500),
            ack(2, Kind::Hold, 9, &[7], 500_000),
        ]);
        assert_eq!(checks(&h), ["double_hold"]);
        // Blocked and unblocked, then held.
        let h = history(vec![
            ack(BACKEND, Kind::Block, 1, &[7], 100),
            ack(BACKEND, Kind::Unblock, 2, &[7], 200),
            ack(2, Kind::Hold, 3, &[7], 300),
        ]);
        assert!(checks(&h).is_empty());
        // Booked, the booking cancelled, then held by another.
        let h = history(vec![
            ack(1, Kind::Hold, 3, &[7], 1_000),
            ack(1, Kind::Book, 4, &[7], 1_200),
            ack(BACKEND, Kind::Unbook, 5, &[7], 1_500),
            ack(2, Kind::Hold, 6, &[7], 2_000),
        ]);
        assert!(checks(&h).is_empty());
        // Two acknowledgements of one seq.
        let h = history(vec![
            ack(1, Kind::Hold, 3, &[7], 1_000),
            ack(2, Kind::Hold, 3, &[9], 1_000),
        ]);
        assert_eq!(checks(&h), ["seq"]);
        // A release of a seat the party does not hold.
        let h = history(vec![
            ack(1, Kind::Hold, 3, &[7], 1_000),
            ack(2, Kind::Release, 4, &[7], 2_000),
        ]);
        assert_eq!(checks(&h), ["transition"]);
    }

    /// Unknown outcomes only excuse: a release, cancellation or unblock that
    /// may have happened lets another party have the seat; a retried success
    /// may share a `seq`. Without them each history is a violation.
    #[test]
    fn unknown_outcomes_never_accuse() {
        let cases: Vec<(Vec<Ack>, Unknown)> = vec![
            (
                vec![
                    ack(2, Kind::Hold, 3, &[7], 1_000),
                    ack(3, Kind::Hold, 5, &[7], 3_000),
                ],
                unknown(2, Kind::Release, &[7], 2_000),
            ),
            (
                vec![
                    ack(2, Kind::Hold, 3, &[7], 1_000),
                    booking(2, 4, &[7], 1_200, 9),
                    ack(3, Kind::Hold, 6, &[7], 3_000),
                ],
                Unknown {
                    order: Some(9),
                    ..unknown(BACKEND, Kind::Unbook, &[7], 2_000)
                },
            ),
            (
                vec![
                    ack(BACKEND, Kind::Block, 3, &[7], 1_000),
                    ack(3, Kind::Hold, 5, &[7], 3_000),
                ],
                unknown(BACKEND, Kind::Unblock, &[7], 2_000),
            ),
        ];
        for (acks, u) in cases {
            let mut h = history(acks);
            assert_eq!(checks(&h), ["double_hold"]);
            h.unknown.push(u);
            assert!(checks(&h).is_empty(), "{:?}", check(&h));
        }
        // An unknown release sent after the other hold was acknowledged
        // excuses nothing.
        let mut h = history(vec![
            ack(2, Kind::Hold, 3, &[7], 1_000),
            ack(3, Kind::Hold, 5, &[7], 3_000),
        ]);
        h.unknown.push(unknown(2, Kind::Release, &[7], 4_000));
        assert_eq!(checks(&h), ["double_hold"]);
        // A retried hold whose success names the `seq` it observed.
        let mut h = history(vec![
            ack(2, Kind::Hold, 3, &[7], 1_000),
            ack(3, Kind::Hold, 3, &[9], 1_000),
        ]);
        assert_eq!(checks(&h), ["seq"]);
        h.acks[1].retried = true;
        assert!(checks(&h).is_empty());
    }

    #[test]
    fn refusals_need_another_owner() {
        let refusal = |party, o: u32, at| Refusal {
            party,
            objects: vec![o],
            send_ms: at,
            recv_ms: at + 3,
        };
        let mut h = history(vec![
            ack(1, Kind::Hold, 3, &[7], 1_000),
            ack(1, Kind::Release, 4, &[7], 5_000),
            ack(2, Kind::Hold, 5, &[8], 9_000),
        ]);
        // While buyer 1 held 7; racing buyer 2's hold of 8 (sent before the
        // refusal arrived); 9 was unavailable before the run.
        h.refusals = vec![refusal(3, 7, 3_000), refusal(3, 8, 8_999), refusal(3, 9, 1)];
        h.before.insert(9);
        assert!(checks(&h).is_empty());
        // After buyer 1 released 7; and seat 8 refused to its own holder only.
        h.refusals = vec![refusal(3, 7, 6_000), refusal(2, 8, 10_000)];
        assert_eq!(checks(&h), ["wrong_refusal", "wrong_refusal"]);
        // A hold or a block whose reply was lost may own the seat.
        h.refusals = vec![refusal(3, 7, 6_000)];
        h.unknown.push(unknown(1, Kind::Hold, &[7], 5_500));
        assert!(checks(&h).is_empty());
        h.unknown = vec![unknown(BACKEND, Kind::Block, &[7], 5_500)];
        assert!(checks(&h).is_empty());
    }

    fn audits(h: &History, seen: &Observed) -> Vec<&'static str> {
        let mut v: Vec<_> = audit(h, seen).0.iter().map(|v| v.check).collect();
        v.sort_unstable();
        v
    }

    /// Buyer 2 (live until 200,000) holds 7; buyer 3's booking 4 has 8 and 9;
    /// 10 is blocked; buyer 1's hold of 11 has expired; audited at 100,000.
    fn audited() -> (History, Observed) {
        let h = history(vec![
            ack(2, Kind::Hold, 3, &[7], 1_000),
            ack(3, Kind::Hold, 4, &[8, 9], 1_000),
            booking(3, 5, &[8, 9], 1_100, 4),
            ack(BACKEND, Kind::Block, 6, &[10], 1_200),
            ack(1, Kind::Hold, 7, &[11], 1_300),
        ]);
        let seen = Observed {
            objects: HashMap::from([
                (7, Seen::Held(200_000)),
                (8, Seen::Booked(Some(4))),
                (9, Seen::Booked(Some(4))),
                (10, Seen::Blocked),
            ]),
            tokens: HashMap::from([(2, Some(vec![7])), (3, Some(vec![]))]),
            start_ms: 100_000,
            end_ms: 100_100,
        };
        (h, seen)
    }

    #[test]
    fn the_audit_finds_lost_acknowledgements_and_failures_that_took_effect() {
        let (h, seen) = audited();
        assert!(audits(&h, &seen).is_empty(), "{:?}", audit(&h, &seen).0);
        assert_eq!(h.parties_to_read(100_000), [2]);
        // A party refused a hold is read too: the refusal must hold.
        let mut refused = audited().0;
        refused.failed.push(Failed {
            party: 3,
            kind: Kind::Hold,
            objects: vec![12],
            order: None,
        });
        assert_eq!(refused.parties_to_read(100_000), [2, 3]);
        // An acknowledged hold, booking or block missing.
        for (o, gone) in [(7, true), (8, false), (10, false)] {
            let mut s = Observed {
                objects: seen.objects.clone(),
                tokens: seen.tokens.clone(),
                ..seen
            };
            s.objects.remove(&o);
            if gone {
                s.tokens.insert(2, Some(vec![]));
            }
            assert_eq!(audits(&h, &s), ["lost_ack"], "{o}");
        }
        // The booking under another order reference.
        let mut s = Observed {
            objects: seen.objects.clone(),
            tokens: seen.tokens.clone(),
            ..seen
        };
        s.objects.insert(9, Seen::Booked(Some(5)));
        assert_eq!(audits(&h, &s), ["lost_ack", "unexplained"]);
        // A refused hold held after all; a refused booking booked.
        let mut h2 = audited().0;
        h2.failed.push(Failed {
            party: 2,
            kind: Kind::Hold,
            objects: vec![12],
            order: None,
        });
        h2.failed.push(Failed {
            party: 1,
            kind: Kind::Book,
            objects: vec![13],
            order: Some(6),
        });
        let mut s = audited().1;
        s.tokens.insert(2, Some(vec![7, 12]));
        s.objects.insert(12, Seen::Held(200_000));
        s.objects.insert(13, Seen::Booked(Some(6)));
        assert_eq!(
            audits(&h2, &s),
            ["failed_took_effect", "failed_took_effect"]
        );
        // An acknowledged release, cancellation or unblock not in effect.
        let (mut h3, mut s) = audited();
        h3.acks.push(ack(2, Kind::Release, 8, &[7], 2_000));
        h3.acks.push(ack(BACKEND, Kind::Unbook, 9, &[8], 2_000));
        h3.acks.push(ack(BACKEND, Kind::Unblock, 10, &[10], 2_000));
        s.tokens.insert(2, Some(vec![]));
        assert_eq!(audits(&h3, &s), ["lost_ack", "lost_ack", "lost_ack"]);
        // A hold long past its expiry still held.
        let (h4, mut s) = audited();
        s.objects.insert(11, Seen::Held(60_000));
        assert_eq!(audits(&h4, &s), ["expiry_late"]);
        // Listed as held, then expired before its token was read: a race of
        // the audit, not a fault; held by nobody, it is.
        let (h5, mut s) = audited();
        s.start_ms = 59_000;
        s.end_ms = 59_100;
        s.objects.insert(11, Seen::Held(60_000));
        s.tokens.insert(1, None);
        assert!(audits(&h5, &s).is_empty(), "{:?}", audit(&h5, &s).0);
        s.objects.insert(12, Seen::Held(60_000));
        assert_eq!(audits(&h5, &s), ["unexplained"]);
    }

    #[test]
    fn unknown_outcomes_resolve_from_the_committed_state() {
        let (mut h, mut seen) = audited();
        h.unknown = vec![
            // Applied: 12 is held by buyer 2.
            unknown(2, Kind::Hold, &[12], 2_000),
            // Not applied: buyer 3 is live and holds nothing.
            unknown(3, Kind::Hold, &[13], 2_000),
            // Buyer 1 has expired: undecidable.
            unknown(1, Kind::Hold, &[14], 2_000),
            // The cancellation of booking 4 did not happen.
            Unknown {
                order: Some(4),
                ..unknown(BACKEND, Kind::Unbook, &[8, 9], 3_000)
            },
        ];
        seen.tokens.insert(2, Some(vec![7, 12]));
        seen.objects.insert(12, Seen::Held(200_000));
        let (v, res) = audit(&h, &seen);
        assert!(v.is_empty(), "{v:?}");
        assert_eq!(res["hold"], [1, 1, 1]);
        assert_eq!(res["unbook"], [0, 1, 0]);
        // Without the unknown hold, buyer 2's 12 is unexplained.
        h.unknown.remove(0);
        assert_eq!(audits(&h, &seen), ["unexplained"]);
    }
}
