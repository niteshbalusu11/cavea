//! The event actor: a bounded mailbox in front of [`EventState`], with natural
//! batching and acknowledge-after-commit (08 §4.3, §4.6).

use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::sync::{mpsc, oneshot};
use tracing::{Instrument, Span, debug, error, info_span};

use crate::state::{EventState, InvariantViolation};
use crate::store::{SeatStore, StoreError};
use crate::types::{Clock, Failure, Reply, Request};

#[derive(Debug, Clone, Copy)]
pub struct ActorConfig {
    /// Commands that may wait in the mailbox; beyond this callers get
    /// `overloaded`.
    pub mailbox: usize,
    /// Most commands in one batch.
    pub max_batch: usize,
}

impl Default for ActorConfig {
    fn default() -> Self {
        Self {
            mailbox: 10_000,
            max_batch: 2_000,
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
            Err(mpsc::error::TrySendError::Full(_)) => Err(Failure::Overloaded),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(Failure::Unavailable),
        }
    }

    pub async fn send(&self, request: Request) -> Reply {
        self.submit(request)?.await
    }
}

/// Create an actor for `state`. The caller drives the returned future on its
/// runtime; it ends with `Ok` once every handle is dropped and all work is
/// committed, or with the error that stopped it.
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
    let run = run(state, store, clock, rx, config.max_batch.max(1));
    (ActorHandle { tx }, run)
}

enum Next {
    Received(Envelope),
    Closed,
    Committed(Result<(), StoreError>),
}

async fn run<S: SeatStore, C: Clock>(
    mut state: EventState,
    store: Arc<S>,
    clock: C,
    mut rx: mpsc::Receiver<Envelope>,
    max_batch: usize,
) -> Result<(), ActorError> {
    // Replies applied to memory, waiting for the next commit to start.
    let mut pending: Vec<Queued> = Vec::new();
    // Replies of the batch being committed.
    let mut committing: Vec<Queued> = Vec::new();
    let mut in_flight = None;
    let mut open = true;

    loop {
        if in_flight.is_none() && !pending.is_empty() {
            match state.take_batch() {
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
        if !open && in_flight.is_none() && pending.is_empty() {
            return Ok(());
        }

        let next = poll_fn(|cx| {
            if let Some(commit) = in_flight.as_mut()
                && let Poll::Ready(result) = commit.as_mut().poll(cx)
            {
                return Poll::Ready(Next::Committed(result));
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
                debug!(commands = committing.len(), "batch committed");
                release(&mut committing);
            }
            Next::Committed(Err(e)) => {
                return Err(stop(ActorError::Store(e), pending, committing, rx));
            }
            Next::Closed => open = false,
            Next::Received(envelope) => {
                let Envelope {
                    request,
                    reply,
                    parent,
                    wait,
                } = envelope;
                drop(wait);
                let span = info_span!(parent: &parent, "apply", op = request.command.op().name());
                let applied =
                    span.in_scope(|| state.apply(&request.actor, &request.command, clock.now()));
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
