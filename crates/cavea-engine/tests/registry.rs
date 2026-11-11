//! The actor registry against the in-memory store on a manual clock: loading
//! once, poison and reload, ownership and takeover between two nodes, idle
//! eviction and shutdown (08 §4.6 to §4.8, 13 §3.2).

#![allow(
    clippy::unwrap_used,
    reason = "test helpers outside #[test] functions; a panic is a test failure"
)]

use std::future::Future;
use std::sync::Arc;

use cavea_engine::{
    ActorConfig, Authority, ChartVersionId, ChartView, Command, EventId, EventState, Failure,
    Fault, Idempotency, MEMORY_LEASE_MS, ManualClock, MemoryCharts, MemoryStore, ObjectRef, Op,
    Outcome, Registry, RegistryConfig, Reply, Request, SeatStore, Status, Timestamp, TokenHash,
};
use tokio::sync::oneshot;

const T0: Timestamp = Timestamp(1_000_000);
const EV: EventId = EventId(7);
const EV2: EventId = EventId(8);
const CHART: ChartVersionId = ChartVersionId(70);
const IDLE_MS: u64 = 600_000;

type Reg = Registry<MemoryStore, MemoryCharts, ManualClock>;

fn charts() -> Arc<MemoryCharts> {
    let charts = MemoryCharts::new();
    let view = ChartView::new((0..20).map(|i| (100 + i, format!("S-{i}")))).unwrap();
    charts.insert(CHART, Arc::new(view));
    Arc::new(charts)
}

/// A store with events `EV` and `EV2`, unowned, on a clock at `T0`.
fn store() -> Arc<MemoryStore> {
    let store = MemoryStore::with_clock(ManualClock::new(T0));
    store.create_event(EV, 0, CHART);
    store.create_event(EV2, 0, CHART);
    Arc::new(store)
}

fn registry(store: &Arc<MemoryStore>, config: RegistryConfig) -> Reg {
    Registry::new(store.clone(), charts(), store.clock().clone(), config)
}

fn tok(n: u8) -> TokenHash {
    TokenHash([n; 32])
}

fn req(command: Command) -> Request {
    Request {
        actor: "sk_test".into(),
        authority: Authority::Secret,
        command,
        idempotency: None,
    }
}

fn create(t: u8) -> Request {
    req(Command::CreateHold {
        token: tok(t),
        ttl_secs: 900,
        max_objects: None,
    })
}

fn hold(t: u8, idx: u32) -> Request {
    req(Command::Hold {
        token: tok(t),
        objects: vec![ObjectRef::Idx(idx)],
    })
}

fn block(idx: u32) -> Request {
    req(Command::ChangeStatus {
        objects: vec![ObjectRef::Idx(idx)],
        status: Status::BLOCKED,
    })
}

fn keyed(request: Request, key: &str) -> Request {
    Request {
        idempotency: Some(Idempotency {
            key: key.into(),
            fingerprint: [1; 32],
            checked_at: T0,
        }),
        ..request
    }
}

/// Let every runnable task run until it blocks.
async fn settle() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
}

fn gate(store: &MemoryStore) -> oneshot::Sender<()> {
    let (open, wait) = oneshot::channel();
    store.push_fault(Fault::Wait(wait));
    open
}

fn epoch(store: &MemoryStore, event: EventId) -> u64 {
    // A read: takes no ownership.
    futures_now(store.load_event(event)).unwrap().epoch
}

/// Run a store call that never waits.
fn futures_now<F: Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let waker = std::task::Waker::noop();
    match f.as_mut().poll(&mut std::task::Context::from_waker(waker)) {
        std::task::Poll::Ready(v) => v,
        std::task::Poll::Pending => panic!("store call waited"),
    }
}

/// `(seq, op)` of every committed log row for `event`, in order.
fn committed(store: &MemoryStore, event: EventId) -> Vec<(u64, Op)> {
    let batches = store.committed();
    batches
        .iter()
        .filter(|b| b.event == event)
        .flat_map(|b| &b.log)
        .map(|r| (r.seq, r.op))
        .collect()
}

/// The `seq` a successful reply carries.
fn seq(reply: &Reply) -> u64 {
    match reply {
        Ok(
            Outcome::HoldCreated { seq, .. }
            | Outcome::Held { seq, .. }
            | Outcome::StatusChanged { seq, .. },
        ) => *seq,
        other => panic!("unexpected reply {other:?}"),
    }
}

/// What a fresh load of `event` would hold, compared by everything the state
/// exposes.
fn stored(store: &MemoryStore, event: EventId) -> (u64, Vec<Option<Status>>, Vec<u64>) {
    let loaded = futures_now(store.load_event(event)).unwrap();
    let chart = futures_now(cavea_engine::ChartProvider::chart(&*charts(), CHART)).unwrap();
    let s = EventState::restore(event, chart, loaded).unwrap();
    let statuses = (0..20).map(|i| s.status(i)).collect();
    let mut holds: Vec<u64> = s.holds().map(|h| h.id.0).collect();
    holds.sort_unstable();
    (s.seq(), statuses, holds)
}

#[tokio::test]
async fn concurrent_first_requests_load_the_event_once() {
    let store = store();
    let reg = registry(&store, RegistryConfig::default());
    let tasks: Vec<_> = (0..8)
        .map(|i| {
            let reg = reg.clone();
            tokio::spawn(async move { reg.send(EV, block(i)).await })
        })
        .collect();
    for task in tasks {
        assert!(task.await.unwrap().is_ok());
    }
    assert_eq!(store.acquisitions(), 1);
    assert_eq!(epoch(&store, EV), 1);
    assert_eq!(committed(&store, EV).len(), 8);
}

#[tokio::test]
async fn a_failed_load_reaches_every_waiter_and_is_retried_after_a_pause() {
    let store = store();
    let reg = registry(&store, RegistryConfig::default());
    store.set_down(true);
    let tasks: Vec<_> = (0..8)
        .map(|i| {
            let reg = reg.clone();
            tokio::spawn(async move { reg.send(EV, block(i)).await })
        })
        .collect();
    for task in tasks {
        assert_eq!(task.await.unwrap(), Err(Failure::Unavailable));
    }
    assert_eq!(store.acquisitions(), 1, "one attempt for all waiters");

    // The reload-storm guard: within the pause, requests fail without
    // touching the store, even once it is back.
    store.set_down(false);
    store.clock().advance_ms(999);
    assert_eq!(reg.send(EV, block(0)).await, Err(Failure::Unavailable));
    assert_eq!(store.acquisitions(), 1);

    store.clock().advance_ms(1);
    assert!(reg.send(EV, block(0)).await.is_ok());
    assert_eq!(store.acquisitions(), 2);

    // A load that fails after acquiring, here for want of its chart, leaves
    // no lease behind for retries to keep renewing.
    let orphan = EventId(9);
    store.create_event(orphan, 0, ChartVersionId(404));
    assert_eq!(reg.send(orphan, block(0)).await, Err(Failure::Unavailable));
    assert_eq!(store.lease(orphan).unwrap().1, None);
    assert_eq!(epoch(&store, orphan), 1);
}

#[tokio::test]
async fn a_failed_commit_poisons_the_actor_and_the_next_request_reloads() {
    let store = store();
    let reg = registry(
        &store,
        RegistryConfig {
            actor: ActorConfig {
                max_batch: 1,
                ..ActorConfig::default()
            },
            ..RegistryConfig::default()
        },
    );
    reg.send(EV, create(1)).await.unwrap();
    reg.send(EV, hold(1, 2)).await.unwrap();
    let before = stored(&store, EV);

    // One batch in flight, one applied and waiting, one in the mailbox.
    let open = gate(&store);
    let first = reg.submit(EV, block(5)).await.unwrap();
    settle().await;
    let second = reg.submit(EV, block(6)).await.unwrap();
    settle().await;
    let third = reg.submit(EV, block(7)).await.unwrap();
    store.set_down(true);
    open.send(()).unwrap();
    for reply in [first, second, third] {
        assert_eq!(reply.await, Err(Failure::Unavailable));
    }
    settle().await;
    assert_eq!(reg.loaded(), 0, "the poisoned actor is dropped");

    store.set_down(false);
    assert_eq!(stored(&store, EV), before, "nothing of the failed batches");
    let reply = reg.send(EV, block(8)).await;
    assert_eq!(
        seq(&reply),
        3,
        "the reloaded actor continues the stored seq"
    );
    assert_eq!(epoch(&store, EV), 2);
    assert_eq!(
        committed(&store, EV),
        [(1, Op::CreateHold), (2, Op::Hold), (3, Op::ChangeStatus)]
    );
}

#[tokio::test]
async fn after_a_lost_reply_the_batch_is_kept_and_a_retry_gets_the_original_reply() {
    let store = store();
    let reg = registry(&store, RegistryConfig::default());
    reg.send(EV, create(1)).await.unwrap();
    store.push_fault(Fault::LoseReply);
    let held = || keyed(hold(1, 4), "hold-4");
    assert_eq!(reg.send(EV, held()).await, Err(Failure::Unavailable));
    settle().await;

    let retried = reg.send(EV, held()).await;
    assert!(
        matches!(&retried, Ok(Outcome::Held { seq: 2, objects, .. }) if objects == &[4]),
        "{retried:?}"
    );
    assert_eq!(stored(&store, EV).1[4], Some(Status::HELD));
    assert_eq!(committed(&store, EV), [(1, Op::CreateHold), (2, Op::Hold)]);
    assert_eq!(epoch(&store, EV), 2);
}

#[tokio::test]
async fn a_panicking_actor_is_dropped_and_the_event_reloaded() {
    let store = store();
    let reg = registry(&store, RegistryConfig::default());
    reg.send(EV, create(1)).await.unwrap();
    reg.send(EV2, create(1)).await.unwrap();

    store.push_fault(Fault::Panic);
    assert_eq!(reg.send(EV, hold(1, 3)).await, Err(Failure::Unavailable));
    settle().await;
    assert_eq!(reg.loaded(), 1, "only the panicked actor is gone");
    assert!(reg.send(EV2, hold(1, 3)).await.is_ok());

    let reply = reg.send(EV, hold(1, 3)).await;
    assert_eq!(seq(&reply), 2);
    assert_eq!(committed(&store, EV), [(1, Op::CreateHold), (2, Op::Hold)]);
}

/// INV-11 and 13 §3.2 "two nodes, lease expiry, takeover": node A stalls past
/// its lease, node B takes over, and A can no longer write.
#[tokio::test]
async fn a_second_node_takes_over_only_after_the_lease_lapses_and_fences_the_first() {
    let store_a = store();
    let store_b = Arc::new(store_a.peer("node-b"));
    // A's heartbeat comes later than its lease runs: a stalled node.
    let stalled = RegistryConfig {
        actor: ActorConfig {
            heartbeat_ms: 20_000,
            ..ActorConfig::default()
        },
        ..RegistryConfig::default()
    };
    let a = registry(&store_a, stalled);
    let b = registry(&store_b, RegistryConfig::default());
    let mut acked = Vec::new();

    acked.push(a.send(EV, create(1)).await);
    acked.push(a.send(EV, hold(1, 1)).await);
    acked.push(a.send(EV2, block(0)).await);
    assert_eq!(b.send(EV, block(9)).await, Err(Failure::Unavailable));
    assert_eq!(store_b.acquisitions(), 1, "refused: A's lease is live");

    store_a.clock().advance_ms(MEMORY_LEASE_MS + 1);
    acked.push(b.send(EV, block(2)).await);
    assert_eq!(b.send(EV2, block(1)).await.map(|o| seq(&Ok(o))), Ok(2));
    assert_eq!(epoch(&store_a, EV), 2);

    // A's next commit is fenced: its actor stops, the command is not applied.
    assert_eq!(a.send(EV, block(3)).await, Err(Failure::Unavailable));
    settle().await;
    assert_eq!(a.loaded(), 1);
    // A's next heartbeat for the idle event is fenced too.
    store_a.clock().advance_ms(5_000);
    settle().await;
    assert_eq!(a.loaded(), 0);
    // B holds the lease now: A cannot load the event again.
    assert_eq!(a.send(EV, block(3)).await, Err(Failure::Unavailable));
    acked.push(b.send(EV, block(4)).await);

    // Every acknowledged command is in the log exactly once, in order, and
    // nothing else is.
    let acked: Vec<u64> = acked.iter().filter(|r| r.is_ok()).map(seq).collect();
    assert_eq!(acked, [1, 2, 1, 3, 4]);
    let log: Vec<u64> = committed(&store_a, EV).iter().map(|r| r.0).collect();
    assert_eq!(log, [1, 2, 3, 4]);
    let statuses = stored(&store_a, EV).1;
    assert_eq!(statuses[3], Some(Status::FREE), "A's fenced command");
    assert_eq!(statuses[4], Some(Status::BLOCKED));
}

/// The late write 08 §4.6 described: a commit of only idempotency records that
/// timed out lands after the event was reloaded. Reloading raised the epoch,
/// so it is refused and a retry keeps getting the reloaded actor's reply.
#[tokio::test]
async fn a_late_write_from_before_a_reload_is_refused() {
    let store = store();
    let reg = registry(&store, RegistryConfig::default());
    reg.send(EV, create(1)).await.unwrap();

    // What the first actor would have committed for a refused keyed command.
    let loaded = store.load_event(EV).await.unwrap();
    let chart = cavea_engine::ChartProvider::chart(&*charts(), CHART)
        .await
        .unwrap();
    let mut old = EventState::restore(EV, chart, loaded).unwrap();
    let refused = keyed(hold(9, 1), "k");
    assert!(old.apply(&refused, T0).unwrap().is_err());
    let late = old.take_batch().unwrap().unwrap();
    assert!(late.log.is_empty() && late.epoch == 1);

    // The first actor dies; the next request reloads under a new epoch.
    store.push_fault(Fault::Fail);
    assert_eq!(reg.send(EV, block(0)).await, Err(Failure::Unavailable));
    settle().await;
    let first = reg.send(EV, keyed(create(9), "k")).await;
    assert_eq!(seq(&first), 2);

    assert_eq!(
        store.commit_batch(late).await,
        Err(cavea_engine::StoreError::Fenced)
    );
    assert_eq!(reg.send(EV, keyed(create(9), "k")).await, first);
}

#[tokio::test]
async fn overdue_holds_are_expired_and_committed_on_load() {
    let store = store();
    let reg = registry(&store, RegistryConfig::default());
    reg.send(EV, create(1)).await.unwrap();
    reg.send(EV, hold(1, 3)).await.unwrap();
    reg.shutdown().await;

    // Loaded long after the hold's expiry, as after an outage.
    store.clock().advance_ms(5_000_000);
    let reg = registry(&store, RegistryConfig::default());
    let _alive = reg.keep_alive(EV).await.unwrap();
    settle().await;
    assert_eq!(
        committed(&store, EV),
        [(1, Op::CreateHold), (2, Op::Hold), (3, Op::Expire)]
    );
    let (seq, statuses, holds) = stored(&store, EV);
    assert_eq!((seq, statuses[3], holds), (3, Some(Status::FREE), vec![]));
}

#[tokio::test]
async fn an_idle_actor_is_evicted_after_ten_minutes_and_reloaded_on_demand() {
    let store = store();
    let peer = store.peer("node-b");
    let reg = registry(&store, RegistryConfig::default());
    let clock = store.clock().clone();

    // A hold keeps the actor; it expires at 900 s, which is activity.
    reg.send(EV, create(1)).await.unwrap();
    clock.advance_ms(IDLE_MS);
    settle().await;
    assert_eq!(reg.loaded(), 1, "a hold is outstanding");
    clock.advance_ms(300_000);
    settle().await;
    assert_eq!(
        committed(&store, EV),
        [(1, Op::CreateHold), (2, Op::Expire)]
    );
    clock.advance_ms(IDLE_MS - 1);
    settle().await;
    assert_eq!(reg.loaded(), 1, "not before ten minutes");
    clock.advance_ms(1);
    settle().await;
    assert_eq!(reg.loaded(), 0);
    assert_eq!(store.lease(EV).unwrap().1, None, "the lease is released");
    assert_eq!(peer.acquire(EV).await.unwrap().epoch, 2);
    peer.release(EV, 2).await.unwrap();

    // A keep-alive guard holds it however long it is idle.
    let alive = reg.keep_alive(EV).await.unwrap();
    assert_eq!(epoch(&store, EV), 3, "reloaded");
    for _ in 0..3 {
        clock.advance_ms(IDLE_MS);
        settle().await;
    }
    assert_eq!(reg.loaded(), 1);
    // Dropped just before a check: the ten minutes start from the drop.
    clock.advance_ms(IDLE_MS - 1);
    drop(alive);
    clock.advance_ms(1);
    settle().await;
    assert_eq!(reg.loaded(), 1, "not right after the guard dropped");
    clock.advance_ms(IDLE_MS);
    settle().await;
    assert_eq!(reg.loaded(), 0);

    // A command after eviction reloads the event with its state.
    let reply = reg.send(EV, block(0)).await;
    assert_eq!(seq(&reply), 3);
    assert_eq!(epoch(&store, EV), 4);
}

#[tokio::test]
async fn shutdown_commits_accepted_work_and_releases_every_lease() {
    let store = store();
    let reg = registry(&store, RegistryConfig::default());
    reg.send(EV2, block(0)).await.unwrap();
    let open = gate(&store);
    let first = reg.submit(EV, block(1)).await.unwrap();
    settle().await;
    let second = reg.submit(EV, block(2)).await.unwrap();

    let stopping = tokio::spawn({
        let reg = reg.clone();
        async move { reg.shutdown().await }
    });
    settle().await;
    assert!(!stopping.is_finished(), "waits for the commit in flight");
    // Nothing more is taken, not even by an actor still flushing.
    assert_eq!(reg.send(EV, block(3)).await, Err(Failure::Unavailable));
    assert_eq!(
        reg.send(EventId(99), block(3)).await,
        Err(Failure::Unavailable)
    );
    open.send(()).unwrap();
    stopping.await.unwrap();

    assert_eq!(seq(&first.await), 1);
    assert_eq!(seq(&second.await), 2);
    assert_eq!(committed(&store, EV).len(), 2);
    assert_eq!(reg.loaded(), 0);
    for event in [EV, EV2] {
        assert_eq!(store.lease(event).unwrap().1, None);
    }
    assert_eq!(reg.send(EV2, block(1)).await, Err(Failure::Unavailable));
}
