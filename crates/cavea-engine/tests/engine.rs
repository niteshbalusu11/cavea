//! Engine behaviour through the public API: the state machine on its own, then
//! the actor against the in-memory store with a manual clock.

#![allow(
    clippy::unwrap_used,
    reason = "test helpers outside #[test] functions; a panic is a test failure"
)]

use std::sync::Arc;

use cavea_engine::{
    ActorConfig, ActorError, ChartView, Command, EventId, EventState, Failure, Fault, HoldId, Idx,
    LoadedEvent, ManualClock, MemoryStore, Missing, ObjectFailure, ObjectReason, ObjectRef,
    Outcome, Request, SeatStore, Status, StoreError, Timestamp, TokenHash, actor,
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

fn req(command: Command) -> Request {
    Request {
        actor: "pk_test".into(),
        command,
    }
}

fn run(s: &mut EventState, c: Command, now: Timestamp) -> Result<Outcome, Failure> {
    s.apply(&"pk_test".into(), &c, now).unwrap()
}

type Fingerprint = (
    u64,
    Vec<(Option<Status>, Option<HoldId>)>,
    Vec<(HoldId, Timestamp, Option<u32>, Vec<Idx>)>,
);

/// Everything observable about the state, in a comparable order.
fn fingerprint(s: &EventState) -> Fingerprint {
    let objects = (0..s.chart().len() as u32)
        .map(|i| (s.status(i), s.holder(i)))
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

/// INV-1 and INV-2 (no expiry yet): every object is free or held, and held
/// exactly when it is listed under exactly one hold, the one it names.
fn check_invariants(s: &EventState) {
    for i in 0..s.chart().len() as u32 {
        let listed: Vec<_> = s.holds().filter(|h| h.objects.contains(&i)).collect();
        match s.status(i) {
            Some(Status::FREE) => assert!(listed.is_empty() && s.holder(i).is_none()),
            Some(Status::HELD) => {
                assert_eq!(listed.len(), 1, "object {i} under {} holds", listed.len());
                assert_eq!(s.holder(i), Some(listed[0].id));
            }
            other => panic!("object {i} has status {other:?}"),
        }
    }
    for h in s.holds() {
        assert!(h.max_objects.is_none_or(|m| h.objects.len() <= m as usize));
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
    let not_free = |idx| ObjectFailure {
        idx,
        reason: ObjectReason::NotFree,
    };
    assert_eq!(
        refused,
        Err(Failure::ObjectNotFree(vec![not_free(1), not_free(3)]))
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

    // Expiry is checked at processing time; the deadline itself is expired.
    let deadline = T0.plus_secs(900);
    assert!(run(&mut s, hold(1, &[1]), Timestamp(deadline.0 - 1)).is_ok());
    let before = fingerprint(&s);
    for c in [
        hold(1, &[2]),
        release(1, &[0]),
        Command::ExtendHold {
            token: tok(1),
            ttl_secs: 600,
        },
    ] {
        assert_eq!(run(&mut s, c, deadline), Err(Failure::HoldTokenExpired));
    }
    assert_eq!(fingerprint(&s), before);
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
        Err(Failure::ObjectNotHeldByToken(vec![
            ObjectFailure {
                idx: 3,
                reason: ObjectReason::NotHeldByToken
            },
            ObjectFailure {
                idx: 4,
                reason: ObjectReason::NotHeldByToken
            },
        ]))
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
        Err(Failure::InvalidRequest(
            cavea_engine::InvalidReason::TtlOutOfRange
        ))
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

/// Random sequences of the four commands and clock advances. After every
/// command: INV-1 and INV-2 hold, a refusal changed nothing, and `seq` moved by
/// at most one. Batches are committed at random points; the store rejects any
/// `seq` gap or repeat (INV-5), and reloading from it reproduces memory.
#[tokio::test]
async fn random_command_sequences_keep_invariants() {
    const SEATS: u32 = 12;
    for seed in 1..=150u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let store = MemoryStore::new();
        store.create_event(EVENT, 1);
        let mut s = empty_state(SEATS);
        let mut now = T0;
        for _ in 0..300 {
            let t = rng.below(6) as u8;
            let objects = |rng: &mut Rng, min: u64| -> Vec<ObjectRef> {
                (0..min + rng.below(4))
                    .map(|_| match rng.below(20) {
                        0 => ObjectRef::Label("missing".into()),
                        _ => ObjectRef::Idx(rng.below(u64::from(SEATS)) as u32),
                    })
                    .collect()
            };
            let command = match rng.below(9) {
                0 => Command::CreateHold {
                    token: tok(t),
                    ttl_secs: [30, 60, 300, 900][rng.below(4) as usize],
                    max_objects: [None, Some(0), Some(2), Some(4)][rng.below(4) as usize],
                },
                1..=3 => Command::Hold {
                    token: tok(t),
                    objects: objects(&mut rng, 1),
                },
                4 | 5 => Command::Release {
                    token: tok(t),
                    objects: objects(&mut rng, 0),
                },
                6 => Command::ExtendHold {
                    token: tok(t),
                    ttl_secs: 60 + rng.below(600) as u32,
                },
                7 => {
                    now = Timestamp(now.0 + rng.below(30_000));
                    continue;
                }
                _ => {
                    if let Some(batch) = s.take_batch().unwrap() {
                        store.commit_batch(batch).await.unwrap();
                    }
                    continue;
                }
            };
            let before = fingerprint(&s);
            let reply = run(&mut s, command, now);
            let after = fingerprint(&s);
            match &reply {
                Err(_) => assert_eq!(after, before, "seed {seed}: refusal changed state"),
                Ok(_) if after.0 == before.0 => {
                    assert_eq!(after, before, "seed {seed}: change without a seq")
                }
                Ok(_) => assert_eq!(after.0, before.0 + 1, "seed {seed}"),
            }
            check_invariants(&s);
        }
        if let Some(batch) = s.take_batch().unwrap() {
            store.commit_batch(batch).await.unwrap();
        }
        let loaded = store.load_event(EVENT).await.unwrap();
        let reloaded = EventState::restore(EVENT, chart(SEATS), loaded).unwrap();
        assert_eq!(fingerprint(&reloaded), fingerprint(&s), "seed {seed}");
    }
}

// ---- The actor ----

struct Rig {
    store: Arc<MemoryStore>,
    handle: cavea_engine::ActorHandle,
    task: tokio::task::JoinHandle<Result<(), ActorError>>,
}

fn rig(config: ActorConfig) -> Rig {
    let store = Arc::new(MemoryStore::new());
    store.create_event(EVENT, 1);
    let (handle, fut) = actor(empty_state(20), store.clone(), ManualClock::new(T0), config);
    Rig {
        store,
        handle,
        task: tokio::spawn(fut),
    }
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
