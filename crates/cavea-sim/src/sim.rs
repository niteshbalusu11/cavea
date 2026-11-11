//! Deterministic simulation (13 §3.2): two nodes, each a real [`Registry`]
//! with its actors and feeds on a current-thread Tokio runtime of its own,
//! against one shared [`MemoryStore`] behind a wrapper that holds every commit
//! until the harness decides it. Clients and subscribers are simulated; one
//! seed decides everything.
//!
//! **Determinism.** Nothing runs unless the harness runs it. Each step the
//! harness acts (sends requests, decides commits, injects faults, moves the
//! clocks), then runs each node that is not frozen until it is idle: a
//! current-thread runtime with no I/O or time driver, polled in a fixed order.
//! Two of tokio's scheduling points are not fixed, and the harness keeps clear
//! of both. `watch` wakes its waiters in an order drawn from a per-runtime
//! random seed (`BigNotify`, settable only with `tokio_unstable`), so: the
//! engine's timers sleep on a [`SimClock`] that wakes due sleepers in the order
//! they started sleeping, not on [`ManualClock`]'s `watch`; and each node's
//! clients, subscriptions and shutdown go through one task, so no two harness
//! tasks wait on the same load or actor. And a fixed number of yields can cut
//! a chain of wakeups short at a point that depends on the order: a node runs
//! until three rounds of yields pass without it reading its clock or its
//! store. Commits waiting for a decision are sorted before any is chosen, and
//! replies, batches and frames are hashed by request, event and subscriber, so
//! a seed gives the same trace hash every time, in any process and with any
//! number of threads.
//!
//! **Faults** (13 §3.2): a commit that fails, one whose reply is lost after it
//! was written, a panic in a commit, a node crash with commits in flight (its
//! runtime is dropped), slow commits (left undecided while traffic arrives), a
//! node frozen past its lease while the other takes the event over, clock jumps
//! on one node, a node cut off from the store, duplicate and retried keyed
//! requests, idle eviction and orderly shutdown mid-traffic.
//!
//! **Checks**, every step: INV-5 and INV-6 (the committed log replays without
//! a gap into exactly the store's rows), INV-7 (an acknowledged command is in
//! the store; no reply before its commit), INV-8 and INV-9 (every subscriber
//! holds the public reduction of the committed state at its `seq`, never past
//! the store's), INV-10 (replay refuses a second booking), INV-11 (each batch
//! from the node that acquired its epoch, epochs never going back), that no
//! command is logged twice or after its batch failed, and that every caller
//! waiting on an actor whose commit failed or lost its reply gets
//! `unavailable`. At the end of a run: a
//! serial replay of the log through the model reproduces every reply clients
//! received, and no hold was used at or after its expiry.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Once, PoisonError};
use std::task::{Context, Poll, Waker};

use cavea_engine::{
    Acquired, ActorConfig, Authority, Batch, ChartVersionId, Clock, Command, End, EventId, Failure,
    Frame, HoldId, Idempotency, LoadedEvent, LogRecord, ManualClock, MemoryCharts, MemoryStore,
    OpenError, Outcome, Received, Registry, RegistryConfig, Replay, Reply, Request, SeatStore,
    StoreError, Subscription, Timestamp, TokenHash,
};
use cavea_format::wire::{self, ServerFrame, View};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot};

use crate::case::{T0, gen_request, reply_kind, token, token_of};
use crate::model::{Model, diff_loaded, public};
use crate::{EVENT, Rng, Stats, TraceHash, Violation, chart, ensure, poll_now};

const CHART: ChartVersionId = ChartVersionId(70);
const OBJECTS: u32 = 10;
const EVENTS: [EventId; 2] = [EVENT, EventId(8)];
const NODES: usize = 2;
/// A node is settled once this many rounds of yields in a row made no
/// progress, or after `SETTLE_ROUNDS` rounds in all.
const IDLE_ROUNDS: usize = 3;
const SETTLE_ROUNDS: usize = 5_000;

type Reg = Registry<SimStore, MemoryCharts, SimClock>;

/// A node's clock. Unlike [`ManualClock`], whose sleepers share one `watch`
/// and wake in an order tokio draws from a random seed, it wakes the sleepers
/// that are due in the order they started sleeping. It also counts every
/// use: an engine task that runs reads the clock or the store, so a node
/// whose count stops moving is idle.
#[derive(Clone)]
struct SimClock {
    timers: Arc<Mutex<Timers>>,
    progress: Arc<AtomicU64>,
}

#[derive(Default)]
struct Timers {
    now: u64,
    next: u64,
    /// By registration order: `(wake at, waker)`.
    sleepers: BTreeMap<u64, (u64, Waker)>,
}

impl SimClock {
    fn new(start: Timestamp, progress: Arc<AtomicU64>) -> Self {
        let timers = Timers {
            now: start.0,
            ..Timers::default()
        };
        Self {
            timers: Arc::new(Mutex::new(timers)),
            progress,
        }
    }

    fn set(&self, t: Timestamp) {
        let due: Vec<Waker> = {
            let mut timers = lock(&self.timers);
            timers.now = t.0;
            let ids: Vec<u64> = timers
                .sleepers
                .iter()
                .filter(|(_, s)| s.0 <= t.0)
                .map(|(id, _)| *id)
                .collect();
            ids.iter()
                .filter_map(|id| timers.sleepers.remove(id))
                .map(|(_, w)| w)
                .collect()
        };
        for waker in due {
            waker.wake();
        }
    }

    fn advance_ms(&self, ms: u64) {
        let now = lock(&self.timers).now;
        self.set(Timestamp(now.saturating_add(ms)));
    }
}

impl Clock for SimClock {
    fn now(&self) -> Timestamp {
        self.progress.fetch_add(1, Ordering::Relaxed);
        Timestamp(lock(&self.timers).now)
    }

    fn sleep_until(&self, t: Timestamp) -> impl Future<Output = ()> + Send {
        self.progress.fetch_add(1, Ordering::Relaxed);
        Sleep {
            clock: self.clone(),
            at: t.0,
            id: None,
        }
    }
}

/// [`SimClock::sleep_until`].
struct Sleep {
    clock: SimClock,
    at: u64,
    id: Option<u64>,
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        let mut timers = lock(&this.clock.timers);
        if timers.now >= this.at {
            if let Some(id) = this.id.take() {
                timers.sleepers.remove(&id);
            }
            return Poll::Ready(());
        }
        let id = match this.id {
            Some(id) => id,
            None => {
                timers.next += 1;
                timers.next
            }
        };
        this.id = Some(id);
        timers.sleepers.insert(id, (this.at, cx.waker().clone()));
        Poll::Pending
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            lock(&self.clock.timers).sleepers.remove(&id);
        }
    }
}

type Slot<T> = Arc<Mutex<Option<T>>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the store does with a commit the harness releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Commit,
    Fail,
    LoseReply,
    Panic,
}

/// A commit waiting for the harness.
struct Gate {
    node: usize,
    event: EventId,
    epoch: u64,
    seq: u64,
    actors: Vec<Arc<str>>,
    since: u64,
    tx: oneshot::Sender<Decision>,
}

#[derive(Default)]
struct Shared {
    gates: Vec<Gate>,
    step: u64,
    /// Every batch written, with the node that wrote it, in commit order.
    committed: Vec<(usize, Batch)>,
    /// Which node acquired each `(event, epoch)`.
    acquired: HashMap<(u128, u64), usize>,
    fenced: u64,
}

/// One node's connection to the database: commits wait for the harness.
struct SimStore {
    inner: MemoryStore,
    node: usize,
    shared: Arc<Mutex<Shared>>,
    progress: Arc<AtomicU64>,
}

impl SimStore {
    fn touch(&self) {
        self.progress.fetch_add(1, Ordering::Relaxed);
    }
}

impl SeatStore for SimStore {
    async fn acquire(&self, event: EventId) -> Result<Acquired, StoreError> {
        self.touch();
        let acquired = self.inner.acquire(event).await?;
        lock(&self.shared)
            .acquired
            .insert((event.0, acquired.epoch), self.node);
        Ok(acquired)
    }

    async fn load_event(&self, event: EventId) -> Result<LoadedEvent, StoreError> {
        self.touch();
        self.inner.load_event(event).await
    }

    async fn commit_batch(&self, batch: Batch) -> Result<(), StoreError> {
        self.touch();
        let (tx, rx) = oneshot::channel();
        {
            let mut shared = lock(&self.shared);
            let since = shared.step;
            shared.gates.push(Gate {
                node: self.node,
                event: batch.event,
                epoch: batch.epoch,
                seq: batch.seq,
                actors: batch.log.iter().map(|r| r.actor.clone()).collect(),
                since,
                tx,
            });
        }
        let decision = rx.await.unwrap_or(Decision::Fail);
        match decision {
            Decision::Fail => return Err(StoreError::Unavailable("sim: injected failure".into())),
            Decision::Panic => panic!("sim: injected panic in commit"),
            Decision::Commit | Decision::LoseReply => {}
        }
        self.touch();
        let copy = batch.clone();
        let result = self.inner.commit_batch(batch).await;
        {
            let mut shared = lock(&self.shared);
            match &result {
                Ok(()) => shared.committed.push((self.node, copy)),
                Err(StoreError::Fenced) => shared.fenced += 1,
                Err(_) => {}
            }
        }
        match (decision, result) {
            (Decision::LoseReply, Ok(())) => Err(StoreError::Unavailable("sim: lost reply".into())),
            (_, result) => result,
        }
    }

    async fn heartbeat(&self, event: EventId, epoch: u64) -> Result<(), StoreError> {
        self.touch();
        let result = self.inner.heartbeat(event, epoch).await;
        if result == Err(StoreError::Fenced) {
            lock(&self.shared).fenced += 1;
        }
        result
    }

    async fn release(&self, event: EventId, epoch: u64) -> Result<(), StoreError> {
        self.touch();
        self.inner.release(event, epoch).await
    }
}

struct Node {
    rt: Runtime,
    /// The node's simulated clients, served in order by one task.
    jobs: mpsc::UnboundedSender<Job>,
    store: Arc<SimStore>,
    /// Steps left frozen: not run at all, as in a long pause.
    paused: u32,
    /// Steps left cut off from the store, and the acquisition count and
    /// clock when the outage began.
    down: u32,
    down_from: (u64, u64),
    /// Set once an orderly shutdown has finished.
    shutdown: Option<Arc<AtomicBool>>,
}

/// What a node's clients ask of its registry.
enum Job {
    /// Requests sent in order by one client: `(id, request)`.
    Send(usize, Vec<(u64, Request)>),
    /// Subscribe to an event from a `seq`, with a hold token.
    Subscribe(
        usize,
        u64,
        Option<TokenHash>,
        Slot<Result<Subscription, OpenError>>,
    ),
    /// Read the staff state of an event from a `seq` (08 §8.3).
    Manager(usize, u64, Slot<StaffRead>),
    /// An orderly shutdown; the flag is set once it has finished.
    Shutdown(Arc<AtomicBool>),
}

/// Serve a node's clients one job at a time. One task, not one per client:
/// tokio's `watch` wakes waiters in an order drawn from a per-runtime random
/// seed (`BigNotify`), so two clients waiting for the same event to load
/// would reach its actor in a different order from run to run. Replies are
/// awaited by tasks of their own, each the only waiter on its channel.
async fn clients(
    reg: Reg,
    mut jobs: mpsc::UnboundedReceiver<Job>,
    results: Arc<Mutex<Vec<(u64, Reply)>>>,
    submitted: Arc<Mutex<HashSet<u64>>>,
    progress: Arc<AtomicU64>,
) {
    while let Some(job) = jobs.recv().await {
        progress.fetch_add(1, Ordering::Relaxed);
        match job {
            Job::Send(event, batch) => {
                for (id, request) in batch {
                    match reg.submit(EVENTS[event], request).await {
                        Ok(pending) => {
                            // In the actor's mailbox now: its fate is the actor's.
                            lock(&submitted).insert(id);
                            let (results, progress) = (results.clone(), progress.clone());
                            tokio::spawn(async move {
                                let reply = pending.await;
                                progress.fetch_add(1, Ordering::Relaxed);
                                lock(&results).push((id, reply));
                            });
                        }
                        Err(f) => lock(&results).push((id, Err(f))),
                    }
                }
            }
            Job::Shutdown(done) => {
                reg.shutdown().await;
                done.store(true, Ordering::SeqCst);
            }
            Job::Subscribe(event, last, token, out) => {
                let r = reg.subscribe(EVENTS[event], last, token).await;
                progress.fetch_add(1, Ordering::Relaxed);
                *lock(&out) = Some(r);
            }
            Job::Manager(event, after, out) => {
                let r = reg.manager_after(EVENTS[event], after).await;
                progress.fetch_add(1, Ordering::Relaxed);
                *lock(&out) = Some(r);
            }
        }
    }
}

/// One client request.
struct Req {
    event: usize,
    node: usize,
    request: Request,
    /// Requests sent in order by one client; their commits must keep it.
    pipeline: u64,
    /// The first request with the same key and fingerprint.
    family: Option<u64>,
    /// The same key as `family` with another fingerprint, sent after the
    /// family had its reply: must be `idempotency_mismatch`.
    probe: bool,
    /// The lowest committed `seq` the command could have observed.
    lo: u64,
    /// The committed `seq` when its reply arrived.
    hi: u64,
    /// The lowest the node's clock was between sending and the reply.
    clock_lo: Timestamp,
    reply: Option<Reply>,
    /// Its node crashed first.
    lost: bool,
}

/// A subscriber, as a renderer applying frames (08 §8.6).
#[derive(Default)]
struct Sub {
    event: usize,
    token: Option<TokenHash>,
    live: Option<Subscription>,
    pending: Option<(usize, Slot<Result<Subscription, OpenError>>)>,
    seq: Option<u64>,
    codes: Vec<u8>,
    mine: Option<(u64, Vec<u32>)>,
    stalled: u32,
}

/// A staff client polling the manager state of an event (08 §8.3), as the
/// console does: domain statuses, from its `seq`.
#[derive(Default)]
struct Staff {
    event: usize,
    pending: Option<(usize, Slot<StaffRead>)>,
    seq: Option<u64>,
    codes: Vec<u8>,
}

/// What a staff read answers: nothing new, or a frame.
type StaffRead = Result<Option<Frame>, OpenError>;

/// What the harness knows of one event's committed history.
#[derive(Default)]
struct Ev {
    replay: Replay,
    /// The committed state at each batch boundary, as subscribers see it.
    history: BTreeMap<u64, Replay>,
    tokens: HashMap<HoldId, TokenHash>,
    log: Vec<LogRecord>,
    logged: HashMap<Arc<str>, u64>,
    last_epoch: u64,
    last_node: Option<usize>,
}

struct Sim {
    rng: Rng,
    db: MemoryStore,
    charts: Arc<MemoryCharts>,
    shared: Arc<Mutex<Shared>>,
    config: RegistryConfig,
    clocks: Vec<SimClock>,
    /// Bumped by everything an engine task does; see [`SimClock`].
    progress: Arc<AtomicU64>,
    /// How far each node's clock has moved forward in all, jumps back aside.
    forward: Vec<u64>,
    nodes: Vec<Node>,
    results: Arc<Mutex<Vec<(u64, Reply)>>>,
    /// Requests a node's registry has put in an actor's mailbox.
    submitted: Arc<Mutex<HashSet<u64>>>,
    /// Requests waiting on an actor when its commit failed, panicked or lost
    /// its reply: each must get `unavailable` (08 §4.3, "Stopping").
    doomed: HashSet<u64>,
    reqs: Vec<Req>,
    pipelines: u64,
    subs: Vec<Sub>,
    staff: Vec<Staff>,
    evs: Vec<Ev>,
    seen: usize,
    /// Commands of batches that failed or died with their node: never visible.
    failed: HashSet<Arc<str>>,
    /// Commands of batches written whose reply was lost.
    lost: HashSet<Arc<str>>,
    /// `(event, epoch)` of actors that failed or died: they keep their lease.
    dead: HashSet<(EventId, u64)>,
    step: u64,
    /// Steps left of a burst: steady traffic, every commit succeeding, while
    /// a subscriber has stopped reading, so that it falls behind and lags.
    burst: u32,
    /// Percent chance per step that a waiting commit is decided.
    decide_pct: u64,
    fault_weights: [u64; 4],
    trace: TraceHash,
    stats: Stats,
}

/// Panics the harness injects are expected; keep them out of test output.
fn quiet_injected_panics() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let msg = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_default();
            if !msg.starts_with("sim: injected") {
                default(info);
            }
        }));
    });
}

/// Run one seed for `steps` steps, then let everything settle and check the
/// whole run. Returns the trace hash.
pub fn simulate(seed: u64, steps: usize, stats: &mut Stats) -> Result<u64, String> {
    quiet_injected_panics();
    let mut sim = Sim::new(seed).map_err(|v| format!("setup: {v}"))?;
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sim.run(steps)));
    let result = match run {
        Ok(r) => r.map_err(|v| format!("step {}: {v}", sim.step)),
        Err(_) => Err(format!("step {}: the harness panicked", sim.step)),
    };
    stats.merge(&sim.stats);
    result.map(|()| sim.trace.0)
}

fn is_unavailable(r: &Reply) -> bool {
    matches!(r, Err(Failure::Unavailable | Failure::Overloaded))
}

fn outcome_seq(o: &Outcome) -> u64 {
    match o {
        Outcome::HoldCreated { seq, .. }
        | Outcome::Held { seq, .. }
        | Outcome::Released { seq, .. }
        | Outcome::Extended { seq, .. }
        | Outcome::Booked { seq, .. }
        | Outcome::StatusChanged { seq, .. } => *seq,
    }
}

impl Sim {
    fn new(seed: u64) -> Result<Self, Violation> {
        let mut rng = Rng::new(seed);
        let db = MemoryStore::with_clock(ManualClock::new(T0));
        for e in EVENTS {
            db.create_event(e, 0, CHART);
        }
        let charts = MemoryCharts::new();
        charts.insert(CHART, chart(OBJECTS)?);
        let small = rng.chance(50);
        let config = RegistryConfig {
            actor: ActorConfig {
                mailbox: if small { 6 } else { 10_000 },
                max_batch: if small { 3 } else { 2_000 },
                ..ActorConfig::default()
            },
            retry_ms: 1_000,
        };
        let progress: Arc<AtomicU64> = Arc::default();
        let mut sim = Self {
            decide_pct: 20 + rng.below(70),
            // A third of the seeds keep commits healthy, so that actors live
            // long enough for subscribers to fall behind and lag.
            fault_weights: if rng.chance(33) {
                [300, 0, 0, 0]
            } else {
                [300, rng.below(12), rng.below(12), rng.below(3)]
            },
            rng,
            db,
            charts: Arc::new(charts),
            shared: Arc::default(),
            config,
            clocks: (0..NODES)
                .map(|_| SimClock::new(T0, progress.clone()))
                .collect(),
            forward: vec![0; NODES],
            progress,
            nodes: Vec::new(),
            results: Arc::default(),
            submitted: Arc::default(),
            doomed: HashSet::new(),
            reqs: Vec::new(),
            pipelines: 0,
            subs: Vec::new(),
            staff: (0..EVENTS.len())
                .map(|event| Staff {
                    event,
                    codes: vec![0; OBJECTS as usize],
                    ..Staff::default()
                })
                .collect(),
            evs: EVENTS.iter().map(|_| Ev::default()).collect(),
            seen: 0,
            failed: HashSet::new(),
            lost: HashSet::new(),
            dead: HashSet::new(),
            burst: 0,
            step: 0,
            trace: TraceHash::default(),
            stats: Stats::default(),
        };
        for ev in &mut sim.evs {
            ev.history.insert(0, Replay::default());
        }
        for id in 0..NODES {
            let node = sim.start(id)?;
            sim.nodes.push(node);
        }
        Ok(sim)
    }

    fn start(&self, id: usize) -> Result<Node, Violation> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .map_err(|e| Violation::new("setup", e.to_string()))?;
        let store = Arc::new(SimStore {
            inner: self.db.peer(&format!("n{id}")),
            node: id,
            shared: self.shared.clone(),
            progress: self.progress.clone(),
        });
        let clock = self
            .clocks
            .get(id)
            .cloned()
            .ok_or_else(|| Violation::new("setup", "no clock"))?;
        let reg = Registry::new(
            store.clone(),
            self.charts.clone(),
            clock,
            self.config.clone(),
        );
        let (jobs, rx) = mpsc::unbounded_channel();
        rt.spawn(clients(
            reg.clone(),
            rx,
            self.results.clone(),
            self.submitted.clone(),
            self.progress.clone(),
        ));
        Ok(Node {
            jobs,
            rt,
            store,
            paused: 0,
            down: 0,
            down_from: (0, 0),
            shutdown: None,
        })
    }

    fn run(&mut self, steps: usize) -> Result<(), Violation> {
        for _ in 0..steps {
            self.one_step()?;
        }
        self.quiesce()?;
        let fenced = lock(&self.shared).fenced;
        self.stats.add("fenced commits and heartbeats", fenced);
        self.stats.runs += 1;
        self.serial_replay()
    }

    fn node_mut(&mut self, n: usize) -> Result<&mut Node, Violation> {
        self.nodes
            .get_mut(n)
            .ok_or_else(|| Violation::new("harness", "no such node"))
    }

    fn one_step(&mut self) -> Result<(), Violation> {
        self.step += 1;
        lock(&self.shared).step = self.step;
        self.stats.steps += 1;
        if self.burst > 0 {
            self.burst -= 1;
            self.send()?;
            self.decide(true);
            self.advance(100);
            self.settle();
            return self.observe();
        }
        for _ in 0..self.rng.below(3) {
            self.send()?;
        }
        let n = self.rng.below(NODES as u64) as usize;
        let mut ms = [0, 10, 50, 100, 100, 250, 1_000][self.rng.below(7) as usize];
        let action = self.rng.weighted(&[3, 4, 6, 3, 30, 3, 3, 1, 10, 937]);
        self.trace.line(format_args!(
            "step {} node {n} action {action} +{ms}",
            self.step
        ));
        match action {
            0 => self.crash(n)?,
            1 if self.nodes.iter().all(|x| x.paused == 0) => {
                self.node_mut(n)?.paused = 20 + self.rng.below(60) as u32;
                self.stats.count("fault: node frozen");
            }
            2 => {
                let jump = self.rng.below(25_000) as i64 - 5_000;
                if let Some(clock) = self.clocks.get(n) {
                    clock.set(Timestamp(clock.now().0.saturating_add_signed(jump)));
                }
                if let Some(f) = self.forward.get_mut(n) {
                    *f += jump.max(0) as u64;
                }
                self.stats.count(if jump < 0 {
                    "fault: clock jump back"
                } else {
                    "fault: clock jump forward"
                });
                self.clock_moved(n);
            }
            3 => {
                let from = (
                    self.node_mut(n)?.store.inner.acquisitions(),
                    self.forward.get(n).copied().unwrap_or(0),
                );
                let steps = 5 + self.rng.below(25) as u32;
                let node = self.node_mut(n)?;
                if node.down == 0 {
                    node.down = steps;
                    node.down_from = from;
                    node.store.inner.set_down(true);
                    self.stats.count("fault: store outage");
                    self.bury(n);
                }
            }
            4 => self.subscribe(None)?,
            5 => {
                let live: Vec<usize> = (0..self.subs.len())
                    .filter(|&i| {
                        self.subs
                            .get(i)
                            .is_some_and(|s| s.event == 0 && s.live.is_some())
                    })
                    .collect();
                if let Some(s) = self.rng.pick(&live).and_then(|i| self.subs.get_mut(i)) {
                    s.stalled = 150;
                    self.burst = 150;
                    self.stats.count("burst with a stalled subscriber");
                }
            }
            6 => {
                ms = 11 * 60_000;
                self.stats.count("idle jump");
            }
            7 if self.nodes.iter().all(|x| x.shutdown.is_none()) => {
                let done = Arc::new(AtomicBool::new(false));
                let node = self.node_mut(n)?;
                let _ = node.jobs.send(Job::Shutdown(done.clone()));
                node.shutdown = Some(done);
                self.stats.count("orderly shutdown");
            }
            8 => {
                ms = 16_000 + self.rng.below(15_000);
                self.stats.count("lease-length jump");
            }
            _ => {}
        }
        self.decide(false);
        self.advance(ms);
        self.settle();
        self.observe()
    }

    /// One client sending one to four requests in order to one node.
    fn send(&mut self) -> Result<(), Violation> {
        let event = usize::from(self.burst == 0 && self.rng.chance(20));
        let owner = self.db.lease(EVENTS[event]).and_then(|(o, _)| o);
        let preferred = owner
            .as_deref()
            .and_then(|o| o.strip_prefix('n'))
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(0);
        let frozen = self.nodes.get(preferred).is_some_and(|n| n.paused > 0);
        let node = if self.rng.chance(if frozen { 40 } else { 85 }) {
            preferred
        } else {
            self.rng.below(NODES as u64) as usize
        };
        let lo = if owner.as_deref() == Some(&format!("n{node}")) {
            self.evs.get(event).map_or(0, |e| e.replay.seq)
        } else {
            0
        };
        self.pipelines += 1;
        let mut batch = Vec::new();
        for _ in 0..=self.rng.below(4) {
            let id = self.reqs.len() as u64;
            let (request, family, probe) = self.request(id, event);
            batch.push((id, request.clone()));
            self.reqs.push(Req {
                event,
                node,
                request,
                pipeline: self.pipelines,
                family,
                probe,
                lo,
                hi: 0,
                clock_lo: self.clocks.get(node).map_or(T0, Clock::now),
                reply: None,
                lost: false,
            });
        }
        let _ = self.node_mut(node)?.jobs.send(Job::Send(event, batch));
        Ok(())
    }

    /// A request: fresh, keyed, a duplicate or retry of a keyed one, or a
    /// probe reusing a key with another request.
    fn request(&mut self, id: u64, event: usize) -> (Request, Option<u64>, bool) {
        let mut request = gen_request(&mut self.rng, OBJECTS);
        if self.burst > 0 {
            // Block or free one seat: nearly always a change, so a publication.
            let i = self.rng.below(u64::from(OBJECTS)) as u32;
            request.authority = Authority::Secret;
            request.actor = format!("r{id}").into();
            request.command = Command::ChangeStatus {
                objects: vec![cavea_engine::ObjectRef::Idx(i)],
                status: cavea_engine::Status::from_code(3 * self.rng.below(2) as u8)
                    .unwrap_or(cavea_engine::Status::FREE),
            };
            return (request, None, false);
        }
        if self.rng.chance(70) {
            request.command = self.plain();
        }
        request.actor = format!("r{id}").into();
        let roll = self.rng.below(100);
        let keyed: Vec<u64> = self
            .reqs
            .iter()
            .enumerate()
            .filter(|(i, r)| r.event == event && r.family == Some(*i as u64))
            .map(|(i, _)| i as u64)
            .rev()
            .take(8)
            .collect();
        if roll < 12 && !keyed.is_empty() {
            let f = keyed
                .get(self.rng.below(keyed.len() as u64) as usize)
                .copied()
                .unwrap_or(0);
            if let Some(orig) = self.reqs.get(f as usize) {
                let delivered = orig.reply.as_ref().is_some_and(|r| !is_unavailable(r));
                if roll < 3 && delivered {
                    self.stats.count("idempotency probes");
                    request.idempotency = orig.request.idempotency.clone().map(|k| Idempotency {
                        fingerprint: [1; 32],
                        ..k
                    });
                    return (request, Some(f), true);
                }
                let retry = orig.reply.as_ref().is_some_and(is_unavailable);
                self.stats.count(if retry {
                    "fault: retry after unavailable"
                } else {
                    "fault: duplicate delivery"
                });
                let mut dup = orig.request.clone();
                dup.actor = request.actor;
                return (dup, Some(f), false);
            }
        }
        if roll < 30 && request.authority == Authority::Secret {
            request.idempotency = Some(Idempotency {
                key: format!("k{id}").into(),
                fingerprint: [0; 32],
                checked_at: self.clocks.first().map_or(T0, Clock::now),
            });
            return (request, Some(id), false);
        }
        // Mostly tokens that exist, as in the model test.
        if let Some(t) = token_of(&request.command)
            && self.rng.chance(40)
        {
            request.command = Command::CreateHold {
                token: t,
                ttl_secs: 900,
                max_objects: None,
            };
        }
        if self.rng.chance(5) {
            request.command = Command::CreateHold {
                token: token(9),
                ttl_secs: 60,
                max_objects: Some(1),
            };
        }
        (request, None, false)
    }

    /// A well-formed command on a few contended seats and tokens: most
    /// traffic, so that holds, bookings and expiries happen under faults.
    fn plain(&mut self) -> Command {
        let t = token(self.rng.below(4) as u8 + 1);
        let i = self.rng.below(u64::from(OBJECTS)) as u32;
        let objects = [
            cavea_engine::ObjectRef::Idx(i),
            cavea_engine::ObjectRef::Idx((i + 1) % OBJECTS),
        ];
        let one = objects
            .get(..=self.rng.below(2) as usize)
            .unwrap_or_default()
            .to_vec();
        match self.rng.weighted(&[20, 35, 15, 10, 5, 5, 10]) {
            0 => Command::CreateHold {
                token: t,
                ttl_secs: [60, 120, 900][self.rng.below(3) as usize],
                max_objects: None,
            },
            1 => Command::Hold {
                token: t,
                objects: one,
            },
            2 => Command::Book {
                token: Some(t),
                objects: Vec::new(),
                order_ref: "o".into(),
                details: Vec::new(),
            },
            3 => Command::Release {
                token: t,
                objects: one,
            },
            4 => Command::ExtendHold {
                token: t,
                ttl_secs: 60,
            },
            5 => Command::ReleaseBooked { objects: one },
            _ => Command::ChangeStatus {
                objects: one,
                status: cavea_engine::Status::from_code([0, 3][self.rng.below(2) as usize])
                    .unwrap_or(cavea_engine::Status::FREE),
            },
        }
    }

    /// Decide waiting commits: some (or, with `all`, every one) commit, and
    /// a few fail, lose their reply or panic.
    fn decide(&mut self, all: bool) {
        let mut shared = lock(&self.shared);
        shared.gates.sort_by_key(|g| (g.node, g.event.0, g.seq));
        let gates = std::mem::take(&mut shared.gates);
        let step = shared.step;
        drop(shared);
        let mut keep = Vec::new();
        for gate in gates {
            if !all && !self.rng.chance(self.decide_pct) {
                keep.push(gate);
                continue;
            }
            let decision = if all {
                Decision::Commit
            } else {
                [
                    Decision::Commit,
                    Decision::Fail,
                    Decision::LoseReply,
                    Decision::Panic,
                ][self.rng.weighted(&self.fault_weights)]
            };
            if step.saturating_sub(gate.since) >= 3 {
                self.stats.count("fault: slow commit");
            }
            match decision {
                Decision::Commit => {}
                Decision::Fail | Decision::Panic => {
                    self.failed.extend(gate.actors.iter().cloned());
                    self.stats.count(if decision == Decision::Fail {
                        "fault: commit fails"
                    } else {
                        "fault: panic in commit"
                    });
                }
                Decision::LoseReply => {
                    self.lost.extend(gate.actors.iter().cloned());
                    self.stats.count("fault: reply lost after commit");
                }
            }
            if decision != Decision::Commit {
                self.dead.insert((gate.event, gate.epoch));
                // Everyone in the actor's batches and mailbox, keyed refusals
                // and no-ops in a batch without log rows included.
                let submitted = lock(&self.submitted);
                let waiting = self.reqs.iter().enumerate().filter(|(id, r)| {
                    r.node == gate.node
                        && EVENTS.get(r.event) == Some(&gate.event)
                        && r.reply.is_none()
                        && !r.lost
                        && submitted.contains(&(*id as u64))
                });
                self.doomed.extend(waiting.map(|(id, _)| id as u64));
            }
            let _ = gate.tx.send(decision);
        }
        lock(&self.shared).gates.extend(keep);
    }

    fn crash(&mut self, n: usize) -> Result<(), Violation> {
        let in_flight: Vec<Gate> = {
            let mut shared = lock(&self.shared);
            let (dead, live) = std::mem::take(&mut shared.gates)
                .into_iter()
                .partition(|g| g.node == n);
            shared.gates = live;
            dead
        };
        if !in_flight.is_empty() {
            self.stats.count("fault: crash with a commit in flight");
        }
        for g in in_flight {
            self.failed.extend(g.actors);
        }
        self.bury(n);
        self.replace(n)?;
        self.stats.count("fault: node crash");
        Ok(())
    }

    /// A new process for node `n`: everything the old one ran is gone.
    fn replace(&mut self, n: usize) -> Result<(), Violation> {
        let fresh = self.start(n)?;
        let old = std::mem::replace(self.node_mut(n)?, fresh);
        drop(old);
        for r in self
            .reqs
            .iter_mut()
            .filter(|r| r.node == n && r.reply.is_none())
        {
            r.lost = true;
        }
        for s in &mut self.subs {
            if s.pending.as_ref().is_some_and(|(node, _)| *node == n) {
                s.pending = None;
            }
        }
        for s in &mut self.staff {
            if s.pending.as_ref().is_some_and(|(node, _)| *node == n) {
                s.pending = None;
            }
        }
        Ok(())
    }

    /// The actors node `n` runs now may die without releasing their lease.
    fn bury(&mut self, n: usize) {
        for e in EVENTS {
            let mine = self
                .db
                .lease(e)
                .and_then(|(o, _)| o)
                .is_some_and(|o| *o == *format!("n{n}"));
            let epoch = poll_now(self.db.load_event(e))
                .and_then(Result::ok)
                .map(|l| l.epoch);
            if let (true, Some(epoch)) = (mine, epoch) {
                self.dead.insert((e, epoch));
            }
        }
    }

    fn advance(&mut self, ms: u64) {
        self.db.clock().advance_ms(ms);
        for c in &self.clocks {
            c.advance_ms(ms);
        }
        for f in &mut self.forward {
            *f += ms;
        }
    }

    /// A node's clock went back: requests in flight there may be applied
    /// at the earlier time.
    fn clock_moved(&mut self, n: usize) {
        let now = self.clocks.get(n).map_or(T0, Clock::now);
        for r in self
            .reqs
            .iter_mut()
            .filter(|r| r.node == n && r.reply.is_none() && !r.lost)
        {
            r.clock_lo = r.clock_lo.min(now);
        }
    }

    /// Run every node that is not frozen until its tasks are idle.
    fn settle(&mut self) {
        for node in &mut self.nodes {
            if node.paused > 0 {
                node.paused -= 1;
                continue;
            }
            let progress = &self.progress;
            node.rt.block_on(async {
                let mut idle = 0;
                for _ in 0..SETTLE_ROUNDS {
                    let before = progress.load(Ordering::Relaxed);
                    for _ in 0..4 {
                        tokio::task::yield_now().await;
                    }
                    idle = if progress.load(Ordering::Relaxed) == before {
                        idle + 1
                    } else {
                        0
                    };
                    if idle == IDLE_ROUNDS {
                        break;
                    }
                }
            });
        }
    }

    fn load(&self, event: usize) -> Result<LoadedEvent, Violation> {
        poll_now(self.db.load_event(EVENTS[event]))
            .ok_or_else(|| Violation::new("harness", "load waited"))?
            .map_err(|e| Violation::new("harness", e.to_string()))
    }

    /// Everything that happened in the last step, checked.
    fn observe(&mut self) -> Result<(), Violation> {
        let mut new: Vec<(usize, Batch)> = lock(&self.shared)
            .committed
            .get(self.seen..)
            .unwrap_or_default()
            .to_vec();
        self.seen += new.len();
        new.sort_by_key(|(_, b)| b.event.0);
        let touched: HashSet<usize> = new
            .iter()
            .filter_map(|(_, b)| EVENTS.iter().position(|e| *e == b.event))
            .collect();
        for (node, batch) in new {
            self.on_batch(node, batch)?;
        }
        let mut touched: Vec<usize> = touched.into_iter().collect();
        touched.sort_unstable();
        for e in touched {
            let loaded = self.load(e)?;
            if let Some(ev) = self.evs.get(e) {
                diff_loaded(&ev.replay, &loaded)?;
            }
            self.stats.count("INV-6 store vs log");
        }
        let mut replies = std::mem::take(&mut *lock(&self.results));
        replies.sort_by_key(|(id, _)| *id);
        for (id, reply) in replies {
            self.on_reply(id, reply)?;
        }
        self.pump()?;
        self.chores()
    }

    /// INV-5, INV-6, INV-10 (replay), INV-11, and nothing applied twice or
    /// after its batch failed.
    fn on_batch(&mut self, node: usize, batch: Batch) -> Result<(), Violation> {
        self.trace.line(format_args!("{node} {batch:?}"));
        let e = EVENTS.iter().position(|e| *e == batch.event).unwrap_or(0);
        let by = lock(&self.shared)
            .acquired
            .get(&(batch.event.0, batch.epoch))
            .copied();
        let Some(ev) = self.evs.get_mut(e) else {
            return Ok(());
        };
        ensure!(
            by == Some(node),
            "INV-11",
            "node {node} committed epoch {} acquired by {by:?}",
            batch.epoch
        );
        ensure!(
            batch.epoch >= ev.last_epoch,
            "INV-11",
            "epoch {} after {}",
            batch.epoch,
            ev.last_epoch
        );
        if ev.last_node.is_some_and(|n| n != node) {
            self.stats.count("fault: takeover by the other node");
        }
        ev.last_node = Some(node);
        ev.last_epoch = batch.epoch;
        for r in &batch.log {
            ev.replay.apply(r)?;
            if &*r.actor != "system" {
                ensure!(
                    !ev.logged.contains_key(&r.actor),
                    "applied twice",
                    "{} at {} and {}",
                    r.actor,
                    ev.logged.get(&r.actor).copied().unwrap_or(0),
                    r.seq
                );
                ensure!(
                    !self.failed.contains(&r.actor),
                    "failed batch visible",
                    "{} at {}",
                    r.actor,
                    r.seq
                );
                ev.logged.insert(r.actor.clone(), r.seq);
            }
            ev.log.push(r.clone());
        }
        ensure!(
            batch.seq == ev.replay.seq,
            "INV-5",
            "batch seq {}, log ends at {}",
            batch.seq,
            ev.replay.seq
        );
        for h in &batch.holds_created {
            ev.tokens.insert(h.id, h.token_hash);
        }
        ev.history.insert(batch.seq, ev.replay.clone());
        self.stats.count("batches committed");
        self.stats.count("INV-5, INV-10, INV-11 checked");
        if batch.log.len() >= 2 {
            self.stats.count("batches of 2+ commands");
        }
        Ok(())
    }

    /// INV-7 as each reply arrives: nothing acknowledged before its commit,
    /// every acknowledged change in the store.
    fn on_reply(&mut self, id: u64, reply: Reply) -> Result<(), Violation> {
        self.trace.line(format_args!("reply {id} {reply:?}"));
        self.stats.count(reply_kind(&reply));
        let stored = match self.reqs.get(id as usize) {
            Some(r) if r.request.idempotency.is_some() => Some(self.load(r.event)?),
            _ => None,
        };
        let Some(req) = self.reqs.get_mut(id as usize) else {
            return Ok(());
        };
        let Some(ev) = self.evs.get(req.event) else {
            return Ok(());
        };
        req.hi = ev.replay.seq;
        let logged = ev.logged.get(&req.request.actor).copied();
        // 13 §3.2: a caller whose batch failed, or was written but lost its
        // reply, gets an error, never a success; so does every other caller
        // waiting on that actor.
        let dead = self.failed.contains(&req.request.actor)
            || self.lost.contains(&req.request.actor)
            || self.doomed.contains(&id);
        ensure!(
            !dead || reply == Err(Failure::Unavailable),
            "failed batch acknowledged",
            "r{id} got {reply:?} from a batch that did not commit for it"
        );
        match &reply {
            Ok(o) => {
                let s = outcome_seq(o);
                ensure!(
                    s <= ev.replay.seq,
                    "INV-7",
                    "r{id} acknowledged at seq {s}, store at {}",
                    ev.replay.seq
                );
                if let Some(x) = logged {
                    ensure!(
                        x == s,
                        "INV-7",
                        "r{id} acknowledged as seq {s}, logged at {x}"
                    );
                }
                let always = matches!(
                    req.request.command,
                    Command::CreateHold { .. }
                        | Command::ExtendHold { .. }
                        | Command::Book { .. }
                        | Command::ReleaseBooked { .. }
                );
                if always && req.family.is_none() {
                    ensure!(
                        logged.is_some(),
                        "INV-7",
                        "r{id} acknowledged {o:?} but not in the store"
                    );
                }
                self.stats.count("INV-7 checked");
            }
            Err(Failure::Unavailable) if logged.is_some() => {
                ensure!(
                    self.lost.contains(&req.request.actor),
                    "INV-7",
                    "r{id} unavailable but committed"
                );
            }
            Err(_) => {}
        }
        if let (Some(k), false) = (
            &req.request.idempotency,
            is_unavailable(&reply) || reply == Err(Failure::IdempotencyMismatch),
        ) {
            let loaded = stored.unwrap_or_else(|| LoadedEvent {
                epoch: 0,
                seq: 0,
                holds: vec![],
                objects: vec![],
                idempotency: vec![],
            });
            ensure!(
                loaded.idempotency.iter().any(|r| r.key == k.key),
                "INV-7",
                "r{id}: keyed reply without a stored record"
            );
        }
        req.reply = Some(reply);
        Ok(())
    }

    /// Start a subscriber, or resubscribe `existing` from its `seq`.
    fn subscribe(&mut self, existing: Option<usize>) -> Result<(), Violation> {
        let i = match existing {
            Some(i) => i,
            None => {
                let event = usize::from(self.rng.chance(20));
                let token = self
                    .rng
                    .chance(50)
                    .then(|| token(self.rng.below(4) as u8 + 1));
                self.subs.push(Sub {
                    event,
                    token,
                    codes: vec![0; OBJECTS as usize],
                    ..Sub::default()
                });
                self.stats.count("subscribers");
                self.subs.len() - 1
            }
        };
        let Some(s) = self.subs.get(i) else {
            return Ok(());
        };
        let (event, token) = (s.event, s.token);
        // Resume from where it is, or now and then from another seq it has
        // not got (a cached snapshot's), which must still work.
        let last = match s.seq {
            Some(seq) if !self.rng.chance(10) => seq,
            _ => 0,
        };
        let owner = self.db.lease(EVENTS[event]).and_then(|(o, _)| o);
        let n = owner
            .as_deref()
            .and_then(|o| o.strip_prefix('n'))
            .and_then(|n| n.parse().ok())
            .filter(|_| self.rng.chance(70))
            .unwrap_or_else(|| self.rng.below(NODES as u64) as usize);
        let slot: Slot<Result<Subscription, OpenError>> = Arc::default();
        let job = Job::Subscribe(event, last, token, slot.clone());
        let _ = self.node_mut(n)?.jobs.send(job);
        if let Some(s) = self.subs.get_mut(i) {
            s.pending = Some((n, slot));
            if last == 0 {
                s.seq = None;
            }
        }
        Ok(())
    }

    /// Apply what each subscriber has; INV-8 and INV-9 on every frame.
    fn pump(&mut self) -> Result<(), Violation> {
        for i in 0..self.subs.len() {
            let mut resubscribe = false;
            let Some(s) = self.subs.get_mut(i) else {
                continue;
            };
            if let Some((_, slot)) = &s.pending {
                let ready = lock(slot).take();
                match ready {
                    Some(Ok(mut sub)) => {
                        s.pending = None;
                        match sub.first.first().map(|f| f.bytes().first().copied()) {
                            Some(Some(wire::types::DELTA)) => {
                                self.stats.count("resumed from the ring")
                            }
                            Some(Some(wire::types::SNAPSHOT)) => {
                                self.stats.count("started from a snapshot")
                            }
                            _ => {}
                        }
                        let first = std::mem::take(&mut sub.first);
                        s.live = Some(sub);
                        s.mine = None;
                        for f in first {
                            apply(s, &f, &self.evs, &mut self.trace, i)?;
                            self.stats.count("INV-8, INV-9 frames checked");
                        }
                        private_sent(s)?;
                    }
                    Some(Err(_)) => {
                        s.pending = None;
                        resubscribe = self.rng.chance(50);
                    }
                    None => {}
                }
            }
            let Some(s) = self.subs.get_mut(i) else {
                continue;
            };
            if s.stalled > 0 {
                s.stalled -= 1;
                continue;
            }
            while let Some(live) = s.live.as_mut() {
                let Some(next) = poll_now(live.next()) else {
                    break;
                };
                match next {
                    Received::Frame(f) => {
                        apply(s, &f, &self.evs, &mut self.trace, i)?;
                        self.stats.count("INV-8, INV-9 frames checked");
                    }
                    Received::Lagged => {
                        self.stats.count("subscriber lagged and resynced");
                        match live.resync() {
                            Ok(frames) => {
                                s.mine = None;
                                for f in frames {
                                    apply(s, &f, &self.evs, &mut self.trace, i)?;
                                    self.stats.count("INV-8, INV-9 frames checked");
                                }
                                private_sent(s)?;
                            }
                            Err(_) => s.live = None,
                        }
                    }
                    Received::Ended(end) => {
                        self.stats.count(if end == End::Failed {
                            "subscription ended: failed"
                        } else {
                            "subscription ended: stopped"
                        });
                        s.live = None;
                    }
                }
            }
            if s.live.is_some()
                && let Some(ev) = self.evs.get(s.event)
            {
                check_private(s, ev)?;
            }
            if s.live.is_none() && s.pending.is_none() && (resubscribe || self.rng.chance(30)) {
                self.subscribe(Some(i))?;
            }
        }
        self.poll_staff()
    }

    /// Apply what each staff client got and, now and then, ask again: from
    /// its `seq`, or now and then from 0 (a fresh page).
    fn poll_staff(&mut self) -> Result<(), Violation> {
        for i in 0..self.staff.len() {
            let Some(p) = self.staff.get_mut(i) else {
                continue;
            };
            if let Some((_, slot)) = &p.pending {
                let ready = lock(slot).take();
                match ready {
                    None => continue,
                    Some(Ok(Some(frame))) => {
                        p.pending = None;
                        apply_manager(p, &frame, &self.evs)?;
                        self.stats.count("staff frames checked");
                    }
                    Some(Ok(None)) => {
                        p.pending = None;
                        self.stats.count("staff polls with nothing new");
                    }
                    Some(Err(_)) => p.pending = None,
                }
            }
            if !self.rng.chance(20) {
                continue;
            }
            let Some(p) = self.staff.get(i) else {
                continue;
            };
            let event = p.event;
            let after = p.seq.filter(|_| !self.rng.chance(10)).unwrap_or(0);
            let owner = self.db.lease(EVENTS[event]).and_then(|(o, _)| o);
            let n = owner
                .as_deref()
                .and_then(|o| o.strip_prefix('n'))
                .and_then(|n| n.parse().ok())
                .filter(|_| self.rng.chance(70))
                .unwrap_or_else(|| self.rng.below(NODES as u64) as usize);
            let slot: Slot<StaffRead> = Arc::default();
            let _ = self
                .node_mut(n)?
                .jobs
                .send(Job::Manager(event, after, slot.clone()));
            if let Some(p) = self.staff.get_mut(i) {
                p.pending = Some((n, slot));
                if after == 0 {
                    p.seq = None;
                }
            }
        }
        Ok(())
    }

    /// Outages and shutdowns that have run their course.
    fn chores(&mut self) -> Result<(), Violation> {
        for n in 0..self.nodes.len() {
            let forward = self.forward.get(n).copied().unwrap_or(0);
            let retry = self.config.retry_ms;
            let node = self.node_mut(n)?;
            if node.down > 0 {
                node.down -= 1;
                if node.down == 0 {
                    node.store.inner.set_down(false);
                    // The reload-storm guard: at most one load per event per
                    // retry period (08 §4.6).
                    let tries = node.store.inner.acquisitions() - node.down_from.0;
                    let periods = forward.saturating_sub(node.down_from.1) / retry + 2;
                    ensure!(
                        tries <= EVENTS.len() as u64 * periods,
                        "reload storm",
                        "{tries} loads in {periods} periods"
                    );
                    self.stats.count("reload-storm guard checked");
                }
            }
            let node = self.node_mut(n)?;
            if node
                .shutdown
                .as_ref()
                .is_some_and(|d| d.load(Ordering::SeqCst))
            {
                // Every lease released, except one an actor that failed kept.
                for e in EVENTS {
                    let (owner, until) = self.db.lease(e).unwrap_or((None, None));
                    let mine = owner.as_deref() == Some(&format!("n{n}"));
                    let epoch = poll_now(self.db.load_event(e))
                        .and_then(Result::ok)
                        .map_or(0, |l| l.epoch);
                    let dead = self.dead.contains(&(e, epoch));
                    ensure!(
                        dead || !mine || until.is_none(),
                        "shutdown",
                        "node {n} kept its lease on {e:?} at epoch {epoch}"
                    );
                }
                self.replace(n)?;
                self.stats.count("restarted after shutdown");
            }
        }
        Ok(())
    }

    /// Lift every fault, commit everything, and run until nothing is left in
    /// flight and every subscriber has the committed state.
    fn quiesce(&mut self) -> Result<(), Violation> {
        for n in 0..self.nodes.len() {
            let node = self.node_mut(n)?;
            node.paused = 0;
            node.down = node.down.min(1);
        }
        for s in &mut self.subs {
            s.stalled = 0;
        }
        for round in 0..600 {
            self.decide(true);
            self.advance(if round % 50 == 49 { 16_000 } else { 200 });
            self.settle();
            self.observe()?;
            let waiting = self
                .reqs
                .iter()
                .filter(|r| r.reply.is_none() && !r.lost)
                .count();
            let behind = self.subs.iter().filter(|s| {
                let committed = self.evs.get(s.event).map_or(0, |e| e.replay.seq);
                s.live.is_none() || s.seq != Some(committed)
            });
            let behind = behind.count();
            if lock(&self.shared).gates.is_empty() && waiting == 0 && behind == 0 {
                return Ok(());
            }
            if round == 599 {
                let detail: Vec<_> = self
                    .subs
                    .iter()
                    .map(|s| {
                        (
                            s.event,
                            s.seq,
                            s.live.is_some(),
                            s.pending.as_ref().map(|p| p.0),
                            self.evs.get(s.event).map(|e| e.replay.seq),
                        )
                    })
                    .collect();
                return Err(Violation::new(
                    "liveness",
                    format!(
                        "{waiting} requests unanswered, {behind} subscribers behind: {detail:?}"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// The client's-eye check: replaying the committed log through the model,
    /// in `seq` order, reproduces every reply a client received, and no hold
    /// was used at or after its expiry.
    fn serial_replay(&mut self) -> Result<(), Violation> {
        ensure!(
            self.db.committed()
                == lock(&self.shared)
                    .committed
                    .iter()
                    .map(|(_, b)| b.clone())
                    .collect::<Vec<_>>(),
            "harness",
            "store and harness disagree on what was committed"
        );
        for e in 0..EVENTS.len() {
            self.serial_replay_event(e)?;
        }
        // Pipelines: commits keep the order the client sent in.
        let mut last: HashMap<u64, u64> = HashMap::new();
        for r in &self.reqs {
            let Some(&seq) = self
                .evs
                .get(r.event)
                .and_then(|ev| ev.logged.get(&r.request.actor))
            else {
                continue;
            };
            if let Some(prev) = last.insert(r.pipeline, seq) {
                ensure!(
                    prev < seq,
                    "order",
                    "{} logged at {seq}, after a later-sent command at {prev}",
                    r.request.actor
                );
            }
        }
        if let Some(ev) = self.evs.first() {
            self.trace.line(format_args!("{:?}", ev.replay));
        }
        Ok(())
    }

    fn serial_replay_event(&mut self, e: usize) -> Result<(), Violation> {
        let Some(ev) = self.evs.get(e) else {
            return Ok(());
        };
        let mut by_actor: HashMap<Arc<str>, usize> = HashMap::new();
        for (i, r) in self.reqs.iter().enumerate().filter(|(_, r)| r.event == e) {
            by_actor.insert(r.request.actor.clone(), i);
        }
        let delivered = |r: &Req| r.reply.clone().filter(|x| !is_unavailable(x));
        // The reply each keyed family must give: the logged member's.
        let mut family_reply: HashMap<u64, Reply> = HashMap::new();
        // Unlogged replies to find a committed state for: (request, lo, hi).
        let mut open: Vec<(usize, u64, u64)> = Vec::new();
        let mut family_open: HashSet<u64> = HashSet::new();
        for (i, r) in self.reqs.iter().enumerate().filter(|(_, r)| r.event == e) {
            let Some(reply) = delivered(r) else { continue };
            if r.probe {
                ensure!(
                    reply == Err(Failure::IdempotencyMismatch),
                    "idempotency",
                    "probe r{i} got {reply:?}"
                );
                continue;
            }
            let family_logged = r.family.is_some_and(|f| {
                self.reqs.iter().any(|m| {
                    m.family == Some(f) && !m.probe && ev.logged.contains_key(&m.request.actor)
                })
            });
            if ev.logged.contains_key(&r.request.actor) || family_logged {
                continue;
            }
            match r.family {
                Some(f) if !family_open.insert(f) => {}
                // The family's reply is its original's, which may have been
                // applied as early as any member was sent.
                Some(f) => {
                    let lo = self
                        .reqs
                        .iter()
                        .filter(|m| m.family == Some(f))
                        .map(|m| m.lo)
                        .min()
                        .unwrap_or(0);
                    let hi = self
                        .reqs
                        .iter()
                        .filter(|m| m.family == Some(f) && m.reply.is_some())
                        .map(|m| m.hi)
                        .max()
                        .unwrap_or(r.hi);
                    open.push((i, lo, hi));
                }
                None => open.push((i, r.lo, r.hi)),
            }
        }
        // Family members not logged must all agree with each other.
        let mut agreed: HashMap<u64, Reply> = HashMap::new();
        for r in self.reqs.iter().filter(|r| r.event == e && !r.probe) {
            if let (Some(f), Some(reply)) = (r.family, delivered(r)) {
                let first = agreed.entry(f).or_insert_with(|| reply.clone());
                ensure!(
                    *first == reply,
                    "idempotency",
                    "{} got {reply:?}, its family {first:?}",
                    r.request.actor
                );
            }
        }
        let mut model = Model::new(OBJECTS);
        let check_open =
            |model: &Model, seq: u64, open: &mut Vec<(usize, u64, u64)>, reqs: &[Req]| {
                open.retain(|&(i, lo, hi)| {
                    let Some(r) = reqs.get(i) else { return false };
                    let reply = r.reply.clone();
                    // Only a state the command could have observed.
                    if seq < lo || seq > hi {
                        return true;
                    }
                    // A no-op names the seq it saw.
                    if let Some(Ok(o)) = &reply
                        && outcome_seq(o) != seq
                    {
                        return true;
                    }
                    let mut at = model.clone();
                    at.execute(&r.request, T0) != reply.unwrap_or(Err(Failure::Unavailable))
                });
            };
        check_open(&model, 0, &mut open, &self.reqs);
        for rec in &ev.log {
            if rec.op == cavea_engine::Op::Expire {
                let ok = rec.hold_id.is_some_and(|id| model.expire(id));
                ensure!(
                    ok,
                    "serial",
                    "seq {}: expiry of {:?}, not live",
                    rec.seq,
                    rec.hold_id
                );
            } else {
                let Some(&i) = by_actor.get(&rec.actor) else {
                    return Err(Violation::new(
                        "serial",
                        format!("seq {}: no request {}", rec.seq, rec.actor),
                    ));
                };
                let Some(r) = self.reqs.get(i) else { continue };
                let ttl = match &r.request.command {
                    Command::CreateHold { ttl_secs, .. } | Command::ExtendHold { ttl_secs, .. } => {
                        Some(*ttl_secs)
                    }
                    _ => None,
                };
                let now = match (ttl, rec.expires_at) {
                    (Some(ttl), Some(at)) => Timestamp(at.0.saturating_sub(u64::from(ttl) * 1_000)),
                    _ => T0,
                };
                // No hold is used at or after its expiry (08 §7.3).
                let used = match &r.request.command {
                    Command::CreateHold { .. } => None,
                    c => token_of(c).and(rec.hold_id),
                };
                if let Some(at) = used.and_then(|id| model.expiry_of(id)) {
                    ensure!(
                        r.clock_lo < at,
                        "expiry",
                        "{} used hold {:?} at {:?}, expiring {at:?}",
                        rec.actor,
                        rec.hold_id,
                        r.clock_lo
                    );
                    self.stats.count("expiry checked");
                }
                if ttl.is_some() {
                    ensure!(
                        now >= r.clock_lo,
                        "expiry",
                        "{} applied at {now:?} before it was sent",
                        rec.actor
                    );
                }
                let reply = model.execute(&r.request, now);
                ensure!(
                    matches!(&reply, Ok(o) if outcome_seq(o) == rec.seq),
                    "serial",
                    "seq {}: {} replays as {reply:?}",
                    rec.seq,
                    rec.actor
                );
                // A keyed request runs once: a duplicate or a retry, even
                // after a lost reply and a reload, replays the first result.
                if let Some(f) = r.family {
                    ensure!(
                        family_reply.insert(f, reply.clone()).is_none(),
                        "idempotency",
                        "{} ran again under a key already logged",
                        rec.actor
                    );
                }
                if let Some(got) = delivered(r) {
                    ensure!(
                        got == reply,
                        "serial",
                        "{} got {got:?}, serial order gives {reply:?}",
                        rec.actor
                    );
                    self.stats.count("replies replayed");
                }
            }
            let expected = model.take_log();
            ensure!(
                expected.as_slice() == std::slice::from_ref(rec),
                "serial",
                "log {rec:?}, model {expected:?}"
            );
            check_open(&model, rec.seq, &mut open, &self.reqs);
        }
        for (f, reply) in &family_reply {
            for r in self
                .reqs
                .iter()
                .filter(|r| r.family == Some(*f) && !r.probe)
            {
                if let Some(got) = delivered(r) {
                    ensure!(
                        got == *reply,
                        "idempotency",
                        "{} got {got:?}, original {reply:?}",
                        r.request.actor
                    );
                    self.stats.count("duplicates given the original reply");
                }
            }
        }
        if let Some(&(i, lo, hi)) = open.first() {
            let r = self.reqs.get(i).map(|r| {
                (
                    r.request.actor.clone(),
                    r.node,
                    r.pipeline,
                    format!("{:?}", r.request.command),
                    r.reply.clone(),
                )
            });
            return Err(Violation::new(
                "serial",
                format!("{r:?} matches no committed state in {lo}..={hi}"),
            ));
        }
        self.stats.count("serial replays");
        Ok(())
    }
}

/// Apply one frame as a renderer would, checking INV-8 and INV-9.
fn apply(
    s: &mut Sub,
    f: &Frame,
    evs: &[Ev],
    trace: &mut TraceHash,
    i: usize,
) -> Result<(), Violation> {
    trace.write(&(i as u64).to_le_bytes());
    trace.write(f.bytes());
    let Some(ev) = evs.get(s.event) else {
        return Ok(());
    };
    let frame = wire::decode_server(f.bytes(), View::Public)
        .map_err(|e| Violation::new("INV-9", format!("{e:?}")))?;
    match frame {
        ServerFrame::Snapshot(snap) => {
            let mut codes = vec![0; OBJECTS as usize];
            snap.write_into(&mut codes)
                .map_err(|e| Violation::new("INV-9", format!("{e:?}")))?;
            // INV-9: the frame is a function of public codes alone.
            let again = wire::encode_snapshot(View::Public, snap.seq, &codes, &[])
                .map_err(|e| Violation::new("INV-9", format!("{e:?}")))?;
            ensure!(
                again == f.bytes(),
                "INV-9",
                "snapshot carries more than public codes"
            );
            s.codes = codes;
            s.seq = Some(snap.seq);
        }
        ServerFrame::Delta(d) => {
            ensure!(
                s.seq == Some(d.from_seq),
                "INV-8",
                "delta from {} applied at {:?}",
                d.from_seq,
                s.seq
            );
            let changes = d
                .changes(OBJECTS)
                .map_err(|e| Violation::new("INV-9", format!("{e:?}")))?;
            for c in changes {
                if let Some(code) = s.codes.get_mut(c.idx as usize) {
                    *code = c.code;
                }
            }
            s.seq = Some(d.to_seq);
        }
        ServerFrame::Private(p) => {
            ensure!(
                s.seq == Some(f.seq()),
                "INV-8",
                "PRIVATE at {} for a client at {:?}",
                f.seq(),
                s.seq
            );
            let objects = p
                .objects(OBJECTS)
                .map_err(|e| Violation::new("INV-8", format!("{e:?}")))?;
            s.mine = Some((p.expires_at_ms, objects.collect()));
        }
        other => return Err(Violation::new("INV-8", format!("unexpected {other:?}"))),
    }
    let seq = s.seq.unwrap_or(0);
    ensure!(
        seq <= ev.replay.seq,
        "INV-8",
        "subscriber at {seq}, store at {}",
        ev.replay.seq
    );
    let Some(state) = ev.history.get(&seq) else {
        return Err(Violation::new(
            "INV-8",
            format!("subscriber at {seq}, never a committed boundary"),
        ));
    };
    ensure!(
        s.codes == public(state, OBJECTS),
        "INV-8",
        "subscriber codes at {seq} differ from the committed state"
    );
    if matches!(frame, ServerFrame::Private(_)) {
        check_private(s, ev)?;
    }
    Ok(())
}

/// Apply one manager frame as a staff client would: it carries domain
/// statuses, never encoding 2, and leaves the client exactly at a committed
/// state (INV-8).
fn apply_manager(p: &mut Staff, f: &Frame, evs: &[Ev]) -> Result<(), Violation> {
    let Some(ev) = evs.get(p.event) else {
        return Ok(());
    };
    let frame = wire::decode_server(f.bytes(), View::Manager)
        .map_err(|e| Violation::new("INV-8", format!("manager frame: {e:?}")))?;
    match frame {
        ServerFrame::Snapshot(snap) => {
            snap.write_into(&mut p.codes)
                .map_err(|e| Violation::new("INV-8", format!("{e:?}")))?;
            p.seq = Some(snap.seq);
        }
        ServerFrame::Delta(d) => {
            ensure!(
                p.seq == Some(d.from_seq),
                "INV-8",
                "manager delta from {} applied at {:?}",
                d.from_seq,
                p.seq
            );
            let changes = d
                .changes(OBJECTS)
                .map_err(|e| Violation::new("INV-8", format!("{e:?}")))?;
            for c in changes {
                if let Some(code) = p.codes.get_mut(c.idx as usize) {
                    *code = c.code;
                }
            }
            p.seq = Some(d.to_seq);
        }
        other => return Err(Violation::new("INV-8", format!("unexpected {other:?}"))),
    }
    let seq = p.seq.unwrap_or(0);
    let Some(state) = ev.history.get(&seq) else {
        return Err(Violation::new(
            "INV-8",
            format!("staff at {seq}, never a committed boundary"),
        ));
    };
    ensure!(
        p.codes == crate::model::domain(state, OBJECTS),
        "INV-8",
        "staff statuses at {seq} differ from the committed state"
    );
    Ok(())
}

/// A subscriber with a token gets a PRIVATE frame when it attaches or
/// resyncs, an empty one if the token holds nothing (08 §8.3).
fn private_sent(s: &Sub) -> Result<(), Violation> {
    ensure!(
        s.token.is_none() || s.mine.is_some(),
        "INV-8",
        "a token subscriber started at {:?} without a PRIVATE frame",
        s.seq
    );
    Ok(())
}

/// The subscriber's last PRIVATE is its token's committed hold at its `seq`.
fn check_private(s: &Sub, ev: &Ev) -> Result<(), Violation> {
    let seq = s.seq.unwrap_or(0);
    let Some(state) = ev.history.get(&seq) else {
        return Ok(());
    };
    if let (Some(t), Some(mine)) = (s.token, &s.mine) {
        let held = state
            .holds
            .iter()
            .find(|(id, _)| ev.tokens.get(id) == Some(&t));
        let expected = held.map_or((0, Vec::new()), |(_, h)| {
            let mut idx: Vec<u32> = h.objects.iter().filter_map(|&o| crate::idx_of(o)).collect();
            idx.sort_unstable();
            (h.expires_at.0, idx)
        });
        ensure!(
            *mine == expected,
            "INV-8",
            "PRIVATE at {seq}: {mine:?}, committed {expected:?}"
        );
    }
    Ok(())
}
