//! Realtime publication against the in-memory store on a manual clock (08
//! §4.5, §8.3, 13 §3.2): subscribers are modelled as clients that decode and
//! apply every frame, and are checked against the committed log.

#![allow(
    clippy::unwrap_used,
    reason = "test helpers outside #[test] functions; a panic is a test failure"
)]

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use cavea_engine::{
    Authority, ChartVersionId, ChartView, Command, End, EventId, Fault, Frame, HoldId, ManualClock,
    MemoryCharts, MemoryStore, ObjectRef, Received, Registry, RegistryConfig, Request, SeatStore,
    Status, Subscription, Timestamp, TokenHash, interval_ms,
};
use cavea_format::wire::{self, ServerFrame, View};
use tokio::sync::oneshot;

const T0: Timestamp = Timestamp(1_000_000);
const EV: EventId = EventId(7);
const CHART: ChartVersionId = ChartVersionId(70);
const N: u32 = 40;
const HASH: [u8; 16] = [9; 16];
const IDLE_MS: u64 = 600_000;

type Reg = Registry<MemoryStore, MemoryCharts, ManualClock>;

fn setup() -> (Arc<MemoryStore>, Reg) {
    setup_with(N)
}

fn setup_with(objects: u32) -> (Arc<MemoryStore>, Reg) {
    let store = MemoryStore::with_clock(ManualClock::new(T0));
    store.create_event(EV, 0, CHART);
    let store = Arc::new(store);
    let charts = MemoryCharts::new();
    let view = ChartView::new((0..objects).map(|i| (100 + i, format!("S-{i}")))).unwrap();
    charts.insert(CHART, Arc::new(view.with_hash(HASH)));
    let reg = Registry::new(
        store.clone(),
        Arc::new(charts),
        store.clock().clone(),
        RegistryConfig::default(),
    );
    (store, reg)
}

fn tok(n: u8) -> TokenHash {
    TokenHash([n; 32])
}

fn objs(idx: &[u32]) -> Vec<ObjectRef> {
    idx.iter().map(|&i| ObjectRef::Idx(i)).collect()
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
        ttl_secs: 60,
        max_objects: None,
    })
}

fn hold(t: u8, idx: &[u32]) -> Request {
    req(Command::Hold {
        token: tok(t),
        objects: objs(idx),
    })
}

fn release(t: u8, idx: &[u32]) -> Request {
    req(Command::Release {
        token: tok(t),
        objects: objs(idx),
    })
}

fn extend(t: u8) -> Request {
    req(Command::ExtendHold {
        token: tok(t),
        ttl_secs: 120,
    })
}

fn book(t: u8, idx: &[u32]) -> Request {
    req(Command::Book {
        token: Some(tok(t)),
        objects: objs(idx),
        order_ref: "order".into(),
        details: Vec::new(),
    })
}

fn status(idx: &[u32], status: Status) -> Request {
    req(Command::ChangeStatus {
        objects: objs(idx),
        status,
    })
}

fn release_booked(idx: &[u32]) -> Request {
    req(Command::ReleaseBooked { objects: objs(idx) })
}

async fn settle() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
}

/// Advance past any publication interval and let the actor publish.
async fn tick(store: &MemoryStore) {
    store.clock().advance_ms(500);
    settle().await;
}

fn gate(store: &MemoryStore) -> oneshot::Sender<()> {
    let (open, wait) = oneshot::channel();
    store.push_fault(Fault::Wait(wait));
    open
}

fn poll_now<F: Future>(f: F) -> Option<F::Output> {
    let mut f = std::pin::pin!(f);
    match f.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(v) => Some(v),
        Poll::Pending => None,
    }
}

/// A buyer's renderer as far as state goes: applies frames by the rules of
/// 08 §8.6 and refuses anything else.
#[derive(Debug, Default, Clone)]
struct Client {
    seq: Option<u64>,
    codes: Vec<u8>,
    /// The last PRIVATE: hold expiry and objects.
    mine: Option<(u64, Vec<u32>)>,
}

impl Client {
    fn apply(&mut self, frame: &Frame) {
        match wire::decode_server(frame.bytes(), View::Public).unwrap() {
            ServerFrame::Snapshot(s) => {
                assert_eq!(s.seq, frame.seq());
                self.codes = vec![0; N as usize];
                s.write_into(&mut self.codes).unwrap();
                self.seq = Some(s.seq);
                // INV-9: the frame is a function of the public codes alone.
                let again = wire::encode_snapshot(View::Public, s.seq, &self.codes, &[]).unwrap();
                assert_eq!(frame.bytes(), again, "a snapshot carries only public codes");
            }
            ServerFrame::Delta(d) => {
                assert_eq!(
                    Some(d.from_seq),
                    self.seq,
                    "a delta continues the client's seq"
                );
                assert_eq!(d.to_seq, frame.seq());
                for c in d.changes(N).unwrap() {
                    self.codes[c.idx as usize] = c.code;
                }
                self.seq = Some(d.to_seq);
            }
            ServerFrame::Private(p) => {
                assert_eq!(
                    Some(frame.seq()),
                    self.seq,
                    "PRIVATE comes after the state it was taken at"
                );
                self.mine = Some((p.expires_at_ms, p.objects(N).unwrap().collect()));
            }
            other => panic!("unexpected frame {other:?}"),
        }
    }

    /// A client that already has the committed state at `seq`.
    fn at(seq: u64, state: &Committed) -> Self {
        Self {
            seq: Some(seq),
            codes: state.public(),
            mine: None,
        }
    }
}

/// Apply every frame the subscription has ready; panics on anything else.
fn drain(sub: &mut Subscription, client: &mut Client) -> Vec<Frame> {
    let mut frames = Vec::new();
    while let Some(next) = poll_now(sub.next()) {
        match next {
            Received::Frame(f) => {
                client.apply(&f);
                frames.push(f);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    frames
}

fn start(sub: &mut Subscription, client: &mut Client) {
    for f in std::mem::take(&mut sub.first) {
        client.apply(&f);
    }
}

fn frame_type(f: &Frame) -> u8 {
    f.bytes()[0]
}

/// Committed state after some `seq`, rebuilt from the batches the store wrote.
#[derive(Debug, Clone)]
struct Committed {
    status: Vec<u8>,
    holder: Vec<Option<HoldId>>,
    holds: HashMap<HoldId, (TokenHash, Timestamp)>,
}

impl Committed {
    fn public(&self) -> Vec<u8> {
        self.status.iter().map(|&s| u8::from(s != 0)).collect()
    }

    fn private(&self, token: TokenHash) -> (u64, Vec<u32>) {
        let Some((&id, &(_, expires))) = self.holds.iter().find(|(_, h)| h.0 == token) else {
            return (0, Vec::new());
        };
        let objects = (0..N).filter(|&i| self.holder[i as usize] == Some(id));
        (expires.0, objects.collect())
    }
}

fn history(store: &MemoryStore) -> HashMap<u64, Committed> {
    let mut s = Committed {
        status: vec![0; N as usize],
        holder: vec![None; N as usize],
        holds: HashMap::new(),
    };
    let mut at = HashMap::from([(0, s.clone())]);
    for b in store.committed().iter().filter(|b| b.event == EV) {
        for h in &b.holds_created {
            s.holds.insert(h.id, (h.token_hash, h.expires_at));
        }
        for (id, expires) in &b.holds_extended {
            s.holds.get_mut(id).unwrap().1 = *expires;
        }
        for id in &b.holds_deleted {
            s.holds.remove(id);
        }
        for o in &b.objects_set {
            let i = (o.oid - 100) as usize;
            s.status[i] = o.status.code();
            s.holder[i] = o.hold_id;
        }
        for oid in &b.objects_freed {
            let i = (oid - 100) as usize;
            s.status[i] = 0;
            s.holder[i] = None;
        }
        at.insert(b.seq, s.clone());
    }
    at
}

fn stored_seq(store: &MemoryStore) -> u64 {
    poll_now(store.load_event(EV)).unwrap().unwrap().seq
}

/// The client holds exactly the public reduction of the committed state at
/// its `seq`, and with a token, that token's committed hold.
fn check(client: &Client, history: &HashMap<u64, Committed>, token: Option<TokenHash>, why: &str) {
    let seq = client.seq.unwrap();
    let state = history
        .get(&seq)
        .unwrap_or_else(|| panic!("{why}: seq {seq} never committed"));
    assert_eq!(client.codes, state.public(), "{why}: seq {seq}");
    if let Some(token) = token {
        assert_eq!(
            client.mine.clone().unwrap(),
            state.private(token),
            "{why}: seq {seq}"
        );
    }
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

struct Sub {
    sub: Subscription,
    client: Client,
    token: Option<TokenHash>,
}

/// Apply what the subscriber has ready; after a failed actor, subscribe again
/// from the client's `seq`, as a transport does.
async fn pump(reg: &Reg, s: &mut Sub, why: &str) -> usize {
    let mut reloads = 0;
    loop {
        match poll_now(s.sub.next()) {
            None => return reloads,
            Some(Received::Frame(f)) => s.client.apply(&f),
            Some(Received::Ended(End::Failed)) => {
                reloads += 1;
                let seq = s.client.seq.unwrap();
                s.sub = reg.subscribe(EV, seq, s.token).await.unwrap();
                start(&mut s.sub, &mut s.client);
            }
            Some(other) => panic!("{why}: {other:?}"),
        }
    }
}

/// 13 §3.2 with INV-8 and INV-9 checked continuously: commands, expiries,
/// slow, failed and lost commits, and subscribers attaching and resuming at
/// random points. Every subscriber always holds the committed public state at
/// its `seq` (and its token's committed hold), and once things go quiet that
/// `seq` is the committed one.
#[tokio::test]
async fn subscribers_hold_exactly_the_committed_state_through_random_runs() {
    #[derive(Default)]
    struct Seen {
        reloads: usize,
        resumed: usize,
        snapshots: usize,
    }
    let mut seen = Seen::default();
    for seed in 1..=12u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let (store, reg) = setup();
        let mut subs: Vec<Sub> = Vec::new();
        let mut gates = Vec::new();
        for step in 0..400 {
            let why = format!("seed {seed} step {step}");
            for _ in 0..=rng.below(3) {
                let t = rng.below(4) as u8 + 1;
                let i = rng.below(u64::from(N)) as u32;
                let request = match rng.below(10) {
                    0 => create(t),
                    1 | 2 => hold(t, &[i, (i + 1) % N]),
                    3 => release(t, &[i]),
                    4 => extend(t),
                    5 => book(t, &[]),
                    6 => status(&[i], Status::BLOCKED),
                    7 => status(&[i], Status::FREE),
                    8 => status(&[i], Status::from_code(5).unwrap()),
                    _ => release_booked(&[i]),
                };
                // Replies are not awaited: commands pile up into batches.
                let _ = reg.submit(EV, request).await;
            }
            match rng.below(40) {
                0 => gates.push(gate(&store)),
                1 => store.push_fault(Fault::Fail),
                2 => store.push_fault(Fault::LoseReply),
                3 | 4 => drop(gates.pop()),
                _ => {}
            }
            let ms = match rng.below(20) {
                0 => 61_000,
                n => n * 15,
            };
            store.clock().advance_ms(ms);
            settle().await;

            if rng.below(8) == 0 || subs.is_empty() {
                let history = history(&store);
                let token = (rng.below(2) == 0).then(|| tok(rng.below(4) as u8 + 1));
                let resume_from = match (rng.below(3), subs.first()) {
                    (0, Some(s)) => s.client.seq.unwrap(),
                    (1, _) => *history
                        .keys()
                        .nth(rng.below(history.len() as u64) as usize)
                        .unwrap(),
                    _ => 0,
                };
                if let Ok(mut sub) = reg.subscribe(EV, resume_from, token).await {
                    let mut client = if resume_from == 0 {
                        Client::default()
                    } else {
                        Client::at(resume_from, &history[&resume_from])
                    };
                    match sub.first.first().map(frame_type) {
                        Some(wire::types::DELTA) => seen.resumed += 1,
                        Some(wire::types::SNAPSHOT) => seen.snapshots += 1,
                        _ => {}
                    }
                    start(&mut sub, &mut client);
                    subs.push(Sub { sub, client, token });
                }
            }
            for s in &mut subs {
                seen.reloads += pump(&reg, s, &why).await;
            }
            let history = history(&store);
            for s in &subs {
                check(&s.client, &history, s.token, &why);
            }
        }
        drop(gates);
        tick(&store).await;
        for s in &mut subs {
            pump(&reg, s, "end").await;
        }
        tick(&store).await;
        let history = history(&store);
        for s in &mut subs {
            pump(&reg, s, "end").await;
            check(&s.client, &history, s.token, &format!("seed {seed} end"));
            assert_eq!(
                s.client.seq,
                Some(stored_seq(&store)),
                "seed {seed}: caught up"
            );
        }
    }
    // The run reached the paths it is meant to check.
    assert!(seen.reloads > 10 && seen.resumed > 10 && seen.snapshots > 10);
}

/// `(from_seq, to_seq, changes)` of a DELTA.
fn delta(f: &Frame) -> (u64, u64, Vec<(u32, u8)>) {
    match wire::decode_server(f.bytes(), View::Public).unwrap() {
        ServerFrame::Delta(d) => {
            let changes = d.changes(N).unwrap().map(|c| (c.idx, c.code)).collect();
            (d.from_seq, d.to_seq, changes)
        }
        other => panic!("not a delta: {other:?}"),
    }
}

/// Apply frames until the subscription ends; returns why.
fn until_end(sub: &mut Subscription, client: &mut Client) -> End {
    loop {
        match poll_now(sub.next()) {
            Some(Received::Frame(f)) => client.apply(&f),
            Some(Received::Ended(end)) => return end,
            other => panic!("unexpected {other:?}"),
        }
    }
}

async fn subscribe(reg: &Reg, last_seq: u64, token: Option<u8>) -> (Subscription, Client) {
    let mut sub = reg.subscribe(EV, last_seq, token.map(tok)).await.unwrap();
    let mut client = Client::default();
    start(&mut sub, &mut client);
    (sub, client)
}

#[tokio::test]
async fn nothing_is_published_while_its_commit_is_open() {
    let (store, reg) = setup();
    let (mut sub, mut client) = subscribe(&reg, 0, None).await;
    assert_eq!(sub.hello.objects, N);
    assert_eq!(sub.hello.chart_hash, HASH);
    assert_eq!(sub.hello.server_time_ms, T0.0);
    assert_eq!(client.seq, Some(0));

    let open = gate(&store);
    let reply = reg.submit(EV, status(&[5], Status::BLOCKED)).await.unwrap();
    settle().await;
    tick(&store).await;
    assert!(drain(&mut sub, &mut client).is_empty());
    assert_eq!(reg.snapshot(EV).await.unwrap().seq(), 0);
    assert_eq!(reg.after(EV, 0).await.unwrap().unwrap().seq(), 0);

    open.send(()).unwrap();
    reply.await.unwrap();
    settle().await;
    let frames = drain(&mut sub, &mut client);
    assert_eq!(
        frames.iter().map(delta).collect::<Vec<_>>(),
        [(0, 1, vec![(5, 1)])]
    );
    assert_eq!(reg.snapshot(EV).await.unwrap().seq(), 1);
}

#[tokio::test]
async fn failed_batches_are_never_published_and_the_reload_matches_the_store() {
    let (store, reg) = setup();
    let (mut sub, mut client) = subscribe(&reg, 0, None).await;
    reg.send(EV, status(&[1], Status::BLOCKED)).await.unwrap();
    settle().await;
    drain(&mut sub, &mut client);

    // One batch in flight and one applied behind it; both fail.
    let open = gate(&store);
    let first = reg.submit(EV, status(&[5], Status::BLOCKED)).await.unwrap();
    settle().await;
    let second = reg.submit(EV, status(&[6], Status::BLOCKED)).await.unwrap();
    settle().await;
    store.set_down(true);
    open.send(()).unwrap();
    assert!(first.await.is_err() && second.await.is_err());
    tick(&store).await;
    assert_eq!(until_end(&mut sub, &mut client), End::Failed);
    assert_eq!(client.seq, Some(1));
    assert_eq!(client.codes[5] + client.codes[6], 0);

    store.set_down(false);
    let mut sub = reg.subscribe(EV, 1, None).await.unwrap();
    assert!(
        sub.first.is_empty(),
        "the reloaded event is at the client's seq"
    );
    assert_eq!(reg.snapshot(EV).await.unwrap().seq(), 1);
    reg.send(EV, status(&[7], Status::BLOCKED)).await.unwrap();
    tick(&store).await;
    drain(&mut sub, &mut client);
    check(&client, &history(&store), None, "after reload");
    assert_eq!(client.seq, Some(stored_seq(&store)));
}

#[tokio::test]
async fn a_batch_whose_reply_was_lost_is_published_after_reload_as_state() {
    let (store, reg) = setup();
    let (mut sub, mut client) = subscribe(&reg, 0, None).await;
    reg.send(EV, create(1)).await.unwrap();
    settle().await;
    drain(&mut sub, &mut client);

    store.push_fault(Fault::LoseReply);
    assert!(reg.send(EV, hold(1, &[4])).await.is_err());
    tick(&store).await;
    assert_eq!(
        until_end(&mut sub, &mut client),
        End::Failed,
        "no phantom delta first"
    );
    assert_eq!(client.codes[4], 0);

    let mut sub = reg.subscribe(EV, client.seq.unwrap(), None).await.unwrap();
    let first = std::mem::take(&mut sub.first);
    assert_eq!(
        first.iter().map(frame_type).collect::<Vec<_>>(),
        [wire::types::SNAPSHOT]
    );
    client.apply(&first[0]);
    assert_eq!((client.seq, client.codes[4]), (Some(2), 1));
}

#[tokio::test]
async fn deltas_are_coalesced_per_interval_and_carry_only_what_the_public_sees() {
    let (store, reg) = setup();
    let (mut sub, mut client) = subscribe(&reg, 0, None).await;
    let clock = store.clock();

    // The first commit after a quiet interval goes out at once.
    reg.send(EV, status(&[3], Status::BLOCKED)).await.unwrap();
    settle().await;
    let frames = drain(&mut sub, &mut client);
    assert_eq!(
        frames.iter().map(delta).collect::<Vec<_>>(),
        [(0, 1, vec![(3, 1)])]
    );

    // Later ones wait for the interval to end, snapshot included.
    for request in [
        status(&[6], Status::BLOCKED),
        status(&[6], Status::FREE),
        status(&[6], Status::from_code(5).unwrap()),
        create(1),
        hold(1, &[8]),
        status(&[9], Status::BLOCKED),
        status(&[9], Status::FREE),
    ] {
        reg.send(EV, request).await.unwrap();
    }
    settle().await;
    clock.advance_ms(99);
    settle().await;
    assert!(drain(&mut sub, &mut client).is_empty());
    assert_eq!(reg.snapshot(EV).await.unwrap().seq(), 1);
    clock.advance_ms(1);
    settle().await;
    let frames = drain(&mut sub, &mut client);
    // 6 changed three times: once, with its final code. 9 went and came back.
    assert_eq!(
        frames.iter().map(delta).collect::<Vec<_>>(),
        [(1, 8, vec![(6, 1), (8, 1)])]
    );
    assert_eq!(reg.snapshot(EV).await.unwrap().seq(), 8);

    // An extend and held to booked change nothing public, but move `seq`.
    clock.advance_ms(100);
    reg.send(EV, extend(1)).await.unwrap();
    reg.send(EV, book(1, &[])).await.unwrap();
    settle().await;
    clock.advance_ms(100);
    settle().await;
    let frames = drain(&mut sub, &mut client);
    assert_eq!(
        frames.iter().map(delta).collect::<Vec<_>>(),
        [(8, 9, vec![]), (9, 10, vec![])]
    );

    // Nothing committed, nothing sent.
    clock.advance_ms(10_000);
    settle().await;
    assert!(drain(&mut sub, &mut client).is_empty());
    check(&client, &history(&store), None, "end");
}

#[tokio::test]
async fn the_interval_stretches_with_more_than_twenty_thousand_subscribers() {
    assert_eq!(interval_ms(0), 100);
    assert_eq!(interval_ms(20_000), 100);
    assert_eq!(interval_ms(30_000), 150);
    assert_eq!(interval_ms(100_000), 500);
    assert_eq!(interval_ms(1_000_000), 500);

    let (store, reg) = setup();
    let mut subs = Vec::new();
    for _ in 0..40_000 {
        subs.push(reg.subscribe(EV, 0, None).await.unwrap());
    }
    assert_eq!(reg.subscribers(EV), 40_000);
    reg.send(EV, status(&[1], Status::BLOCKED)).await.unwrap();
    reg.send(EV, status(&[2], Status::BLOCKED)).await.unwrap();
    settle().await;
    store.clock().advance_ms(199);
    settle().await;
    assert_eq!(reg.snapshot(EV).await.unwrap().seq(), 1);
    store.clock().advance_ms(1);
    settle().await;
    assert_eq!(reg.snapshot(EV).await.unwrap().seq(), 2);

    // Subscribers leaving shortens a pending interval.
    reg.send(EV, status(&[3], Status::BLOCKED)).await.unwrap();
    settle().await;
    store.clock().advance_ms(50);
    settle().await;
    drop(subs);
    assert_eq!(reg.subscribers(EV), 0);
    store.clock().advance_ms(50);
    settle().await;
    assert_eq!(reg.snapshot(EV).await.unwrap().seq(), 3);
}

#[tokio::test]
async fn a_subscriber_resumes_from_a_buffered_seq_and_otherwise_gets_the_snapshot() {
    let (store, reg) = setup();
    let (mut sub, mut client) = subscribe(&reg, 0, None).await;
    let mut published = Vec::new();
    for i in 0..5 {
        reg.send(EV, status(&[i], Status::BLOCKED)).await.unwrap();
        tick(&store).await;
        published.extend(drain(&mut sub, &mut client));
    }
    let history = history(&store);
    for last in 1..=5u64 {
        let mut resumed = reg.subscribe(EV, last, None).await.unwrap();
        let first = std::mem::take(&mut resumed.first);
        let expected = &published[last as usize..];
        assert_eq!(
            first.iter().map(Frame::bytes).collect::<Vec<_>>(),
            expected.iter().map(Frame::bytes).collect::<Vec<_>>()
        );
        let mut c = Client::at(last, &history[&last]);
        first.iter().for_each(|f| c.apply(f));
        assert_eq!(c.codes, client.codes);
    }

    // Frames older than 60 s are dropped once a newer one is published.
    store.clock().advance_ms(60_000);
    reg.send(EV, status(&[10], Status::BLOCKED)).await.unwrap();
    tick(&store).await;
    drain(&mut sub, &mut client);
    let old = reg.subscribe(EV, 3, None).await.unwrap();
    assert_eq!(
        old.first.iter().map(frame_type).collect::<Vec<_>>(),
        [wire::types::SNAPSHOT]
    );
    let recent = reg.subscribe(EV, 5, None).await.unwrap();
    assert_eq!(
        recent.first.iter().map(frame_type).collect::<Vec<_>>(),
        [wire::types::DELTA]
    );

    // A reload starts with an empty buffer: older seqs get the snapshot.
    store.push_fault(Fault::Fail);
    assert!(reg.send(EV, status(&[11], Status::BLOCKED)).await.is_err());
    settle().await;
    drop((old, recent));
    let stale = reg.subscribe(EV, 5, None).await.unwrap();
    assert_eq!(
        stale.first.iter().map(frame_type).collect::<Vec<_>>(),
        [wire::types::SNAPSHOT]
    );
    let current = reg.subscribe(EV, 6, None).await.unwrap();
    assert!(current.first.is_empty());
}

#[tokio::test]
async fn attaching_mid_commit_or_mid_interval_loses_and_repeats_nothing() {
    let (store, reg) = setup();
    let (mut a, mut ca) = subscribe(&reg, 0, None).await;

    // While a commit is in flight.
    let open = gate(&store);
    let reply = reg.submit(EV, status(&[1], Status::BLOCKED)).await.unwrap();
    settle().await;
    let mut b = reg.subscribe(EV, ca.seq.unwrap(), None).await.unwrap();
    let mut cb = ca.clone();
    start(&mut b, &mut cb);
    open.send(()).unwrap();
    reply.await.unwrap();
    settle().await;
    assert_eq!(drain(&mut a, &mut ca).len(), 1);
    assert_eq!(drain(&mut b, &mut cb).len(), 1);

    // Committed but not yet published: the interval has not ended.
    reg.send(EV, status(&[2], Status::BLOCKED)).await.unwrap();
    settle().await;
    let mut c = reg.subscribe(EV, cb.seq.unwrap(), None).await.unwrap();
    assert!(
        c.first.is_empty(),
        "the snapshot is still at the last publication"
    );
    let mut cc = cb.clone();
    tick(&store).await;
    for (sub, client) in [(&mut a, &mut ca), (&mut b, &mut cb), (&mut c, &mut cc)] {
        assert_eq!(drain(sub, client).len(), 1);
        assert_eq!(client.codes, ca_codes(&store));
    }
}

fn ca_codes(store: &MemoryStore) -> Vec<u8> {
    history(store)[&stored_seq(store)].public()
}

#[tokio::test]
async fn a_stalled_subscriber_lags_and_recovers_from_the_snapshot() {
    let (store, reg) = setup();
    let (mut slow, mut cs) = subscribe(&reg, 0, None).await;
    let (mut fast, mut cf) = subscribe(&reg, 0, None).await;
    for i in 0..(cavea_engine::QUEUE_FRAMES as u32 + 6) {
        let s = if i % 2 == 0 {
            Status::BLOCKED
        } else {
            Status::FREE
        };
        reg.send(EV, status(&[0], s)).await.unwrap();
        tick(&store).await;
        drain(&mut fast, &mut cf);
    }
    assert!(matches!(poll_now(slow.next()), Some(Received::Lagged)));
    assert!(
        matches!(poll_now(slow.next()), Some(Received::Lagged)),
        "until it resyncs"
    );

    let frames = slow.resync().unwrap();
    assert_eq!(
        frames.iter().map(frame_type).collect::<Vec<_>>(),
        [wire::types::SNAPSHOT]
    );
    frames.iter().for_each(|f| cs.apply(f));
    assert_eq!((cs.seq, &cs.codes), (cf.seq, &cf.codes));
    reg.send(EV, status(&[39], Status::BLOCKED)).await.unwrap();
    tick(&store).await;
    assert_eq!(drain(&mut slow, &mut cs).len(), 1);
    drain(&mut fast, &mut cf);
    assert_eq!(cs.codes, cf.codes);
}

#[tokio::test]
async fn private_frames_follow_their_delta_for_the_token_only() {
    let (store, reg) = setup();
    let (mut mine, mut cm) = subscribe(&reg, 0, Some(1)).await;
    let (mut other, mut co) = subscribe(&reg, 0, Some(2)).await;
    assert_eq!(cm.mine, Some((0, vec![])), "a token without a hold");

    let steps = [
        create(1),
        hold(1, &[3, 4]),
        release(1, &[4]),
        extend(1),
        book(1, &[3]),
        create(1),
        hold(1, &[5]),
    ];
    for request in steps {
        reg.send(EV, request).await.unwrap();
        tick(&store).await;
        let types: Vec<u8> = drain(&mut mine, &mut cm).iter().map(frame_type).collect();
        assert_eq!(types, [wire::types::DELTA, wire::types::PRIVATE]);
        check(&cm, &history(&store), Some(tok(1)), "mine");
        let types: Vec<u8> = drain(&mut other, &mut co).iter().map(frame_type).collect();
        assert_eq!(types, [wire::types::DELTA]);
    }
    assert_eq!(cm.mine.as_ref().unwrap().1, [5]);

    // A new subscriber with the token gets its holds on attach.
    let (_late, cl) = subscribe(&reg, 0, Some(1)).await;
    assert_eq!(cl.mine, cm.mine);

    // Expiry.
    store.clock().advance_ms(61_000);
    settle().await;
    let types: Vec<u8> = drain(&mut mine, &mut cm).iter().map(frame_type).collect();
    assert_eq!(types, [wire::types::DELTA, wire::types::PRIVATE]);
    assert_eq!(cm.mine, Some((0, vec![])));
    assert_eq!(drain(&mut other, &mut co).len(), 1);
}

#[tokio::test]
async fn subscriptions_keep_the_actor_and_end_with_it() {
    let (store, reg) = setup();
    let clock = store.clock().clone();
    let (sub, _) = subscribe(&reg, 0, None).await;
    for _ in 0..3 {
        clock.advance_ms(IDLE_MS);
        settle().await;
    }
    assert_eq!(reg.loaded(), 1, "a subscriber keeps the actor");
    drop(sub);
    clock.advance_ms(IDLE_MS);
    settle().await;
    clock.advance_ms(IDLE_MS);
    settle().await;
    assert_eq!(reg.loaded(), 0, "evicted once it is gone");

    // Shutdown publishes the last commit, then ends every subscription.
    let (mut sub, mut client) = subscribe(&reg, 0, None).await;
    let open = gate(&store);
    let reply = reg.submit(EV, status(&[2], Status::BLOCKED)).await.unwrap();
    settle().await;
    let stopping = tokio::spawn({
        let reg = reg.clone();
        async move { reg.shutdown().await }
    });
    settle().await;
    open.send(()).unwrap();
    stopping.await.unwrap();
    reply.await.unwrap();
    assert_eq!(until_end(&mut sub, &mut client), End::Stopped);
    assert_eq!(client.codes[2], 1);
    assert!(reg.subscribe(EV, 0, None).await.is_err());
}

#[tokio::test]
async fn polling_gets_one_delta_from_its_seq_or_the_snapshot() {
    let (store, reg) = setup();
    let (mut sub, mut client) = subscribe(&reg, 0, None).await;
    let mut published = Vec::new();
    for i in [3, 4, 3, 5] {
        let s = if client.codes[i as usize] == 0 {
            Status::BLOCKED
        } else {
            Status::FREE
        };
        reg.send(EV, status(&[i], s)).await.unwrap();
        tick(&store).await;
        published.extend(drain(&mut sub, &mut client));
    }
    let history = history(&store);
    let merged = reg.after(EV, 1).await.unwrap().unwrap();
    let mut c = Client::at(1, &history[&1]);
    c.apply(&merged);
    assert_eq!((c.seq, &c.codes), (client.seq, &client.codes));
    assert_eq!(
        reg.after(EV, 3).await.unwrap().unwrap().bytes(),
        published[3].bytes()
    );
    assert!(reg.after(EV, 4).await.unwrap().is_none());
    assert_eq!(
        frame_type(&reg.after(EV, 0).await.unwrap().unwrap()),
        wire::types::SNAPSHOT
    );
}

/// Measured, not asserted: run with
/// `cargo test --release -p cavea-engine --test realtime measure_ -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "measurement; run in release with --nocapture"]
async fn measure_realtime_publication() {
    const SEATS: u32 = 100_000;
    const ROUND: u32 = 2_000;
    const ROUNDS: u32 = 500;
    // The first pass warms up the allocator and is not reported.
    for (pass, subscribers) in [0, 0, 1, 10_000].into_iter().enumerate() {
        let (store, reg) = setup_with(SEATS);
        reg.send(EV, status(&[0], Status::FREE)).await.unwrap();
        let mut tasks = Vec::new();
        for _ in 0..subscribers {
            let mut sub = reg.subscribe(EV, 0, None).await.unwrap();
            tasks.push(tokio::spawn(async move {
                let mut frames = 0u64;
                loop {
                    match sub.next().await {
                        Received::Frame(_) => frames += 1,
                        Received::Lagged => frames += sub.resync().unwrap().len() as u64,
                        Received::Ended(_) => return frames,
                    }
                }
            }));
        }
        let started = std::time::Instant::now();
        for round in 0..ROUNDS {
            let mut replies = Vec::with_capacity(ROUND as usize);
            for k in 0..ROUND {
                let n = round * ROUND + k;
                let s = if (n / SEATS).is_multiple_of(2) {
                    Status::BLOCKED
                } else {
                    Status::FREE
                };
                replies.push(reg.submit(EV, status(&[n % SEATS], s)).await.unwrap());
            }
            for reply in replies {
                reply.await.unwrap();
            }
            // One coalescing interval per round: 2,000 commands per delta.
            store.clock().advance_ms(100);
        }
        let elapsed = started.elapsed();
        reg.shutdown().await;
        let mut frames = 0;
        for task in tasks {
            frames += task.await.unwrap();
        }
        if pass == 0 {
            continue;
        }
        println!(
            "{subscribers} subscribers: {:.0} commands/s ({} commands in {elapsed:?}), {frames} frames delivered",
            f64::from(ROUND * ROUNDS) / elapsed.as_secs_f64(),
            ROUND * ROUNDS,
        );
    }

    // From submitting a command to its delta being in a receiver, as the
    // first commit after a quiet interval, with a 100,000-object snapshot
    // re-encoded on the way.
    let (store, reg) = setup_with(SEATS);
    let mut sub = reg.subscribe(EV, 0, None).await.unwrap();
    let mut times = Vec::new();
    for i in 0..500 {
        store.clock().advance_ms(100);
        let s = if i % 2 == 0 {
            Status::BLOCKED
        } else {
            Status::FREE
        };
        let started = std::time::Instant::now();
        reg.send(EV, status(&[7], s)).await.unwrap();
        loop {
            if let Received::Frame(_) = sub.next().await {
                break;
            }
        }
        times.push(started.elapsed());
    }
    times.sort_unstable();
    println!(
        "command to delta received, 100,000 objects: p50 {:?}, p95 {:?}, max {:?}",
        times[times.len() / 2],
        times[times.len() * 95 / 100],
        times[times.len() - 1],
    );
    println!(
        "per subscriber: Subscription {} bytes, its pending next() {} bytes; frames shared",
        std::mem::size_of::<Subscription>(),
        std::mem::size_of_val(&sub.next()),
    );
}
