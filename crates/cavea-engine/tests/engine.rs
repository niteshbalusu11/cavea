//! Engine behaviour through the public API: the state machine on its own, then
//! the actor against the in-memory store with a manual clock.

#![allow(
    clippy::unwrap_used,
    reason = "test helpers outside #[test] functions; a panic is a test failure"
)]

use std::sync::Arc;

use cavea_engine::{
    ActorConfig, ActorError, Authority, Batch, BookDetail, Booking, ChartView, Command, EventId,
    EventState, Failure, Fault, HoldId, Idempotency, Idx, InvalidReason, LoadedEvent, ManualClock,
    MemoryStore, Missing, ObjectFailure, ObjectReason, ObjectRef, Op, Outcome, Request, SeatStore,
    Status, StoreError, Timestamp, TokenHash, actor, limits,
};
use tokio::sync::oneshot;

const EVENT: EventId = EventId(7);
const T0: Timestamp = Timestamp(1_000_000);

fn chart(n: u32) -> Arc<ChartView> {
    Arc::new(ChartView::new((0..n).map(|i| (100 + i, format!("S-{i}")))).unwrap())
}

fn empty_state(n: u32) -> EventState {
    let loaded = LoadedEvent {
        epoch: 1,
        seq: 0,
        holds: vec![],
        objects: vec![],
        idempotency: vec![],
    };
    EventState::restore(EVENT, chart(n), loaded).unwrap()
}

fn tok(n: u8) -> TokenHash {
    TokenHash([n; 32])
}

fn labels(idxs: &[u32]) -> Vec<ObjectRef> {
    idxs.iter()
        .map(|i| ObjectRef::Label(format!("S-{i}").into()))
        .collect()
}

fn create(t: u8, max: Option<u32>) -> Command {
    Command::CreateHold {
        token: tok(t),
        ttl_secs: 900,
        max_objects: max,
    }
}

fn hold(t: u8, idxs: &[u32]) -> Command {
    Command::Hold {
        token: tok(t),
        objects: labels(idxs),
    }
}

fn release(t: u8, idxs: &[u32]) -> Command {
    Command::Release {
        token: tok(t),
        objects: labels(idxs),
    }
}

fn book(t: Option<u8>, idxs: &[u32], order: &str) -> Command {
    Command::Book {
        token: t.map(tok),
        objects: labels(idxs),
        order_ref: order.into(),
        details: vec![],
    }
}

fn change(idxs: &[u32], code: u8) -> Command {
    Command::ChangeStatus {
        objects: labels(idxs),
        status: Status::from_code(code).unwrap(),
    }
}

fn detail(idx: u32, ticket_type: &str) -> BookDetail {
    BookDetail {
        object: ObjectRef::Idx(idx),
        ticket_type: Some(ticket_type.into()),
        extra: None,
    }
}

fn req(command: Command) -> Request {
    Request {
        actor: "sk_test".into(),
        authority: Authority::Secret,
        command,
        idempotency: None,
    }
}

fn keyed(command: Command, key: &str, fingerprint: u8) -> Request {
    Request {
        idempotency: Some(Idempotency {
            key: key.into(),
            fingerprint: [fingerprint; 32],
            checked_at: T0,
        }),
        ..req(command)
    }
}

fn run(s: &mut EventState, c: Command, now: Timestamp) -> Result<Outcome, Failure> {
    s.apply(&req(c), now).unwrap()
}

fn not(reason: ObjectReason, idxs: &[u32]) -> Vec<ObjectFailure> {
    idxs.iter()
        .map(|&idx| ObjectFailure { idx, reason })
        .collect()
}

type Fingerprint = (
    u64,
    Vec<(Option<Status>, Option<HoldId>, Option<Booking>)>,
    Vec<(HoldId, Timestamp, Option<u32>, Vec<Idx>)>,
);

/// Everything observable about the state, in a comparable order.
fn fingerprint(s: &EventState) -> Fingerprint {
    let objects = (0..s.chart().len() as u32)
        .map(|i| (s.status(i), s.holder(i), s.booking(i).cloned()))
        .collect();
    let mut holds: Vec<_> = s
        .holds()
        .map(|h| {
            let mut o = h.objects.to_vec();
            o.sort_unstable();
            (h.id, h.expires_at, h.max_objects, o)
        })
        .collect();
    holds.sort();
    (s.seq(), objects, holds)
}

/// INV-1 and INV-2 at time `now`: every object has one valid status; it is
/// held exactly when it is listed under exactly one hold, which is unexpired
/// and the one it names; it carries booking data exactly when booked.
fn check_invariants(s: &EventState, now: Timestamp) {
    for h in s.holds() {
        assert!(h.expires_at > now, "hold {:?} outlived its expiry", h.id);
        assert!(h.max_objects.is_none_or(|m| h.objects.len() <= m as usize));
    }
    for i in 0..s.chart().len() as u32 {
        let listed: Vec<_> = s.holds().filter(|h| h.objects.contains(&i)).collect();
        let status = s.status(i).unwrap();
        assert_eq!(
            s.booking(i).is_some(),
            status == Status::BOOKED,
            "object {i}"
        );
        if status == Status::HELD {
            assert_eq!(listed.len(), 1, "object {i} under {} holds", listed.len());
            assert_eq!(s.holder(i), Some(listed[0].id));
        } else {
            assert!(listed.is_empty() && s.holder(i).is_none(), "object {i}");
        }
    }
}

#[test]
fn hold_is_all_or_nothing_with_per_object_reasons() {
    let mut s = empty_state(5);
    run(&mut s, create(1, None), T0).unwrap();
    run(&mut s, create(2, None), T0).unwrap();
    run(&mut s, hold(1, &[1, 3]), T0).unwrap();
    let before = fingerprint(&s);

    let refused = run(&mut s, hold(2, &[0, 1, 2, 3]), T0);
    assert_eq!(
        refused,
        Err(Failure::ObjectNotFree(not(ObjectReason::NotFree, &[1, 3])))
    );
    assert_eq!(fingerprint(&s), before);

    let mut objects = labels(&[0]);
    objects.push(ObjectRef::Label("nope".into()));
    objects.push(ObjectRef::Idx(5));
    assert_eq!(
        run(
            &mut s,
            Command::Hold {
                token: tok(2),
                objects
            },
            T0
        ),
        Err(Failure::NotFound(Missing::Objects(vec![
            ObjectRef::Label("nope".into()),
            ObjectRef::Idx(5)
        ])))
    );
    assert_eq!(fingerprint(&s), before);
}

#[test]
fn reholding_own_objects_is_a_noop() {
    let mut s = empty_state(4);
    run(&mut s, create(1, Some(3)), T0).unwrap();
    run(&mut s, hold(1, &[1]), T0).unwrap();
    let batch = s.take_batch().unwrap().unwrap();
    assert_eq!(batch.seq, 2);

    let again = run(&mut s, hold(1, &[1, 1]), T0).unwrap();
    assert!(matches!(again, Outcome::Held { seq: 2, ref objects, .. } if objects == &[1]));
    assert_eq!(s.take_batch().unwrap(), None, "a no-op writes nothing");

    // Partly new: only the new object is logged, and the limit counts it once.
    let more = run(&mut s, hold(1, &[2, 1]), T0).unwrap();
    assert!(matches!(more, Outcome::Held { seq: 3, ref objects, .. } if objects == &[1, 2]));
    let batch = s.take_batch().unwrap().unwrap();
    assert_eq!(batch.log.len(), 1);
    assert_eq!(batch.log[0].oids, vec![102]);
}

#[test]
fn token_limit_unknown_and_expired_tokens() {
    let mut s = empty_state(5);
    run(&mut s, create(1, Some(2)), T0).unwrap();
    run(&mut s, hold(1, &[0]), T0).unwrap();
    assert_eq!(
        run(&mut s, hold(1, &[1, 2]), T0),
        Err(Failure::HoldLimitExceeded {
            limit: 2,
            requested: 3
        })
    );
    assert_eq!(
        run(&mut s, hold(9, &[1]), T0),
        Err(Failure::NotFound(Missing::HoldToken))
    );

    // Expiry is checked at processing time; at the deadline itself the hold
    // has expired and is gone, its objects free.
    let deadline = T0.plus_secs(900);
    assert!(run(&mut s, hold(1, &[1]), Timestamp(deadline.0 - 1)).is_ok());
    for c in [
        hold(1, &[2]),
        release(1, &[0]),
        Command::ExtendHold {
            token: tok(1),
            ttl_secs: 600,
        },
        book(Some(1), &[], "o"),
    ] {
        assert_eq!(
            run(&mut s, c, deadline),
            Err(Failure::NotFound(Missing::HoldToken))
        );
    }
    assert_eq!(s.status(0), Some(Status::FREE));
    assert_eq!(s.status(1), Some(Status::FREE));
}

#[test]
fn release_and_extend() {
    let mut s = empty_state(5);
    run(&mut s, create(1, None), T0).unwrap();
    run(&mut s, create(2, None), T0).unwrap();
    run(&mut s, hold(1, &[0, 1, 2]), T0).unwrap();
    run(&mut s, hold(2, &[3]), T0).unwrap();

    let before = fingerprint(&s);
    assert_eq!(
        run(&mut s, release(1, &[1, 3, 4]), T0),
        Err(Failure::ObjectNotHeldByToken(not(
            ObjectReason::NotHeldByToken,
            &[3, 4]
        )))
    );
    assert_eq!(fingerprint(&s), before);

    run(&mut s, release(1, &[1]), T0).unwrap();
    assert_eq!(s.status(1), Some(Status::FREE));
    let all = run(&mut s, release(1, &[]), T0).unwrap();
    assert!(matches!(all, Outcome::Released { ref objects, .. } if objects == &[0, 2]));
    assert_eq!(s.status(3), Some(Status::HELD), "other tokens untouched");
    // The token outlives its objects.
    run(&mut s, hold(1, &[4]), T0).unwrap();

    let later = T0.plus_secs(800);
    let extended = run(
        &mut s,
        Command::ExtendHold {
            token: tok(1),
            ttl_secs: 600,
        },
        later,
    )
    .unwrap();
    assert!(
        matches!(extended, Outcome::Extended { expires_at, .. } if expires_at == later.plus_secs(600))
    );
    assert!(run(&mut s, hold(1, &[1]), T0.plus_secs(1000)).is_ok());
    assert_eq!(
        run(
            &mut s,
            Command::ExtendHold {
                token: tok(1),
                ttl_secs: 7_201
            },
            later
        ),
        Err(Failure::InvalidRequest(InvalidReason::TtlOutOfRange))
    );
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

#[test]
fn book_with_a_token_and_directly() {
    let mut s = empty_state(8);
    run(&mut s, create(1, None), T0).unwrap();
    run(&mut s, create(2, None), T0).unwrap();
    run(&mut s, hold(1, &[0, 1, 2]), T0).unwrap();
    run(&mut s, hold(2, &[3]), T0).unwrap();
    s.take_batch().unwrap();
    let before = fingerprint(&s);

    let refusals = [
        (
            book(Some(1), &[2, 3, 4], "A"),
            Failure::ObjectNotHeldByToken(not(ObjectReason::NotHeldByToken, &[3, 4])),
        ),
        (
            book(None, &[3, 5], "A"),
            Failure::ObjectNotFree(not(ObjectReason::NotFree, &[3])),
        ),
        (
            Command::Book {
                token: Some(tok(1)),
                objects: labels(&[0]),
                order_ref: "A".into(),
                details: vec![detail(1, "adult")],
            },
            Failure::InvalidRequest(InvalidReason::DetailMismatch),
        ),
        (
            book(Some(1), &[0], &"x".repeat(129)),
            Failure::InvalidRequest(InvalidReason::OrderRefLength),
        ),
        (
            book(None, &[], "A"),
            Failure::InvalidRequest(InvalidReason::NoObjects),
        ),
    ];
    for (command, failure) in refusals {
        assert_eq!(run(&mut s, command, T0), Err(failure));
    }
    let publishable = Request {
        authority: Authority::Publishable,
        ..req(book(Some(1), &[0], "A"))
    };
    assert_eq!(s.apply(&publishable, T0).unwrap(), Err(Failure::Forbidden));
    assert_eq!(fingerprint(&s), before, "a refusal changed state");
    assert_eq!(s.take_batch().unwrap(), None);

    // Some of the token's objects, with a ticket type and `extra`.
    let mut d = detail(0, "adult");
    d.extra = Some(r#"{"seat_note":"aisle"}"#.into());
    let named = Command::Book {
        token: Some(tok(1)),
        objects: labels(&[0]),
        order_ref: "A".into(),
        details: vec![d],
    };
    let reply = run(&mut s, named, T0).unwrap();
    assert_eq!(
        reply,
        Outcome::Booked {
            seq: 5,
            objects: vec![0]
        }
    );
    let booking = s.booking(0).unwrap();
    assert_eq!(
        (&*booking.order_ref, booking.ticket_type.as_deref()),
        ("A", Some("adult"))
    );
    assert!(s.hold(&tok(1)).is_some(), "the token still holds 1 and 2");

    // All the token's remaining objects: the token is deleted.
    let reply = run(&mut s, book(Some(1), &[], "B"), T0).unwrap();
    assert_eq!(
        reply,
        Outcome::Booked {
            seq: 6,
            objects: vec![1, 2]
        }
    );
    assert!(s.hold(&tok(1)).is_none());
    assert_eq!(
        run(&mut s, hold(1, &[5]), T0),
        Err(Failure::NotFound(Missing::HoldToken))
    );

    let reply = run(&mut s, book(None, &[5, 6], "C"), T0).unwrap();
    assert_eq!(
        reply,
        Outcome::Booked {
            seq: 7,
            objects: vec![5, 6]
        }
    );

    let batch = s.take_batch().unwrap().unwrap();
    assert_eq!(batch.holds_deleted, vec![HoldId(1)]);
    let rows: Vec<_> = batch
        .objects_set
        .iter()
        .map(|r| (r.oid, r.status, r.booking.as_ref().map(|b| &*b.order_ref)))
        .collect();
    let booked = |oid, order| (oid, Status::BOOKED, Some(order));
    assert_eq!(
        rows,
        vec![
            booked(100, "A"),
            booked(101, "B"),
            booked(102, "B"),
            booked(105, "C"),
            booked(106, "C")
        ]
    );
    check_invariants(&s, T0);
}

/// U+0000 cannot be stored in PostgreSQL text: such values are refused when
/// applied, before they can reach a batch and fail its commit.
#[test]
fn text_with_a_nul_is_refused_before_it_reaches_a_batch() {
    let mut s = empty_state(4);
    let invalid = |r| Err(Failure::InvalidRequest(r));
    assert_eq!(
        run(&mut s, book(None, &[0], "A\0B"), T0),
        invalid(InvalidReason::OrderRefLength)
    );
    let nul_ticket = Command::Book {
        token: None,
        objects: labels(&[0]),
        order_ref: "77120".into(),
        details: vec![detail(0, "adult\0")],
    };
    assert_eq!(
        run(&mut s, nul_ticket, T0),
        invalid(InvalidReason::TicketTypeLength)
    );
    let keyed_nul = keyed(book(None, &[0], "77120"), "k\0", 1);
    assert_eq!(
        s.apply(&keyed_nul, T0).unwrap(),
        invalid(InvalidReason::IdempotencyKeyLength)
    );
    assert_eq!(s.take_batch().unwrap(), None);
}

#[test]
fn release_booked_and_change_status() {
    let mut s = empty_state(8);
    run(&mut s, book(None, &[0, 1], "A"), T0).unwrap();
    run(&mut s, create(1, None), T0).unwrap();
    run(&mut s, hold(1, &[2]), T0).unwrap();
    let release_booked = |idxs: &[u32]| Command::ReleaseBooked {
        objects: labels(idxs),
    };

    let before = fingerprint(&s);
    for (command, failure) in [
        (
            release_booked(&[0, 2, 3]),
            Failure::ObjectStatusConflict(not(ObjectReason::NotBooked, &[2, 3])),
        ),
        (
            change(&[0], 1),
            Failure::InvalidRequest(InvalidReason::BadStatus),
        ),
        (
            change(&[1, 2, 4], 0),
            Failure::ObjectStatusConflict(not(ObjectReason::NotBlocked, &[1, 2])),
        ),
    ] {
        assert_eq!(run(&mut s, command, T0), Err(failure));
    }
    // INV-3: a publishable key never touches a booked object.
    for command in [release_booked(&[0]), change(&[0], 0), change(&[5], 3)] {
        let request = Request {
            authority: Authority::Publishable,
            ..req(command)
        };
        assert_eq!(s.apply(&request, T0).unwrap(), Err(Failure::Forbidden));
    }
    assert_eq!(fingerprint(&s), before, "a refusal changed state");

    let reply = run(&mut s, release_booked(&[0]), T0).unwrap();
    assert_eq!(
        reply,
        Outcome::Released {
            seq: 4,
            objects: vec![0]
        }
    );
    assert_eq!((s.status(0), s.booking(0)), (Some(Status::FREE), None));

    run(&mut s, change(&[3, 4], 3), T0).unwrap();
    let again = run(&mut s, change(&[4, 3], 3), T0).unwrap();
    assert_eq!(
        again,
        Outcome::StatusChanged {
            seq: 5,
            objects: vec![3, 4]
        }
    );
    run(&mut s, change(&[5], 15), T0).unwrap();
    assert_eq!(s.status(5), Status::from_code(15));
    // Blocked → custom is not a listed transition: go through free.
    assert_eq!(
        run(&mut s, change(&[4], 9), T0),
        Err(Failure::ObjectNotFree(not(ObjectReason::NotFree, &[4])))
    );
    assert_eq!(
        run(&mut s, change(&[3, 5, 6], 0), T0).unwrap(),
        Outcome::StatusChanged {
            seq: 7,
            objects: vec![3, 5, 6]
        }
    );
    assert_eq!(s.status(3), Some(Status::FREE));
    assert_eq!(s.status(4), Some(Status::BLOCKED));
    assert_eq!(s.status(5), Some(Status::FREE));

    let batch = s.take_batch().unwrap().unwrap();
    assert_eq!(batch.objects_freed, vec![100, 103, 105]);
    let ops: Vec<_> = batch.log.iter().map(|r| (r.op, r.status)).collect();
    assert_eq!(
        ops[3..],
        [
            (Op::Release, None),
            (Op::ChangeStatus, Some(Status::BLOCKED)),
            (Op::ChangeStatus, Status::from_code(15)),
            (Op::ChangeStatus, Some(Status::FREE))
        ]
    );
}

#[test]
fn holds_expire_at_their_deadline_in_expiry_order() {
    let mut s = empty_state(8);
    for (t, ttl) in [(1, 900), (2, 600), (3, 300), (4, 900), (5, 900)] {
        let c = Command::CreateHold {
            token: tok(t),
            ttl_secs: ttl,
            max_objects: None,
        };
        run(&mut s, c, T0).unwrap();
        run(&mut s, hold(t, &[u32::from(t)]), T0).unwrap();
    }
    s.take_batch().unwrap();
    // Token 3 moves from T0+300 to T0+1000; token 4 releases everything but
    // still expires; token 5 is fully booked and never fires.
    let extend = Command::ExtendHold {
        token: tok(3),
        ttl_secs: 900,
    };
    run(&mut s, extend, T0.plus_secs(100)).unwrap();
    run(&mut s, release(4, &[]), T0).unwrap();
    run(&mut s, book(Some(5), &[], "A"), T0).unwrap();
    s.take_batch().unwrap();
    let seq = s.seq();

    assert_eq!(s.next_expiry(), Some(T0.plus_secs(600)));
    assert_eq!(s.expire_due(Timestamp(T0.plus_secs(600).0 - 1)).unwrap(), 0);
    assert_eq!(s.expire_due(T0.plus_secs(900)).unwrap(), 3);
    check_invariants(&s, T0.plus_secs(900));
    assert_eq!(s.status(2), Some(Status::FREE));
    assert_eq!(
        s.status(3),
        Some(Status::HELD),
        "the extended hold is still live"
    );
    assert_eq!(s.status(5), Some(Status::BOOKED));

    let batch = s.take_batch().unwrap().unwrap();
    let log: Vec<_> = batch
        .log
        .iter()
        .map(|r| (r.seq, r.op, r.hold_id, r.oids.clone()))
        .collect();
    // A hold's id is the `seq` of its `CreateHold`: 1, 3, 5, 7, 9.
    let (h1, h2, h4) = (HoldId(1), HoldId(3), HoldId(7));
    assert_eq!(
        log,
        vec![
            (seq + 1, Op::Expire, Some(h2), vec![102]),
            (seq + 2, Op::Expire, Some(h1), vec![101]),
            (seq + 3, Op::Expire, Some(h4), vec![]),
        ]
    );
    assert_eq!(batch.holds_deleted, vec![h1, h2, h4]);

    assert_eq!(s.next_expiry(), Some(T0.plus_secs(1000)));
    assert_eq!(s.expire_due(Timestamp(u64::MAX)).unwrap(), 1);
    assert_eq!(s.next_expiry(), None);
    assert_eq!(s.holds().count(), 0);
}

#[test]
fn a_command_at_the_expiry_instant_sees_the_hold_whole_or_gone() {
    let mut s = empty_state(4);
    run(&mut s, create(1, None), T0).unwrap();
    run(&mut s, create(2, None), T0).unwrap();
    run(&mut s, hold(1, &[0, 1]), T0).unwrap();
    run(&mut s, hold(2, &[2, 3]), T0).unwrap();
    let deadline = T0.plus_secs(900);

    // One instant before: the booking takes every object and nothing expires.
    let last = Timestamp(deadline.0 - 1);
    assert!(run(&mut s, book(Some(1), &[], "A"), last).is_ok());
    // At the instant: the expiry comes first, so the booking finds no token.
    assert_eq!(
        run(&mut s, book(Some(2), &[], "B"), deadline),
        Err(Failure::NotFound(Missing::HoldToken))
    );
    let statuses: Vec<_> = (0..4).map(|i| s.status(i).unwrap()).collect();
    assert_eq!(
        statuses,
        [Status::BOOKED, Status::BOOKED, Status::FREE, Status::FREE]
    );
    let log = s.take_batch().unwrap().unwrap().log;
    let ops: Vec<_> = log.iter().map(|r| r.op).collect();
    assert_eq!(ops[4..], [Op::Book, Op::Expire]);
}

#[test]
fn idempotency_keys_replay_the_original_reply() {
    let mut s = empty_state(4);
    let created = s.apply(&keyed(create(1, None), "k1", 1), T0).unwrap();
    // A refusal is recorded too.
    let refused = s.apply(&keyed(hold(7, &[0]), "k2", 2), T0).unwrap();
    assert_eq!(refused, Err(Failure::NotFound(Missing::HoldToken)));
    run(&mut s, create(7, None), T0).unwrap();
    let before = fingerprint(&s);

    let later = T0.plus_secs(60);
    assert_eq!(
        s.apply(&keyed(create(1, None), "k1", 1), later).unwrap(),
        created
    );
    assert_eq!(
        s.apply(&keyed(hold(7, &[0]), "k2", 2), later).unwrap(),
        refused
    );
    assert_eq!(
        s.apply(&keyed(hold(7, &[1]), "k1", 3), later).unwrap(),
        Err(Failure::IdempotencyMismatch)
    );
    assert_eq!(fingerprint(&s), before, "a replay changed state");

    let batch = s.take_batch().unwrap().unwrap();
    let records: Vec<_> = batch
        .idempotency
        .iter()
        .map(|r| (&*r.key, r.reply.clone()))
        .collect();
    assert_eq!(records, vec![("k1", created.clone()), ("k2", refused)]);

    // After 24 hours the key no longer matches: the command runs again.
    let day = Timestamp(T0.0 + limits::IDEMPOTENCY_TTL_MS);
    let again = s.apply(&keyed(create(1, None), "k1", 9), day).unwrap();
    // Both holds expired first (seq 3 and 4).
    assert!(
        matches!(again, Ok(Outcome::HoldCreated { seq: 5, .. })),
        "{again:?}"
    );
}

#[test]
fn the_idempotency_window_is_bounded() {
    let mut s = empty_state(1);
    let first = s.apply(&keyed(create(1, None), "k0", 0), T0).unwrap();
    for i in 1..limits::IDEMPOTENCY_WINDOW {
        let refused = s.apply(&keyed(hold(9, &[0]), &format!("k{i}"), 0), T0);
        assert!(refused.unwrap().is_err());
    }
    // Full of records younger than the minimum retention: new keys wait.
    let fresh = keyed(hold(9, &[0]), "fresh", 0);
    assert_eq!(s.apply(&fresh, T0).unwrap(), Err(Failure::Overloaded));
    assert_eq!(
        s.apply(&keyed(create(1, None), "k0", 0), T0).unwrap(),
        first
    );

    // Retention counts from the caller's store lookup: a command looked up at
    // T0 that waited in the mailbox past the minute still finds no room, so a
    // record its lookup could not see is never dropped under it.
    let later = Timestamp(T0.0 + limits::IDEMPOTENCY_MIN_RETENTION_MS);
    assert_eq!(s.apply(&fresh, later).unwrap(), Err(Failure::Overloaded));
    // A fresh lookup a minute after the oldest record makes room.
    let mut looked_up = fresh.clone();
    if let Some(key) = &mut looked_up.idempotency {
        key.checked_at = later;
    }
    assert!(matches!(
        s.apply(&looked_up, later).unwrap(),
        Err(Failure::NotFound(Missing::HoldToken))
    ));
    // k0 was dropped from memory, so it runs again; the caller's store lookup
    // covers records the window has dropped (08 §6.1).
    let mut k0 = keyed(create(1, None), "k0", 0);
    if let Some(key) = &mut k0.idempotency {
        key.checked_at = later;
    }
    assert_eq!(
        s.apply(&k0, later).unwrap(),
        Err(Failure::InvalidRequest(InvalidReason::TokenAlreadyExists))
    );
}

/// Random sequences of every command, from both authorities, some with
/// idempotency keys, with the clock moving (sometimes backwards). After every
/// step: INV-1 and INV-2 including expiry; a refusal or replay changed
/// nothing; `seq` moved by at most one; a publishable command left booked
/// objects alone (INV-3); a key replays its first reply or is a mismatch.
/// Batches commit at random points and the store rejects any `seq` gap
/// (INV-5). At the end: replaying the committed log rebuilds memory (INV-6),
/// INV-10 holds over that log, and reloading from the store reproduces memory.
#[tokio::test]
async fn random_command_sequences_keep_invariants() {
    const SEATS: u32 = 12;
    for seed in 1..=150u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let store = MemoryStore::new();
        store.create_event(EVENT, 1);
        let mut s = empty_state(SEATS);
        let mut now = T0;
        let mut keys = std::collections::HashMap::new();
        for _ in 0..400 {
            let t = rng.below(6) as u8;
            let objects = |rng: &mut Rng, min: u64| -> Vec<ObjectRef> {
                (0..min + rng.below(4))
                    .map(|_| match rng.below(20) {
                        0 => ObjectRef::Label("missing".into()),
                        _ => ObjectRef::Idx(rng.below(u64::from(SEATS)) as u32),
                    })
                    .collect()
            };
            let command = match rng.below(15) {
                0 => Command::CreateHold {
                    token: tok(t),
                    ttl_secs: [30, 60, 300, 900][rng.below(4) as usize],
                    max_objects: [None, Some(0), Some(2), Some(4)][rng.below(4) as usize],
                },
                1..=3 => Command::Hold {
                    token: tok(t),
                    objects: objects(&mut rng, 1),
                },
                4 => Command::Release {
                    token: tok(t),
                    objects: objects(&mut rng, 0),
                },
                5 => Command::ExtendHold {
                    token: tok(t),
                    ttl_secs: 60 + rng.below(600) as u32,
                },
                6 | 7 => {
                    let objects = objects(&mut rng, 0);
                    let details = (rng.below(3) == 0)
                        .then(|| detail(rng.below(u64::from(SEATS)) as u32, "adult"))
                        .into_iter()
                        .collect();
                    Command::Book {
                        token: (rng.below(3) > 0).then(|| tok(t)),
                        objects,
                        order_ref: ["o1", "o2", ""][rng.below(3) as usize].into(),
                        details,
                    }
                }
                8 => Command::ReleaseBooked {
                    objects: objects(&mut rng, 1),
                },
                9 => Command::ChangeStatus {
                    objects: objects(&mut rng, 1),
                    status: Status::from_code([0, 0, 3, 3, 5, 2][rng.below(6) as usize]).unwrap(),
                },
                10 | 11 => {
                    now = Timestamp(now.0 + rng.below(120_000));
                    continue;
                }
                12 => {
                    now = Timestamp(now.0.saturating_sub(rng.below(5_000))); // a jump back
                    continue;
                }
                _ => {
                    if let Some(batch) = s.take_batch().unwrap() {
                        store.commit_batch(batch).await.unwrap();
                    }
                    continue;
                }
            };
            let key = (rng.below(3) == 0).then(|| (rng.below(8) as u8, rng.below(4) == 0));
            let request = Request {
                actor: "a".into(),
                authority: [Authority::Publishable, Authority::Secret][rng.below(2) as usize],
                command,
                idempotency: key.map(|(k, odd)| Idempotency {
                    key: format!("key-{k}").into(),
                    fingerprint: [u8::from(odd); 32],
                    checked_at: now,
                }),
            };

            s.expire_due(now).unwrap();
            check_invariants(&s, now);
            let before = fingerprint(&s);
            let reply = s.apply(&request, now).unwrap();
            let after = fingerprint(&s);
            let replayed = match key {
                Some((k, odd)) => match keys.get(&k) {
                    Some((first_odd, first)) => {
                        let expected = if *first_odd == odd {
                            first
                        } else {
                            &Err(Failure::IdempotencyMismatch)
                        };
                        assert_eq!(&reply, expected, "seed {seed}");
                        true
                    }
                    None => {
                        keys.insert(k, (odd, reply.clone()));
                        false
                    }
                },
                None => false,
            };
            if replayed || reply.is_err() || after.0 == before.0 {
                assert_eq!(after, before, "seed {seed}: change without a seq");
            } else {
                assert_eq!(after.0, before.0 + 1, "seed {seed}");
            }
            if request.authority == Authority::Publishable {
                for (b, a) in before.1.iter().zip(&after.1) {
                    if b.0 == Some(Status::BOOKED) {
                        assert_eq!(a, b, "seed {seed}: INV-3");
                    }
                }
            }
            check_invariants(&s, now);
        }
        if let Some(batch) = s.take_batch().unwrap() {
            store.commit_batch(batch).await.unwrap();
        }

        // INV-6 from the log alone, as the reconciler does (13 §3.3): every
        // object and hold rebuilt from `event_log` matches memory. Along the
        // way, INV-10: no object is booked twice without a release between.
        let mut objects = std::collections::HashMap::new();
        let mut holds = std::collections::BTreeMap::new();
        for r in store.committed().iter().flat_map(|b: &Batch| &b.log) {
            let hold = r.hold_id.and_then(|id| holds.get_mut(&id).map(|h| (id, h)));
            match (r.op, hold) {
                (Op::CreateHold, _) => {
                    holds.insert(r.hold_id.unwrap(), (r.expires_at.unwrap(), vec![]));
                }
                (Op::ExtendHold, Some((_, h))) => h.0 = r.expires_at.unwrap(),
                (Op::Hold, Some((_, h))) => h.1.extend(&r.oids),
                (Op::Release | Op::Book, Some((id, h))) => {
                    h.1.retain(|o| !r.oids.contains(o));
                    if h.1.is_empty() && r.op == Op::Book {
                        holds.remove(&id);
                    }
                }
                (Op::Expire, Some((id, _))) => {
                    holds.remove(&id);
                }
                (Op::Release | Op::Book | Op::ChangeStatus, None) => {}
                (op, _) => panic!("seed {seed}: {op:?} for an unknown hold"),
            }
            for &oid in &r.oids {
                let before = objects.get(&oid).map(|o: &(Status, _, _)| o.0);
                let after = match r.op {
                    Op::Hold => (Status::HELD, r.hold_id, None),
                    Op::Book => {
                        assert_ne!(before, Some(Status::BOOKED), "seed {seed}: INV-10");
                        let detail = r.details.iter().find(|d| d.0 == oid);
                        let booking = Booking {
                            order_ref: r.order_ref.clone().unwrap(),
                            ticket_type: detail.and_then(|d| d.1.clone()),
                            extra: detail.and_then(|d| d.2.clone()),
                        };
                        (Status::BOOKED, None, Some(booking))
                    }
                    Op::ChangeStatus => (r.status.unwrap(), None, None),
                    _ => (Status::FREE, None, None),
                };
                objects.insert(oid, after);
            }
        }
        for i in 0..SEATS {
            let replayed = objects.remove(&(100 + i));
            let replayed = replayed.unwrap_or((Status::FREE, None, None));
            let live = (s.status(i).unwrap(), s.holder(i), s.booking(i).cloned());
            assert_eq!(replayed, live, "seed {seed}: object {i}");
        }
        let replayed: Vec<_> = holds
            .into_iter()
            .map(|(id, (at, mut oids))| {
                oids.sort_unstable();
                (id, at, oids)
            })
            .collect();
        let live: Vec<_> = fingerprint(&s)
            .2
            .into_iter()
            .map(|(id, at, _, idxs)| (id, at, idxs.iter().map(|i| 100 + i).collect()))
            .collect();
        assert_eq!(replayed, live, "seed {seed}: holds");

        let loaded = store.load_event(EVENT).await.unwrap();
        let reloaded = EventState::restore(EVENT, chart(SEATS), loaded).unwrap();
        assert_eq!(fingerprint(&reloaded), fingerprint(&s), "seed {seed}");
    }
}

// ---- The actor ----

struct Rig {
    store: Arc<MemoryStore>,
    clock: ManualClock,
    handle: cavea_engine::ActorHandle,
    task: tokio::task::JoinHandle<Result<(), ActorError>>,
}

fn rig(config: ActorConfig) -> Rig {
    let store = Arc::new(MemoryStore::new());
    store.create_event(EVENT, 1);
    start(empty_state(20), store, T0, config)
}

fn start(state: EventState, store: Arc<MemoryStore>, now: Timestamp, config: ActorConfig) -> Rig {
    let clock = ManualClock::new(now);
    let (handle, fut) = actor(state, store.clone(), clock.clone(), config);
    Rig {
        store,
        clock,
        handle,
        task: tokio::spawn(fut),
    }
}

async fn reload(store: &Arc<MemoryStore>, now: Timestamp) -> Rig {
    let loaded = store.load_event(EVENT).await.unwrap();
    let state = EventState::restore(EVENT, chart(20), loaded).unwrap();
    start(state, store.clone(), now, ActorConfig::default())
}

/// The op of every committed log row, in order.
fn committed_ops(store: &MemoryStore) -> Vec<(u64, Op)> {
    let batches = store.committed();
    batches
        .iter()
        .flat_map(|b| &b.log)
        .map(|r| (r.seq, r.op))
        .collect()
}

/// Let every runnable task run until it blocks.
async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

fn gate(store: &MemoryStore) -> oneshot::Sender<()> {
    let (open, wait) = oneshot::channel();
    store.push_fault(Fault::Wait(wait));
    open
}

#[tokio::test]
async fn replies_wait_for_the_commit() {
    let r = rig(ActorConfig::default());
    let open = gate(&r.store);
    let mut reply = tokio::spawn(r.handle.submit(req(create(1, None))).unwrap());
    settle().await;
    assert!(!reply.is_finished(), "replied before the commit completed");
    assert!(r.store.committed().is_empty());

    open.send(()).unwrap();
    let outcome = (&mut reply).await.unwrap().unwrap();
    assert!(matches!(outcome, Outcome::HoldCreated { seq: 1, .. }));
    assert_eq!(r.store.committed().len(), 1);
}

#[tokio::test]
async fn commands_during_a_slow_commit_form_the_next_batches_in_order() {
    let r = rig(ActorConfig {
        mailbox: 100,
        max_batch: 3,
    });
    let (open1, open2) = (gate(&r.store), gate(&r.store));
    let mut replies = vec![r.handle.submit(req(create(0, None))).unwrap()];
    settle().await;
    for t in 1..8 {
        replies.push(r.handle.submit(req(create(t, None))).unwrap());
    }
    // A refusal rides along without taking a seq.
    replies.push(r.handle.submit(req(hold(99, &[0]))).unwrap());
    settle().await;
    open1.send(()).unwrap();
    settle().await;
    open2.send(()).unwrap();

    let mut seqs = vec![];
    for reply in replies {
        match reply.await {
            Ok(Outcome::HoldCreated { seq, .. }) => seqs.push(seq),
            other => assert_eq!(other, Err(Failure::NotFound(Missing::HoldToken))),
        }
    }
    assert_eq!(seqs, (1..=8).collect::<Vec<_>>());
    let sizes: Vec<_> = r.store.committed().iter().map(|b| b.log.len()).collect();
    assert_eq!(sizes, vec![1, 3, 3, 1]);
}

#[tokio::test]
async fn failed_commit_fails_that_batch_and_everything_after_it() {
    let r = rig(ActorConfig {
        mailbox: 10,
        max_batch: 1,
    });
    let open = gate(&r.store);
    let first = r.handle.submit(req(create(1, None))).unwrap();
    settle().await;
    let second = r.handle.submit(req(create(2, None))).unwrap(); // applied, pending
    settle().await;
    let third = r.handle.submit(req(create(3, None))).unwrap(); // still in the mailbox
    settle().await;

    // Another node takes the event while the first commit is in flight.
    r.store.set_owner_epoch(EVENT, 2);
    open.send(()).unwrap();
    for reply in [first, second, third] {
        assert_eq!(reply.await, Err(Failure::Unavailable));
    }
    assert_eq!(
        r.task.await.unwrap(),
        Err(ActorError::Store(StoreError::Fenced))
    );
    assert!(r.store.committed().is_empty());
    assert_eq!(
        r.handle.send(req(create(4, None))).await,
        Err(Failure::Unavailable)
    );
}

#[tokio::test]
async fn injected_failure_writes_nothing_and_stops_the_actor() {
    let r = rig(ActorConfig::default());
    r.store.push_fault(Fault::Fail);
    assert_eq!(
        r.handle.send(req(create(1, None))).await,
        Err(Failure::Unavailable)
    );
    assert!(matches!(
        r.task.await.unwrap(),
        Err(ActorError::Store(StoreError::Unavailable(_)))
    ));
    assert_eq!(r.store.load_event(EVENT).await.unwrap().seq, 0);
}

#[tokio::test]
async fn a_lost_reply_is_still_durable_after_reload() {
    let r = rig(ActorConfig::default());
    assert!(r.handle.send(req(create(1, None))).await.is_ok());
    r.store.push_fault(Fault::LoseReply);
    assert_eq!(
        r.handle.send(req(hold(1, &[4]))).await,
        Err(Failure::Unavailable)
    );
    assert!(r.task.await.unwrap().is_err());

    let loaded = r.store.load_event(EVENT).await.unwrap();
    let reloaded = EventState::restore(EVENT, chart(20), loaded).unwrap();
    assert_eq!(reloaded.seq(), 2);
    assert_eq!(reloaded.status(4), Some(Status::HELD));

    let (handle, fut) = actor(
        reloaded,
        r.store.clone(),
        ManualClock::new(T0),
        ActorConfig::default(),
    );
    let task = tokio::spawn(fut);
    let again = handle.send(req(hold(1, &[4, 5]))).await.unwrap();
    assert!(matches!(again, Outcome::Held { seq: 3, .. }));
    drop(handle);
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn a_full_mailbox_is_refused_as_overloaded() {
    let r = rig(ActorConfig {
        mailbox: 1,
        max_batch: 1,
    });
    let open = gate(&r.store);
    let mut accepted = vec![];
    for t in 0..3 {
        // In flight, then the full next batch, then the one mailbox slot.
        accepted.push(r.handle.submit(req(create(t, None))).unwrap());
        settle().await;
    }
    assert_eq!(
        r.handle.submit(req(create(9, None))).err(),
        Some(Failure::Overloaded)
    );
    open.send(()).unwrap();
    for reply in accepted {
        assert!(reply.await.is_ok());
    }
}

#[tokio::test]
async fn a_rejected_commit_leaves_the_store_untouched() {
    let store = MemoryStore::new();
    store.create_event(EVENT, 1);
    let mut s = empty_state(4);
    run(&mut s, create(1, None), T0).unwrap();
    let mut batch = s.take_batch().unwrap().unwrap();
    // Valid seq and a new hold, but an extension of a hold the store lacks.
    batch.holds_extended.push((HoldId(99), T0));
    let before = store.load_event(EVENT).await.unwrap();
    assert!(matches!(
        store.commit_batch(batch).await,
        Err(StoreError::Inconsistent(_))
    ));
    assert_eq!(store.load_event(EVENT).await.unwrap(), before);
}

#[tokio::test]
async fn the_actor_expires_holds_when_its_clock_reaches_them() {
    let r = rig(ActorConfig::default());
    r.handle.send(req(create(1, None))).await.unwrap();
    r.handle.send(req(create(2, None))).await.unwrap();
    r.handle.send(req(hold(1, &[0, 1]))).await.unwrap();
    let extend = Command::ExtendHold {
        token: tok(2),
        ttl_secs: 1_800,
    };
    r.handle.send(req(extend)).await.unwrap();

    r.clock.set(Timestamp(T0.plus_secs(900).0 - 1));
    settle().await;
    assert_eq!(committed_ops(&r.store).len(), 4, "nothing is due yet");

    // No command arrives: the timer alone expires hold 1, and not hold 2,
    // whose first expiry was replaced.
    r.clock.set(T0.plus_secs(900));
    settle().await;
    assert_eq!(committed_ops(&r.store)[4..], [(5, Op::Expire)]);
    let stored = r.store.load_event(EVENT).await.unwrap();
    assert!(stored.objects.is_empty());
    assert_eq!(stored.holds.len(), 1);

    r.clock.advance_ms(3_600_000);
    settle().await;
    assert_eq!(committed_ops(&r.store)[5..], [(6, Op::Expire)]);
    assert_eq!(r.store.load_event(EVENT).await.unwrap().holds, vec![]);
}

#[tokio::test]
async fn overdue_holds_expire_promptly_after_a_reload() {
    let store = Arc::new(MemoryStore::new());
    store.create_event(EVENT, 1);
    let mut s = empty_state(20);
    run(&mut s, create(1, None), T0).unwrap();
    run(&mut s, hold(1, &[3]), T0).unwrap();
    store
        .commit_batch(s.take_batch().unwrap().unwrap())
        .await
        .unwrap();

    // Loaded long after the hold's expiry, as after an outage.
    let r = reload(&store, T0.plus_secs(5_000)).await;
    settle().await;
    assert_eq!(
        committed_ops(&store),
        [(1, Op::CreateHold), (2, Op::Hold), (3, Op::Expire)]
    );
    let stored = store.load_event(EVENT).await.unwrap();
    assert!(stored.holds.is_empty() && stored.objects.is_empty());
    drop(r.handle);
    assert_eq!(r.task.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn a_keyed_retry_after_a_lost_reply_returns_the_original_result() {
    let r = rig(ActorConfig::default());
    let booking = || keyed(book(None, &[4, 5], "77120"), "order-77120", 1);
    r.store.push_fault(Fault::LoseReply);
    assert_eq!(r.handle.send(booking()).await, Err(Failure::Unavailable));
    assert!(r.task.await.unwrap().is_err());

    // The batch was written; after a reload the retry gets the original reply,
    // not `object_not_free`, and books nothing twice.
    let r = reload(&r.store, T0.plus_secs(30)).await;
    let original = Ok(Outcome::Booked {
        seq: 1,
        objects: vec![4, 5],
    });
    assert_eq!(r.handle.send(booking()).await, original);
    let other = keyed(book(None, &[6], "77120"), "order-77120", 2);
    assert_eq!(
        r.handle.send(other).await,
        Err(Failure::IdempotencyMismatch)
    );
    assert_eq!(committed_ops(&r.store), [(1, Op::Book)]);
}
