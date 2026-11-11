//! The realtime hub (08 §8): the node's WebSocket and SSE connections, their
//! limits and counters, their shutdown, and [`run`], the loop that feeds one
//! engine subscription to one connection through its transport's [`Link`].
//! The HTTP side (the upgrade, SUBSCRIBE, the SSE body, the snapshot and
//! polling) is in `http::realtime`.
//!
//! Streams run on a runtime of their own ([`runtime`]), apart from request
//! handling (08 §8.3, D-45); the HTTP side spawns them there with
//! [`Realtime::spawn`] once a subscription is made.
//!
//! One connection is one task. It waits on four things at once: the shutdown
//! signal, the client, the subscription and one timer, which is re-armed for
//! the next PING or the idle deadline, whichever is first. An idle connection
//! is therefore one parked task with one timer entry, woken every 25 s.
//!
//! Each frame is written and flushed before the next is taken from the
//! subscription, so a connection holds at most one frame of its own; what it
//! has not yet sent stays in the engine's broadcast channel, shared by every
//! subscriber of the event. A client that stops reading blocks the write;
//! the subscription falls behind and lags after 64 publications, and when the
//! write completes the client gets RESYNC and the current SNAPSHOT. A write
//! that does not complete within [`Timing::stall`] ends the connection.

use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt;
use std::future::Future;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use cavea_engine::{
    ChartProvider, Clock, EventId, Frame, Received, Registry, SeatStore, Subscription, TokenHash,
};
use cavea_format::wire::{self, CloseReason};
use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio::time::{Instant, sleep_until, timeout};

/// The runtime realtime connections stream on (08 §8.3, D-41): `threads`
/// workers, each under Linux's `SCHED_IDLE` policy. Request handling, actors
/// and commits run on another runtime under the default policy, so the kernel
/// runs them first whenever they are ready, and fanning out frames to
/// subscribers takes the cores they leave idle: when there are not enough,
/// it is delta delivery that slows (and publications coalesce over a longer
/// interval, `cavea_engine::MAX_WAIT_MS`), never a command.
pub fn runtime(threads: usize) -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads.max(1))
        .thread_name("cavea-realtime")
        .on_thread_start(yield_to_requests)
        .enable_all()
        .build()
}

#[cfg(target_os = "linux")]
fn yield_to_requests() {
    // SAFETY: `sched_setscheduler` reads the parameter struct we pass and
    // keeps no reference to it; an all-zero `sched_param` is a valid one
    // (priority 0, the only one `SCHED_IDLE` takes). On Linux a pid of 0
    // names the calling thread, so this changes only the runtime's own
    // threads; moving to `SCHED_IDLE` needs no privilege.
    let failed = unsafe {
        let param: libc::sched_param = std::mem::zeroed();
        libc::sched_setscheduler(0, libc::SCHED_IDLE, &raw const param)
    } != 0;
    if failed {
        tracing::warn!(
            error = %std::io::Error::last_os_error(),
            "realtime thread not moved to SCHED_IDLE; fanning out competes with requests"
        );
    }
}

#[cfg(not(target_os = "linux"))]
fn yield_to_requests() {}

/// The protocol's timers (08 §8.3). Fixed in production; tests shorten them.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// PING (an SSE comment) every this long.
    pub ping: Duration,
    /// A WebSocket whose client has sent nothing for this long is closed.
    pub idle: Duration,
    /// A WebSocket must send SUBSCRIBE within this long of the upgrade.
    pub handshake: Duration,
    /// A write that has not completed in this long ends the connection: the
    /// client has stopped reading.
    pub stall: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            ping: Duration::from_secs(25),
            idle: Duration::from_secs(60),
            handshake: Duration::from_secs(10),
            stall: Duration::from_secs(30),
        }
    }
}

/// The most messages a client may send between two PINGs: it needs one, the
/// PONG. More is CLOSE 6.
pub const MAX_MESSAGES_PER_PING: u32 = 10;
/// The largest client message: SUBSCRIBE with every field at its limit is
/// 1,812 bytes.
pub const MAX_CLIENT_MESSAGE: usize = 2048;
/// How long a CLOSE frame may take to write.
const CLOSE_WRITE: Duration = Duration::from_secs(1);

/// A long-lived transport. Polling has no connection and is not counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    WebSocket,
    Sse,
}

impl Transport {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WebSocket => "websocket",
            Self::Sse => "sse",
        }
    }
}

/// The node's realtime connections. Cheap to clone; one per server, in
/// `AppState`.
#[derive(Clone)]
pub struct Realtime {
    inner: Arc<Inner>,
}

struct Inner {
    timing: Timing,
    max_connections: usize,
    max_per_ip: u32,
    /// Set once, at shutdown.
    stop: watch::Sender<bool>,
    /// Open connections, subscribed or not.
    open: watch::Sender<usize>,
    per_ip: Mutex<HashMap<IpAddr, u32>>,
    /// Subscribed connections per [`Transport`], for
    /// `cavea_rt_subscribers{transport}`.
    subscribers: [AtomicUsize; 2],
    /// RESYNCs sent, for `cavea_rt_resyncs_total`.
    resyncs: AtomicU64,
    /// Frames written, for `cavea_rt_frames_total`.
    frames: AtomicU64,
    /// Where connections stream; `None`: the caller's runtime.
    runtime: Option<Handle>,
}

impl fmt::Debug for Realtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Realtime")
            .field("connections", &self.connections())
            .finish_non_exhaustive()
    }
}

/// Why a connection was not admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    ShuttingDown,
    /// The node holds `CAVEA_RT_MAX_CONNECTIONS`.
    Node,
    /// The client IP holds `CAVEA_RT_MAX_PER_IP`.
    Ip,
}

impl Realtime {
    /// Connections stream on the runtime of whoever spawns them.
    #[must_use]
    pub fn new(max_connections: usize, max_per_ip: u32, timing: Timing) -> Self {
        Self::build(max_connections, max_per_ip, timing, None)
    }

    /// Connections stream on `runtime`, apart from request handling (08 §8.3).
    #[must_use]
    pub fn on_runtime(
        max_connections: usize,
        max_per_ip: u32,
        timing: Timing,
        runtime: Handle,
    ) -> Self {
        Self::build(max_connections, max_per_ip, timing, Some(runtime))
    }

    fn build(
        max_connections: usize,
        max_per_ip: u32,
        timing: Timing,
        runtime: Option<Handle>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                timing,
                max_connections,
                max_per_ip,
                stop: watch::Sender::new(false),
                open: watch::Sender::new(0),
                per_ip: Mutex::default(),
                subscribers: [AtomicUsize::new(0), AtomicUsize::new(0)],
                resyncs: AtomicU64::new(0),
                frames: AtomicU64::new(0),
                runtime,
            }),
        }
    }

    /// Run a connection's stream: on the realtime runtime if there is one.
    pub fn spawn<F>(&self, stream: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        match &self.inner.runtime {
            Some(runtime) => drop(runtime.spawn(stream)),
            None => drop(tokio::spawn(stream)),
        }
    }

    #[must_use]
    pub fn timing(&self) -> Timing {
        self.inner.timing
    }

    fn per_ip(&self) -> MutexGuard<'_, HashMap<IpAddr, u32>> {
        self.inner
            .per_ip
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Take a connection slot for a client at `ip` (`None` if unknown: only
    /// the node limit applies). The slot is given back when dropped.
    pub fn admit(&self, ip: Option<IpAddr>) -> Result<Slot, Refused> {
        if *self.inner.stop.borrow() {
            return Err(Refused::ShuttingDown);
        }
        let mut per_ip = self.per_ip();
        if let Some(ip) = ip
            && per_ip.get(&ip).is_some_and(|&n| n >= self.inner.max_per_ip)
        {
            return Err(Refused::Ip);
        }
        let mut admitted = false;
        self.inner.open.send_if_modified(|n| {
            admitted = *n < self.inner.max_connections;
            *n += usize::from(admitted);
            admitted
        });
        if !admitted {
            return Err(Refused::Node);
        }
        if let Some(ip) = ip {
            *per_ip.entry(ip).or_default() += 1;
        }
        Ok(Slot {
            realtime: self.clone(),
            ip,
        })
    }

    /// Tell every connection to send CLOSE 3 and end, and admit no more
    /// (08 §14).
    pub fn shutdown(&self) {
        self.inner.stop.send_replace(true);
    }

    /// Resolves once no connection is open.
    pub async fn closed(&self) {
        let mut open = self.inner.open.subscribe();
        let _ = open.wait_for(|&n| n == 0).await;
    }

    /// Open connections, subscribed or not.
    #[must_use]
    pub fn connections(&self) -> usize {
        *self.inner.open.borrow()
    }

    /// Subscribed connections on `transport`.
    #[must_use]
    pub fn subscribers(&self, transport: Transport) -> usize {
        self.inner.subscribers[transport as usize].load(Ordering::Relaxed)
    }

    /// RESYNCs sent since start-up.
    #[must_use]
    pub fn resyncs(&self) -> u64 {
        self.inner.resyncs.load(Ordering::Relaxed)
    }

    /// Frames written since start-up.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.inner.frames.load(Ordering::Relaxed)
    }

    /// Whether shutdown has begun.
    #[must_use]
    pub fn is_stopping(&self) -> bool {
        *self.inner.stop.borrow()
    }

    /// Count a subscriber on `transport` while the guard lives.
    #[must_use]
    pub fn subscribed(&self, transport: Transport) -> Subscribed {
        self.inner.subscribers[transport as usize].fetch_add(1, Ordering::Relaxed);
        Subscribed(self.clone(), transport)
    }

    /// Resolves once shutdown has begun.
    pub async fn stopping(&self) {
        let mut stop = self.inner.stop.subscribe();
        if stop.wait_for(|&s| s).await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// One admitted connection.
#[derive(Debug)]
pub struct Slot {
    realtime: Realtime,
    ip: Option<IpAddr>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        if let Some(ip) = self.ip {
            let mut per_ip = self.realtime.per_ip();
            if let Some(n) = per_ip.get_mut(&ip) {
                *n -= 1;
                if *n == 0 {
                    per_ip.remove(&ip);
                }
            }
        }
        self.realtime
            .inner
            .open
            .send_modify(|n| *n = n.saturating_sub(1));
    }
}

/// One subscriber, counted while it lives.
#[derive(Debug)]
pub struct Subscribed(Realtime, Transport);

impl Drop for Subscribed {
    fn drop(&mut self) {
        self.0.inner.subscribers[self.1 as usize].fetch_sub(1, Ordering::Relaxed);
    }
}

/// What a connection sends.
#[derive(Debug)]
pub enum Out {
    /// A frame of the engine's, shared with every subscriber.
    Frame(Frame),
    /// A frame built for this connection: HELLO, RESYNC, CLOSE.
    Bytes(Vec<u8>),
    /// PING with this nonce (a comment on SSE).
    Ping(u64),
}

/// What a client sent after SUBSCRIBE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inbound {
    /// PONG, or anything else that is allowed and shows the client is there.
    Alive,
    /// A frame that does not decode, a second SUBSCRIBE, a text or oversized
    /// message: CLOSE 6.
    Bad,
    /// The client closed the connection.
    Gone,
}

/// The connection is closed.
#[derive(Debug, Clone, Copy)]
pub struct Gone;

/// One transport's side of a connection.
pub trait Link: Send {
    /// Whether the client can send anything. Only then is it closed for
    /// silence.
    const HEARS_CLIENT: bool;

    /// Write one frame and flush it.
    fn send(&mut self, out: Out) -> impl Future<Output = Result<(), Gone>> + Send;

    /// The client's next message. Cancel-safe.
    fn recv(&mut self) -> impl Future<Output = Inbound> + Send;
}

/// Why a connection ended, as logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The client closed it.
    ClientGone,
    /// Silent for [`Timing::idle`].
    Idle,
    /// A write did not complete within [`Timing::stall`].
    Stalled,
    /// CLOSE 6.
    Violation,
    /// CLOSE 3: the server is shutting down.
    Shutdown,
    /// CLOSE 3: the event's actor ended and could not be had again.
    Unavailable,
}

impl Ending {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClientGone => "client_closed",
            Self::Idle => "idle",
            Self::Stalled => "stalled",
            Self::Violation => "bad_frame",
            Self::Shutdown => "shutdown",
            Self::Unavailable => "unavailable",
        }
    }
}

/// What a connection did, for its closing log record.
#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub frames: u64,
    pub resyncs: u64,
}

/// Send CLOSE with `reason`, giving up after a second.
pub async fn close<L: Link>(link: &mut L, reason: CloseReason) {
    let _ = timeout(
        CLOSE_WRITE,
        link.send(Out::Bytes(wire::encode_close(reason))),
    )
    .await;
}

/// Write one frame. A write blocked by a client that does not read ends at
/// the stall limit, or at once on shutdown, without a CLOSE: it could not be
/// written either.
async fn put<L: Link>(
    realtime: &Realtime,
    link: &mut L,
    out: Out,
    stats: &mut Stats,
) -> Result<(), Ending> {
    let sent = tokio::select! {
        biased;
        () = realtime.stopping() => return Err(Ending::Shutdown),
        sent = timeout(realtime.timing().stall, link.send(out)) => sent,
    };
    match sent {
        Ok(Ok(())) => {
            stats.frames += 1;
            realtime.inner.frames.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        Ok(Err(Gone)) => Err(Ending::ClientGone),
        Err(_) => Err(Ending::Stalled),
    }
}

/// What a connection follows: the event and the subscriber's hold token.
pub struct Feed<'a, S, P, C> {
    pub registry: &'a Registry<S, P, C>,
    pub event: EventId,
    pub token: Option<TokenHash>,
}

/// Send HELLO and the subscription's first frames, then every frame it
/// publishes, until the client goes, the connection breaks a rule or times
/// out, or the server shuts down. A lagging subscription gets RESYNC and the
/// SNAPSHOT; one whose actor ended subscribes again from its `seq`, which the
/// client sees as more DELTAs or a SNAPSHOT.
pub async fn run<L, S, P, C>(
    realtime: &Realtime,
    link: &mut L,
    feed: &Feed<'_, S, P, C>,
    sub: Subscription,
    stats: &mut Stats,
) -> Ending
where
    L: Link,
    S: SeatStore,
    P: ChartProvider,
    C: Clock + Clone,
{
    match stream(realtime, link, feed, sub, stats).await {
        Err(ending) => ending,
    }
}

async fn stream<L, S, P, C>(
    realtime: &Realtime,
    link: &mut L,
    feed: &Feed<'_, S, P, C>,
    mut sub: Subscription,
    stats: &mut Stats,
) -> Result<Infallible, Ending>
where
    L: Link,
    S: SeatStore,
    P: ChartProvider,
    C: Clock + Clone,
{
    let timing = realtime.timing();
    // Only an object count over the format's limit fails, which no chart has.
    let Ok(hello) = wire::encode_hello(&sub.hello) else {
        close(link, CloseReason::SHUTTING_DOWN).await;
        return Err(Ending::Unavailable);
    };
    put(realtime, link, Out::Bytes(hello), stats).await?;
    for frame in std::mem::take(&mut sub.first) {
        put(realtime, link, Out::Frame(frame), stats).await?;
    }
    let mut heard = Instant::now();
    let mut next_ping = heard + timing.ping;
    let deadline = |heard: Instant, next_ping: Instant| {
        if L::HEARS_CLIENT {
            next_ping.min(heard + timing.idle)
        } else {
            next_ping
        }
    };
    let (mut nonce, mut messages) = (0u64, 0u32);
    let timer = sleep_until(deadline(heard, next_ping));
    tokio::pin!(timer);
    loop {
        tokio::select! {
            biased;
            () = realtime.stopping() => {
                close(link, CloseReason::SHUTTING_DOWN).await;
                return Err(Ending::Shutdown);
            }
            inbound = link.recv() => {
                messages += 1;
                match inbound {
                    Inbound::Alive if messages <= MAX_MESSAGES_PER_PING => heard = Instant::now(),
                    Inbound::Alive | Inbound::Bad => {
                        close(link, CloseReason::BAD_FRAME).await;
                        return Err(Ending::Violation);
                    }
                    Inbound::Gone => return Err(Ending::ClientGone),
                }
                timer.as_mut().reset(deadline(heard, next_ping));
            }
            // Before the feed: a feed that is always ready must not hold
            // back PING or the silence deadline.
            () = &mut timer => {
                let now = Instant::now();
                if L::HEARS_CLIENT && now >= heard + timing.idle {
                    return Err(Ending::Idle);
                }
                if now >= next_ping {
                    put(realtime, link, Out::Ping(nonce), stats).await?;
                    nonce += 1;
                    messages = 0;
                    next_ping = now + timing.ping;
                }
                timer.as_mut().reset(deadline(heard, next_ping));
            }
            received = sub.next() => {
                let frames = match received {
                    Received::Frame(frame) => {
                        put(realtime, link, Out::Frame(frame), stats).await?;
                        continue;
                    }
                    Received::Lagged => {
                        stats.resyncs += 1;
                        realtime.inner.resyncs.fetch_add(1, Ordering::Relaxed);
                        put(realtime, link, Out::Bytes(wire::encode_resync()), stats).await?;
                        sub.resync()
                    }
                    Received::Ended(_) => Err(cavea_engine::End::Failed),
                };
                let frames = match frames {
                    Ok(frames) => frames,
                    // The actor ended: its replacement resumes from `seq`.
                    Err(_) => {
                        let again = feed.registry.subscribe(feed.event, sub.seq(), feed.token).await;
                        let Ok(mut again) = again else {
                            close(link, CloseReason::SHUTTING_DOWN).await;
                            return Err(Ending::Unavailable);
                        };
                        let first = std::mem::take(&mut again.first);
                        sub = again;
                        first
                    }
                };
                for frame in frames {
                    put(realtime, link, Out::Frame(frame), stats).await?;
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]
    #![allow(
        clippy::await_holding_lock,
        reason = "the serial lock of `http::tests` is held across a whole test: the engine's \
                  log callsites are shared with its log tests"
    )]

    use std::sync::Arc;

    use cavea_engine::{
        Authority, ChartVersionId, ChartView, Command, Fault, ManualClock, MemoryCharts,
        MemoryStore, ObjectRef, Request, Status, Timestamp,
    };
    use cavea_format::wire::{ServerFrame, View};
    use tokio::sync::mpsc;

    use super::*;

    /// SUBSCRIBE with every field at its limit (08 §8.2) is a message the
    /// server reads.
    #[test]
    fn the_largest_subscribe_fits_a_client_message() {
        let key = "\u{1F600}".repeat(wire::MAX_KEY_CHARS);
        let hold = [b'h'; wire::MAX_TOKEN_BYTES];
        let session = [b's'; wire::MAX_SESSION_TOKEN_BYTES];
        let frame = wire::encode_subscribe(&wire::Subscribe {
            version: 1,
            event_key: &key,
            last_seq: u64::MAX,
            hold_token: Some(&hold),
            session_token: Some(&session),
        })
        .unwrap();
        assert_eq!(frame.len(), 1_812);
        assert!(frame.len() <= MAX_CLIENT_MESSAGE);
    }

    /// A buyer's client: applies every frame as the renderer would, and
    /// refuses a DELTA that does not continue from its `seq` (08 §8.6).
    #[derive(Debug, Default)]
    pub(crate) struct Client {
        pub(crate) seq: u64,
        pub(crate) codes: Vec<u8>,
        pub(crate) hello: Option<wire::Hello>,
        /// Frame types in arrival order.
        pub(crate) types: Vec<u8>,
        /// The last PRIVATE: expiry and objects.
        pub(crate) private: Option<(u64, Vec<u32>)>,
        pub(crate) closed: Option<u8>,
    }

    impl Client {
        pub(crate) fn apply(&mut self, frame: &[u8]) {
            self.types.push(frame[0]);
            // From HELLO, or set by the caller for a client without one.
            let objects = u32::try_from(self.codes.len()).unwrap();
            match wire::decode_server(frame, View::Public).unwrap() {
                ServerFrame::Hello(h) => {
                    assert!(self.hello.is_none(), "a second HELLO");
                    self.hello = Some(h);
                    self.codes = vec![0; h.objects as usize];
                }
                ServerFrame::Snapshot(s) => {
                    s.write_into(&mut self.codes).unwrap();
                    self.seq = s.seq;
                }
                ServerFrame::Delta(d) => {
                    assert_eq!(d.from_seq, self.seq, "a gap or a repeat");
                    for c in d.changes(objects).unwrap() {
                        self.codes[c.idx as usize] = c.code;
                    }
                    self.seq = d.to_seq;
                }
                ServerFrame::Private(p) => {
                    let held = p.objects(objects).unwrap().collect();
                    self.private = Some((p.expires_at_ms, held));
                }
                ServerFrame::Close(reason) => self.closed = Some(reason.0),
                ServerFrame::Resync | ServerFrame::Ping(_) => {}
            }
        }

        pub(crate) fn count(&self, kind: u8) -> usize {
            self.types.iter().filter(|&&t| t == kind).count()
        }
    }

    const T0: Timestamp = Timestamp(1_000_000);
    const EV: EventId = EventId(7);
    const CHART: ChartVersionId = ChartVersionId(70);
    const N: u32 = 40;
    type Reg = Registry<MemoryStore, MemoryCharts, ManualClock>;

    fn setup() -> (Arc<MemoryStore>, Reg) {
        setup_with(cavea_engine::RegistryConfig::default())
    }

    fn setup_with(config: cavea_engine::RegistryConfig) -> (Arc<MemoryStore>, Reg) {
        let store = MemoryStore::with_clock(ManualClock::new(T0));
        store.create_event(EV, 0, CHART);
        let store = Arc::new(store);
        let charts = MemoryCharts::new();
        let view = ChartView::new((0..N).map(|i| (100 + i, format!("S-{i}")))).unwrap();
        charts.insert(CHART, Arc::new(view.with_hash([9; 16])));
        let reg = Registry::new(
            store.clone(),
            Arc::new(charts),
            store.clock().clone(),
            config,
        );
        (store, reg)
    }

    fn block(idx: u32, status: Status) -> Request {
        Request {
            actor: "sk_test".into(),
            authority: Authority::Secret,
            command: Command::ChangeStatus {
                objects: vec![ObjectRef::Idx(idx)],
                status,
            },
            idempotency: None,
        }
    }

    /// Commit, then let the actor publish.
    async fn commit(store: &MemoryStore, reg: &Reg, idx: u32, status: Status) {
        let _ = reg.send(EV, block(idx, status)).await;
        store.clock().advance_ms(500);
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
    }

    struct Mock {
        out: mpsc::Sender<Vec<u8>>,
        inbound: mpsc::UnboundedReceiver<Inbound>,
        /// How long each write takes.
        delay: Duration,
    }

    impl Link for Mock {
        const HEARS_CLIENT: bool = true;

        async fn send(&mut self, out: Out) -> Result<(), Gone> {
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            let bytes = match out {
                Out::Frame(f) => f.bytes().to_vec(),
                Out::Bytes(b) => b,
                Out::Ping(n) => wire::encode_ping(n),
            };
            self.out.send(bytes).await.map_err(|_| Gone)
        }

        async fn recv(&mut self) -> Inbound {
            self.inbound.recv().await.unwrap_or(Inbound::Gone)
        }
    }

    /// A link whose client reads at most `room` frames ahead, and the
    /// client's ends.
    fn mock(
        room: usize,
    ) -> (
        Mock,
        mpsc::Receiver<Vec<u8>>,
        mpsc::UnboundedSender<Inbound>,
    ) {
        let (out, frames) = mpsc::channel(room);
        let (say, inbound) = mpsc::unbounded_channel();
        let delay = Duration::ZERO;
        (
            Mock {
                out,
                inbound,
                delay,
            },
            frames,
            say,
        )
    }

    fn feed(reg: &Reg) -> Feed<'_, MemoryStore, MemoryCharts, ManualClock> {
        Feed {
            registry: reg,
            event: EV,
            token: None,
        }
    }

    fn timing(stall: Duration) -> Timing {
        Timing {
            stall,
            ..Timing::default()
        }
    }

    fn drain(frames: &mut mpsc::Receiver<Vec<u8>>, client: &mut Client) {
        while let Ok(frame) = frames.try_recv() {
            client.apply(&frame);
        }
    }

    fn public(store: &MemoryStore) -> Vec<u8> {
        let mut codes = vec![0; N as usize];
        // The chart's oids are 100 + idx.
        for batch in store.committed() {
            for row in &batch.objects_set {
                codes[row.oid as usize - 100] = u8::from(row.status != Status::FREE);
            }
            for &oid in &batch.objects_freed {
                codes[oid as usize - 100] = 0;
            }
        }
        codes
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_client_is_pinged_every_25_s_and_closed_at_60_s() {
        let _serial = crate::http::tests::serial();
        let (_store, reg) = setup();
        let realtime = Realtime::new(10, 10, Timing::default());
        let (mut link, mut frames, _say) = mock(64);
        let sub = reg.subscribe(EV, 0, None).await.unwrap();
        let started = Instant::now();
        let ending = run(
            &realtime,
            &mut link,
            &feed(&reg),
            sub,
            &mut Stats::default(),
        )
        .await;
        assert_eq!(ending, Ending::Idle);
        assert_eq!(started.elapsed(), Duration::from_secs(60));
        let mut client = Client::default();
        drain(&mut frames, &mut client);
        use wire::types::*;
        assert_eq!(client.types, [HELLO, SNAPSHOT, PING, PING]);
        assert_eq!(client.hello.unwrap().chart_hash, [9; 16]);
        assert_eq!(reg.subscribers(EV), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_client_that_answers_pings_is_kept_until_shutdown() {
        let _serial = crate::http::tests::serial();
        let (_store, reg) = setup();
        let realtime = Realtime::new(10, 10, Timing::default());
        let (mut link, mut frames, say) = mock(64);
        let client = tokio::spawn(async move {
            let mut client = Client::default();
            while let Some(frame) = frames.recv().await {
                client.apply(&frame);
                if frame[0] == wire::types::PING {
                    // A round trip later.
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    let _ = say.send(Inbound::Alive);
                }
            }
            client
        });
        let stop = realtime.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(190)).await;
            stop.shutdown();
        });
        let sub = reg.subscribe(EV, 0, None).await.unwrap();
        let started = Instant::now();
        let ending = run(
            &realtime,
            &mut link,
            &feed(&reg),
            sub,
            &mut Stats::default(),
        )
        .await;
        assert_eq!(ending, Ending::Shutdown);
        assert_eq!(started.elapsed(), Duration::from_secs(190));
        drop(link);
        let client = client.await.unwrap();
        assert_eq!(client.count(wire::types::PING), 7);
        assert_eq!(client.closed, Some(CloseReason::SHUTTING_DOWN.0));
    }

    /// The client stops reading while 80 publications go out: the connection
    /// holds one frame, the subscription lags, and when the client reads again
    /// it gets RESYNC and the current SNAPSHOT, and ends consistent.
    #[tokio::test]
    async fn a_stalled_client_is_resynced_when_it_reads_again() {
        let _serial = crate::http::tests::serial();
        let (store, reg) = setup();
        let realtime = Realtime::new(10, 10, timing(Duration::from_secs(3600)));
        let (mut link, mut frames, _say) = mock(1);
        let sub = reg.subscribe(EV, 0, None).await.unwrap();
        let session = {
            let (realtime, reg) = (realtime.clone(), reg.clone());
            tokio::spawn(async move {
                run(
                    &realtime,
                    &mut link,
                    &feed(&reg),
                    sub,
                    &mut Stats::default(),
                )
                .await
            })
        };
        for i in 0..80 {
            let status = if i % 2 == 0 {
                Status::BLOCKED
            } else {
                Status::FREE
            };
            commit(&store, &reg, i / 2 % N, status).await;
        }
        let mut client = Client::default();
        while client.seq < 80 {
            client.apply(&frames.recv().await.unwrap());
        }
        use wire::types::*;
        assert_eq!(client.types, [HELLO, SNAPSHOT, RESYNC, SNAPSHOT]);
        assert_eq!(realtime.resyncs(), 1);
        commit(&store, &reg, 3, Status::BLOCKED).await;
        client.apply(&frames.recv().await.unwrap());
        assert_eq!((client.seq, client.codes.clone()), (81, public(&store)));
        realtime.shutdown();
        assert_eq!(session.await.unwrap(), Ending::Shutdown);
    }

    #[tokio::test(start_paused = true)]
    async fn a_client_that_never_reads_is_dropped_after_the_stall_limit() {
        let _serial = crate::http::tests::serial();
        let (_store, reg) = setup();
        let realtime = Realtime::new(10, 10, Timing::default());
        let (mut link, _frames, _say) = mock(1);
        let sub = reg.subscribe(EV, 0, None).await.unwrap();
        let started = Instant::now();
        let ending = run(
            &realtime,
            &mut link,
            &feed(&reg),
            sub,
            &mut Stats::default(),
        )
        .await;
        assert_eq!(ending, Ending::Stalled);
        assert_eq!(started.elapsed(), Duration::from_secs(30));
        assert_eq!(reg.subscribers(EV), 0);
    }

    /// After a failed commit the stream continues from the client's `seq`;
    /// after a commit whose reply was lost, the reloaded actor is ahead and
    /// the client gets a SNAPSHOT. No gap, no second HELLO.
    #[tokio::test]
    async fn the_stream_survives_actor_failures() {
        let _serial = crate::http::tests::serial();
        let (store, reg) = setup();
        let realtime = Realtime::new(10, 10, timing(Duration::from_secs(3600)));
        let (mut link, mut frames, _say) = mock(64);
        let sub = reg.subscribe(EV, 0, None).await.unwrap();
        let session = {
            let (realtime, reg) = (realtime.clone(), reg.clone());
            tokio::spawn(async move {
                run(
                    &realtime,
                    &mut link,
                    &feed(&reg),
                    sub,
                    &mut Stats::default(),
                )
                .await
            })
        };
        let mut client = Client::default();
        commit(&store, &reg, 1, Status::BLOCKED).await;
        store.push_fault(Fault::Fail);
        commit(&store, &reg, 2, Status::BLOCKED).await;
        commit(&store, &reg, 3, Status::BLOCKED).await;
        store.push_fault(Fault::LoseReply);
        commit(&store, &reg, 4, Status::BLOCKED).await;
        commit(&store, &reg, 5, Status::BLOCKED).await;
        while client.seq < 4 {
            client.apply(&frames.recv().await.unwrap());
        }
        use wire::types::*;
        assert_eq!(
            client.types,
            [HELLO, SNAPSHOT, DELTA, DELTA, SNAPSHOT, DELTA]
        );
        assert_eq!(client.codes, public(&store));
        realtime.shutdown();
        assert_eq!(session.await.unwrap(), Ending::Shutdown);
    }

    #[tokio::test(start_paused = true)]
    async fn a_blocked_write_ends_at_shutdown() {
        let _serial = crate::http::tests::serial();
        let (_store, reg) = setup();
        let realtime = Realtime::new(10, 10, Timing::default());
        let (mut link, _frames, _say) = mock(1);
        let stop = realtime.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            stop.shutdown();
        });
        let sub = reg.subscribe(EV, 0, None).await.unwrap();
        let started = Instant::now();
        let ending = run(
            &realtime,
            &mut link,
            &feed(&reg),
            sub,
            &mut Stats::default(),
        )
        .await;
        assert_eq!(ending, Ending::Shutdown);
        assert_eq!(started.elapsed(), Duration::from_secs(5));
    }

    /// Publications every 100 ms to a client whose writes take 150 ms: the
    /// feed is always ready, and still the client is pinged and, silent, is
    /// closed at 60 s, give or take the writes in flight (RESYNC and SNAPSHOT
    /// at most).
    #[tokio::test(start_paused = true)]
    async fn a_busy_feed_does_not_hold_back_ping_or_silence() {
        let _serial = crate::http::tests::serial();
        let (store, reg) = setup();
        let realtime = Realtime::new(10, 10, timing(Duration::from_secs(3600)));
        let (mut link, mut frames, _say) = mock(64);
        link.delay = Duration::from_millis(150);
        let pings = tokio::spawn(async move {
            let mut pings = 0;
            while let Some(frame) = frames.recv().await {
                pings += usize::from(frame[0] == wire::types::PING);
            }
            pings
        });
        let busy = {
            let reg = reg.clone();
            tokio::spawn(async move {
                for i in 0..2_000 {
                    let status = if i % 2 == 0 {
                        Status::BLOCKED
                    } else {
                        Status::FREE
                    };
                    commit(&store, &reg, 0, status).await;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
        };
        let sub = reg.subscribe(EV, 0, None).await.unwrap();
        let started = Instant::now();
        let ending = run(
            &realtime,
            &mut link,
            &feed(&reg),
            sub,
            &mut Stats::default(),
        )
        .await;
        assert_eq!(ending, Ending::Idle);
        assert!(
            started.elapsed() <= Duration::from_millis(60_500),
            "{:?}",
            started.elapsed()
        );
        busy.abort();
        drop(link);
        assert_eq!(pings.await.unwrap(), 2);
    }

    /// A link whose every write takes `cost` of CPU, as a write to a socket
    /// on loopback costs the kernel's work at both ends; its client says
    /// nothing.
    struct Busy {
        cost: Duration,
    }

    impl Link for Busy {
        const HEARS_CLIENT: bool = false;

        async fn send(&mut self, _: Out) -> Result<(), Gone> {
            let started = std::time::Instant::now();
            while started.elapsed() < self.cost {
                std::hint::spin_loop();
            }
            Ok(())
        }

        async fn recv(&mut self) -> Inbound {
            std::future::pending().await
        }
    }

    /// 2,000 subscribers whose every frame takes 200 µs of CPU to write keep
    /// their runtime busy for 400 ms per publication. Commands go to the
    /// actor on the request runtime, a single thread here, and each is
    /// answered at once all the same: no reply waits behind fanning out
    /// (08 §8.3, D-41). With the streams on the request runtime, each would
    /// wait for a whole pass.
    #[test]
    fn fanning_out_never_delays_a_command() {
        let _serial = crate::http::tests::serial();
        let requests = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let streams = runtime(1).unwrap();
        let worst = requests.block_on(async {
            let (store, reg) = setup_with(cavea_engine::RegistryConfig {
                actor: cavea_engine::ActorConfig {
                    broadcast_on: Some(streams.handle().clone()),
                    ..cavea_engine::ActorConfig::default()
                },
                ..cavea_engine::RegistryConfig::default()
            });
            let realtime =
                Realtime::on_runtime(10_000, 10_000, Timing::default(), streams.handle().clone());
            for _ in 0..2_000 {
                let sub = reg.subscribe(EV, 0, None).await.unwrap();
                let (realtime, reg) = (realtime.clone(), reg.clone());
                realtime.clone().spawn(async move {
                    let mut link = Busy {
                        cost: Duration::from_micros(200),
                    };
                    run(
                        &realtime,
                        &mut link,
                        &feed(&reg),
                        sub,
                        &mut Stats::default(),
                    )
                    .await;
                });
            }
            let mut worst = Duration::ZERO;
            for i in 0..20 {
                let status = if i % 2 == 0 {
                    Status::BLOCKED
                } else {
                    Status::FREE
                };
                let started = std::time::Instant::now();
                reg.send(EV, block(i / 2 % N, status)).await.unwrap();
                worst = worst.max(started.elapsed());
                // The next publication is due; let the pass get going.
                store.clock().advance_ms(100);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            realtime.shutdown();
            worst
        });
        streams.shutdown_background();
        assert!(worst < Duration::from_millis(50), "a reply took {worst:?}");
    }
}
