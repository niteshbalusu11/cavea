//! The generator's own correctness checks, independent of the reconciler: it
//! sees every acknowledgement, so after a run it replays them in `seq` order
//! and asks whether two parties were ever both told they hold one seat, and
//! whether every refusal could have been right.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Hold,
    Release,
    Book,
    Block,
    Unblock,
}

/// One acknowledged command.
#[derive(Debug, Clone)]
pub struct Ack {
    pub party: Party,
    pub kind: Kind,
    pub seq: u64,
    pub objects: Vec<u32>,
    pub send_ms: i64,
    pub recv_ms: i64,
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
    /// Holds whose outcome is unknown (a transport error, a timeout, or
    /// `unavailable`, which a lost commit reply also gives): they may hold
    /// their seats from when they were sent.
    pub unknown: Vec<(Party, Vec<u32>, i64)>,
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
    Booked(Party),
    Blocked,
}

impl History {
    /// The latest local time `party`'s hold may still be live.
    fn live_until(&self, party: Party) -> i64 {
        match (self.expiry.get(&party), self.clock.lo) {
            (Some(e), lo) if lo != i64::MIN => e.saturating_add(999 - lo).saturating_add(SLACK_MS),
            _ => i64::MAX,
        }
    }

    /// True if `party`'s hold was certainly unexpired at local time `t`.
    fn certainly_live(&self, party: Party, t: i64) -> bool {
        match (self.expiry.get(&party), self.clock.hi) {
            (Some(e), hi) if hi != i64::MAX => t.saturating_add(hi).saturating_add(SLACK_MS) < *e,
            _ => false,
        }
    }
}

/// Every violation in `h`.
pub fn check(h: &History) -> Vec<Violation> {
    let mut out = Vec::new();
    let mut bad = |check, detail: String| out.push(Violation { check, detail });
    let mut acks: Vec<&Ack> = h.acks.iter().collect();
    acks.sort_by_key(|a| a.seq);
    for w in acks.windows(2) {
        if w[0].seq == w[1].seq {
            bad("seq", format!("two acknowledgements name seq {}", w[0].seq));
        }
    }
    // Per object: who owns it, and since when (local send time of the ack).
    let mut owner: HashMap<u32, (Owner, u64, i64)> = HashMap::new();
    // Every interval in which another party may have owned an object.
    let mut owned: BTreeMap<u32, Vec<(i64, i64, Party)>> = BTreeMap::new();
    let mut close = |o: u32, party: Party, start: i64, end: i64| {
        owned.entry(o).or_default().push((start, end, party));
    };
    for a in acks {
        for &o in &a.objects {
            let current = owner.get(&o).copied();
            let next = match (a.kind, current) {
                (Kind::Hold, None) => Some(Owner::Held(a.party)),
                (Kind::Hold, Some((Owner::Held(p), seq, start))) if p != a.party => {
                    if h.certainly_live(p, a.recv_ms) {
                        bad(
                            "double_hold",
                            format!(
                                "object {o}: held by party {p} at seq {seq} and by party {} at seq {}",
                                a.party, a.seq
                            ),
                        );
                    }
                    close(o, p, start, h.live_until(p));
                    Some(Owner::Held(a.party))
                }
                (Kind::Hold, Some((Owner::Held(_), ..))) => None,
                (Kind::Hold, Some((other, seq, _))) => {
                    bad(
                        "double_hold",
                        format!(
                            "object {o}: {other:?} at seq {seq}, held by party {} at seq {}",
                            a.party, a.seq
                        ),
                    );
                    None
                }
                (Kind::Release, Some((Owner::Held(p), _, start))) if p == a.party => {
                    close(o, p, start, a.recv_ms);
                    owner.remove(&o);
                    continue;
                }
                (Kind::Book, Some((Owner::Held(p), ..))) if p == a.party => Some(Owner::Booked(p)),
                (Kind::Block, None) => Some(Owner::Blocked),
                (Kind::Block, Some((Owner::Held(p), _, start)))
                    if !h.certainly_live(p, a.recv_ms) =>
                {
                    close(o, p, start, h.live_until(p));
                    Some(Owner::Blocked)
                }
                (Kind::Unblock, Some((Owner::Blocked, _, start))) => {
                    close(o, BACKEND, start, a.recv_ms);
                    owner.remove(&o);
                    continue;
                }
                (kind, current) => {
                    bad(
                        "transition",
                        format!(
                            "object {o}: {kind:?} by party {} at seq {} while {current:?}",
                            a.party, a.seq
                        ),
                    );
                    None
                }
            };
            if let Some(n) = next {
                let start = match (n, current) {
                    (Owner::Booked(_), Some((_, _, s))) => s,
                    _ => a.send_ms,
                };
                owner.insert(o, (n, a.seq, start));
            }
        }
    }
    for (o, (who, _, start)) in owner {
        match who {
            Owner::Held(p) => close(o, p, start, h.live_until(p)),
            Owner::Booked(p) => close(o, p, start, i64::MAX),
            Owner::Blocked => close(o, BACKEND, start, i64::MAX),
        }
    }
    for (party, objects, sent) in &h.unknown {
        for &o in objects {
            close(o, *party, *sent, h.live_until(*party));
        }
    }
    // A refusal is right if someone else may have owned the object then.
    for r in &h.refusals {
        for &o in &r.objects {
            let explained = h.before.contains(&o)
                || owned.get(&o).is_some_and(|v| {
                    v.iter().any(|&(start, end, p)| {
                        p != r.party && start < r.recv_ms && end > r.send_ms
                    })
                });
            if !explained {
                bad(
                    "wrong_refusal",
                    format!(
                        "object {o}: refused to party {} as not free, but no one else held it",
                        r.party
                    ),
                );
            }
        }
    }
    out
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
        // A hold whose reply was lost may own the seat.
        h.unknown.push((1, vec![7], 5_500));
        h.refusals = vec![refusal(3, 7, 6_000)];
        assert!(checks(&h).is_empty());
    }
}
