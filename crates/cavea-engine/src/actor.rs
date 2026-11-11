//! The event actor: a bounded mailbox in front of [`EventState`], with natural
//! batching, acknowledge-after-commit and hold expiry on the injected clock
//! (08 §4.3, §4.6, §7.3), lease renewal when idle (§4.8) and idle eviction
//! (§4.7).

use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::task::{Context, Poll};

use tokio::sync::{mpsc, oneshot, watch};
use tracing::{Instrument, Span, debug, error, info, info_span};

use crate::feed::{End, Feed, Publisher, Sealed};
use crate::state::{EventState, InvariantViolation};
use crate::store::{SeatStore, StoreError};
use crate::types::{Clock, Failure, Op, Reply, Request, Timestamp};

#[derive(Debug, Clone, Copy)]
pub struct ActorConfig {
    /// Commands that may wait in the mailbox; beyond this callers get
    /// `overloaded`.
    pub mailbox: usize,
    /// Most commands in one batch.
    pub max_batch: usize,
    /// How long after its last commit or renewal an idle actor renews its
    /// lease. Well under the store's lease (15 s).
    pub heartbeat_ms: u64,
    /// An actor with no command, no hold and no [`KeepAlive`] for this long
    /// stops. It is checked once per period, so after the last command, hold
    /// expiry or `KeepAlive` it can take up to twice this.
    pub idle_ms: u64,
}

impl Default for ActorConfig {
    fn default() -> Self {
        Self {
            mailbox: 10_000,
            max_batch: 2_000,
            heartbeat_ms: 5_000,
            idle_ms: 600_000,
        }
    }
}

/// Why an actor stopped. The event must be reloaded from the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActorError {
    Store(StoreError),
    Invariant(InvariantViolation),
}

impl std::fmt::Display for ActorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(e) => e.fmt(f),
            Self::Invariant(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ActorError {}

struct Envelope {
    request: Request,
    reply: oneshot::Sender<Reply>,
    /// The caller's span; apply runs inside it.
    parent: Span,
    /// Open while the command waits in the mailbox.
    wait: Span,
}

/// A reply waiting to be released, with its `apply` span and the caller's span.
type Queued = (oneshot::Sender<Reply>, Reply, Span, Span);

/// Sends commands to one actor. Cheap to clone.
#[derive(Debug, Clone)]
pub struct ActorHandle {
    tx: mpsc::Sender<Envelope>,
    control: Arc<Control>,
    feed: Arc<Feed>,
}

#[derive(Debug)]
struct Control {
    /// Set to ask the actor to finish its work and stop.
    stop: watch::Sender<bool>,
    /// Live [`KeepAlive`] guards.
    keep: AtomicUsize,
    /// How many [`KeepAlive`] guards have dropped: one dropping is activity.
    released: AtomicU64,
}

/// Keeps an actor from being evicted while it exists: what a realtime
/// subscriber holds. It does not keep a stopped actor alive.
#[derive(Debug)]
pub struct KeepAlive(Arc<Control>);

impl Clone for KeepAlive {
    fn clone(&self) -> Self {
        self.0.keep.fetch_add(1, Ordering::SeqCst);
        Self(self.0.clone())
    }
}

impl Drop for KeepAlive {
    fn drop(&mut self) {
        // Counted as released before it stops counting as held: an idle
        // check that sees the guard gone also sees it released (below).
        self.0.released.fetch_add(1, Ordering::SeqCst);
        self.0.keep.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Why a command could not be enqueued.
#[derive(Debug)]
pub(crate) enum Rejected {
    Overloaded,
    /// The actor has stopped or is stopping; the request was not applied.
    Closed(Box<Request>),
}

impl std::fmt::Debug for Envelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Envelope")
    }
}

/// The reply to a submitted command. Resolves once the command's batch has
/// committed; resolves to `unavailable` if the actor stops first.
#[derive(Debug)]
pub struct PendingReply(oneshot::Receiver<Reply>);

impl Future for PendingReply {
    type Output = Reply;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Reply> {
        Pin::new(&mut self.0)
            .poll(cx)
            .map(|r| r.unwrap_or(Err(Failure::Unavailable)))
    }
}

impl ActorHandle {
    /// Enqueue without waiting. Commands are applied in submission order.
    pub fn submit(&self, request: Request) -> Result<PendingReply, Failure> {
        self.try_submit(request).map_err(|r| match r {
            Rejected::Overloaded => Failure::Overloaded,
            Rejected::Closed(_) => Failure::Unavailable,
        })
    }

    pub(crate) fn try_submit(&self, request: Request) -> Result<PendingReply, Rejected> {
        let (reply, rx) = oneshot::channel();
        let parent = Span::current();
        let wait = info_span!("mailbox_wait", op = request.command.op().name());
        let envelope = Envelope {
            request,
            reply,
            parent,
            wait,
        };
        match self.tx.try_send(envelope) {
            Ok(()) => Ok(PendingReply(rx)),
            Err(mpsc::error::TrySendError::Full(_)) => Err(Rejected::Overloaded),
            Err(mpsc::error::TrySendError::Closed(e)) => Err(Rejected::Closed(Box::new(e.request))),
        }
    }

    pub async fn send(&self, request: Request) -> Reply {
        self.submit(request)?.await
    }

    /// Ask the actor to stop taking commands, apply and commit what it has
    /// accepted, and end with `Ok`.
    pub fn stop(&self) {
        self.control.stop.send_replace(true);
    }

    /// Keep the actor from being evicted while the guard lives.
    #[must_use]
    pub fn keep_alive(&self) -> KeepAlive {
        self.control.keep.fetch_add(1, Ordering::SeqCst);
        KeepAlive(self.control.clone())
    }

    /// What the actor publishes for realtime readers.
    pub(crate) fn feed(&self) -> &Arc<Feed> {
        &self.feed
    }

    /// Whether both handles lead to the same actor.
    pub(crate) fn same_actor(&self, other: &Self) -> bool {
        self.tx.same_channel(&other.tx)
    }
}

/// Create an actor for `state`. The caller drives the returned future on its
/// runtime; it ends with `Ok` once every handle is dropped, it is stopped, or
/// it has been idle for [`ActorConfig::idle_ms`], and all its work is
/// committed and published; or with the error that stopped it. Its first
/// public snapshot is published before this returns. Its lease is renewed by every
/// commit and, when no commit has done it for [`ActorConfig::heartbeat_ms`],
/// by a heartbeat; a failed heartbeat stops it like a failed commit.
pub fn actor<S: SeatStore, C: Clock>(
    state: EventState,
    store: Arc<S>,
    clock: C,
    config: ActorConfig,
) -> (
    ActorHandle,
    impl Future<Output = Result<(), ActorError>> + Send,
) {
    let (tx, rx) = mpsc::channel(config.mailbox.max(1));
    let control = Arc::new(Control {
        stop: watch::Sender::new(false),
        keep: AtomicUsize::new(0),
        released: AtomicU64::new(0),
    });
    let (feed, publisher) = Publisher::new(&state);
    let run = run(state, publisher, store, clock, rx, control.clone(), config);
    (ActorHandle { tx, control, feed }, run)
}

#[allow(
    clippy::large_enum_variant,
    reason = "one short-lived value per loop turn; boxing would allocate per command"
)]
enum Next {
    Received(Envelope),
    Closed,
    Committed(Result<(), StoreError>),
    /// The clock reached the earliest hold expiry.
    Expiry,
    /// The clock reached the next heartbeat or idle check.
    Chore,
    /// [`ActorHandle::stop`] was called.
    Stop,
    /// The clock reached the next publication.
    Publish,
}

/// A commit or a heartbeat.
type Write<'a> = Pin<Box<dyn Future<Output = Result<(), StoreError>> + Send + 'a>>;

async fn run<S: SeatStore, C: Clock>(
    mut state: EventState,
    publisher: Result<Publisher, InvariantViolation>,
    store: Arc<S>,
    clock: C,
    mut rx: mpsc::Receiver<Envelope>,
    control: Arc<Control>,
    config: ActorConfig,
) -> Result<(), ActorError> {
    let mut publisher = match publisher {
        Ok(p) => p,
        Err(v) => return Err(stop(ActorError::Invariant(v), Vec::new(), Vec::new(), rx)),
    };
    // What the batch in flight changes, published only once it commits.
    let mut sealed = Sealed::default();
    // The wait for the next publication.
    let mut publish_timer = None;
    let max_batch = config.max_batch.max(1);
    let (event, epoch) = (state.event(), state.epoch());
    // Replies applied to memory, waiting for the next commit to start.
    let mut pending: Vec<Queued> = Vec::new();
    // Replies of the batch being committed.
    let mut committing: Vec<Queued> = Vec::new();
    let mut in_flight: Option<Write<'_>> = None;
    // The write in flight is a heartbeat.
    let mut renewing = false;
    let mut open = true;
    // The wait for the earliest expiry, rebuilt when that expiry changes.
    let mut timer = None;
    // When the lease is next renewed if no commit does it, and when the actor
    // is next checked for idleness; one timer waits for the earlier.
    let mut renew_at = after(clock.now(), config.heartbeat_ms);
    let mut idle_at = after(clock.now(), config.idle_ms);
    // `control.released` at the last idle check.
    let mut released = 0;
    let mut chore = None;
    let mut stop_signal = Some(Box::pin({
        let mut stop = control.stop.subscribe();
        async move {
            // The sender lives in `control`, as long as this future.
            let _ = stop.wait_for(|&stop| stop).await;
        }
    }));

    loop {
        // Expiries change state without a caller, so a batch may have no
        // replies.
        if in_flight.is_none() {
            match state.take_batch_into(&mut sealed) {
                Err(v) => return Err(stop(ActorError::Invariant(v), pending, committing, rx)),
                // Only refusals and no-ops: nothing to write, and everything
                // they observed is already committed.
                Ok(None) => release(&mut pending),
                Ok(Some(batch)) => {
                    // `seq` is per event, so the event id names the commit too.
                    let event_id = format!("{:032x}", batch.event.0);
                    let span = info_span!(
                        "commit",
                        event_id,
                        seq = batch.seq,
                        commands = pending.len()
                    );
                    for (_, _, apply, caller) in &pending {
                        span.follows_from(apply);
                        // Lets the caller's own log records name this commit.
                        caller.record("event_id", &event_id);
                        caller.record("commit_seq", batch.seq);
                    }
                    in_flight = Some(Box::pin(store.commit_batch(batch).instrument(span)));
                    committing = std::mem::take(&mut pending);
                }
            }
        }
        let now = clock.now();
        if publisher.due().is_some_and(|at| now >= at)
            && let Err(v) = publisher.publish(now)
        {
            return Err(stop(ActorError::Invariant(v), pending, committing, rx));
        }
        if !open && in_flight.is_none() && pending.is_empty() {
            // Everything is committed: publish it before ending.
            if let Err(v) = publisher.publish(now) {
                return Err(stop(ActorError::Invariant(v), pending, committing, rx));
            }
            publisher.end(End::Stopped);
            return Ok(());
        }
        let next_publish = publisher.due();
        if publish_timer.as_ref().map(|(at, _)| *at) != next_publish {
            publish_timer = next_publish.map(|at| (at, Box::pin(clock.sleep_until(at))));
        }
        let next_expiry = state.next_expiry();
        if timer.as_ref().map(|(at, _)| *at) != next_expiry {
            timer = next_expiry.map(|at: Timestamp| (at, Box::pin(clock.sleep_until(at))));
        }
        // A write in flight renews the lease itself; a stopping actor needs
        // no renewal.
        let may_renew = in_flight.is_none() && open && stop_signal.is_some();
        let next_chore = if may_renew {
            renew_at.min(idle_at)
        } else {
            idle_at
        };
        if chore.as_ref().map(|(at, _)| *at) != Some(next_chore) {
            chore = Some((next_chore, Box::pin(clock.sleep_until(next_chore))));
        }

        let next = poll_fn(|cx| {
            if let Some(commit) = in_flight.as_mut()
                && let Poll::Ready(result) = commit.as_mut().poll(cx)
            {
                return Poll::Ready(Next::Committed(result));
            }
            if let Some((_, sleep)) = timer.as_mut()
                && sleep.as_mut().poll(cx).is_ready()
            {
                return Poll::Ready(Next::Expiry);
            }
            if let Some((_, sleep)) = chore.as_mut()
                && sleep.as_mut().poll(cx).is_ready()
            {
                return Poll::Ready(Next::Chore);
            }
            if let Some((_, sleep)) = publish_timer.as_mut()
                && sleep.as_mut().poll(cx).is_ready()
            {
                return Poll::Ready(Next::Publish);
            }
            if let Some(stop) = stop_signal.as_mut()
                && stop.as_mut().poll(cx).is_ready()
            {
                return Poll::Ready(Next::Stop);
            }
            // A full batch stops intake; the mailbox absorbs the rest.
            if open && pending.len() < max_batch {
                return rx
                    .poll_recv(cx)
                    .map(|m| m.map_or(Next::Closed, Next::Received));
            }
            Poll::Pending
        })
        .await;

        match next {
            Next::Committed(Ok(())) => {
                in_flight = None;
                renew_at = after(clock.now(), config.heartbeat_ms);
                if std::mem::take(&mut renewing) {
                    debug!("lease renewed");
                } else {
                    debug!(commands = committing.len(), "batch committed");
                    publisher.commit(&sealed);
                }
                release(&mut committing);
            }
            Next::Committed(Err(e)) => {
                return Err(stop(ActorError::Store(e), pending, committing, rx));
            }
            Next::Closed => open = false,
            Next::Stop => {
                stop_signal = None;
                debug!("stopping");
                // Commands already accepted are still received, then `None`.
                rx.close();
            }
            // Published at the top of the loop.
            Next::Publish => publish_timer = None,
            Next::Chore => {
                chore = None;
                let now = clock.now();
                publisher.prune(now);
                if now >= idle_at {
                    // A guard dropped since the last check started a new
                    // idle period. `keep` is read before `released`, the
                    // reverse of `KeepAlive::drop`: a drop that has made
                    // `keep` zero has already counted itself in `released`.
                    // A heartbeat is not activity.
                    let held = control.keep.load(Ordering::SeqCst) > 0;
                    let last =
                        std::mem::replace(&mut released, control.released.load(Ordering::SeqCst));
                    let idle = (in_flight.is_none() || renewing)
                        && pending.is_empty()
                        && state.next_expiry().is_none()
                        && !held
                        && released == last;
                    if idle && stop_signal.take().is_some() {
                        info!("actor idle; evicting");
                        rx.close();
                    }
                    idle_at = after(now, config.idle_ms);
                }
                if may_renew && stop_signal.is_some() && now >= renew_at {
                    renew_at = Timestamp(u64::MAX);
                    renewing = true;
                    let span = info_span!("heartbeat");
                    in_flight = Some(Box::pin(store.heartbeat(event, epoch).instrument(span)));
                }
            }
            Next::Expiry => {
                timer = None;
                idle_at = after(clock.now(), config.idle_ms);
                let span = info_span!("apply", op = Op::Expire.name());
                match span.in_scope(|| state.expire_due(clock.now())) {
                    Ok(expired) => span.in_scope(|| debug!(expired, "holds expired")),
                    Err(v) => return Err(stop(ActorError::Invariant(v), pending, committing, rx)),
                }
            }
            Next::Received(envelope) => {
                let Envelope {
                    request,
                    reply,
                    parent,
                    wait,
                } = envelope;
                drop(wait);
                idle_at = after(clock.now(), config.idle_ms);
                let span = info_span!(parent: &parent, "apply", op = request.command.op().name());
                let applied = span.in_scope(|| state.apply(&request, clock.now()));
                match applied {
                    Ok(result) => pending.push((reply, result, span, parent)),
                    Err(v) => {
                        let _ = reply.send(Err(Failure::Unavailable));
                        return Err(stop(ActorError::Invariant(v), pending, committing, rx));
                    }
                }
            }
        }
    }
}

fn after(t: Timestamp, ms: u64) -> Timestamp {
    Timestamp(t.0.saturating_add(ms))
}

fn release(queued: &mut Vec<Queued>) {
    for (reply, result, span, _) in queued.drain(..) {
        if let Err(failure) = &result {
            span.in_scope(|| debug!(code = failure.code(), "command refused"));
        }
        // The caller may have gone away; nothing to do then.
        let _ = reply.send(result);
    }
}

/// Fail every caller whose command is not durable, close the mailbox and drain
/// it. Memory is discarded with the actor; reloading is the only recovery.
fn stop(
    err: ActorError,
    pending: Vec<Queued>,
    committing: Vec<Queued>,
    mut rx: mpsc::Receiver<Envelope>,
) -> ActorError {
    match &err {
        ActorError::Store(StoreError::Fenced) => {
            error!(error = %err, "fencing check failed: another node owns this event; actor stopped");
        }
        _ => error!(error = %err, "actor stopped; event must be reloaded"),
    }
    for (reply, _, _, _) in committing.into_iter().chain(pending) {
        let _ = reply.send(Err(Failure::Unavailable));
    }
    rx.close();
    while let Ok(envelope) = rx.try_recv() {
        let _ = envelope.reply.send(Err(Failure::Unavailable));
    }
    err
}
