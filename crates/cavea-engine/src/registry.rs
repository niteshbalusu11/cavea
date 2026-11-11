//! The actor registry (08 §3, §4.6, §4.7): finds the live actor for an event or
//! loads it, once, however many requests ask at the same time; drops an actor
//! that stops, so that the next request loads the event afresh; and stops
//! every actor in order at shutdown.
//!
//! Each event has one task that loads it, runs its actor and cleans up after
//! it. The map's lock is held only to look up or change an entry, never across
//! an await, so one event's load never delays another event's commands.

use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};

use tokio::sync::watch;
use tracing::{Instrument as _, debug, error, info, info_span, warn};

use crate::actor::{ActorConfig, ActorHandle, KeepAlive, PendingReply, Rejected, actor};
use crate::state::{EventState, LoadError};
use crate::store::{Acquired, ChartProvider, SeatStore, StoreError};
use crate::types::{Clock, EventId, Failure, Reply, Request, Timestamp};

#[derive(Debug, Clone, Copy)]
pub struct RegistryConfig {
    pub actor: ActorConfig,
    /// After a load fails, requests for that event fail at once for this long
    /// instead of trying again, so an outage costs the store at most one load
    /// attempt per event per period.
    pub retry_ms: u64,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            actor: ActorConfig::default(),
            retry_ms: 1_000,
        }
    }
}

/// Why an event's actor could not be had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// Acquiring, reading the chart or reading state failed. `Leased` means
    /// another node owns the event.
    Store(StoreError),
    /// The stored state does not fit the chart.
    Restore(LoadError),
    /// The load task panicked.
    Panicked,
    /// The registry is shutting down.
    ShuttingDown,
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(e) => e.fmt(f),
            Self::Restore(e) => e.fmt(f),
            Self::Panicked => f.write_str("event load panicked"),
            Self::ShuttingDown => f.write_str("shutting down"),
        }
    }
}

impl std::error::Error for OpenError {}

type LoadResult = Option<Result<Live, OpenError>>;

#[derive(Debug, Clone)]
struct Live {
    handle: ActorHandle,
    /// Becomes `true` once the actor has ended, its lease is released and its
    /// entry is gone.
    done: watch::Receiver<bool>,
}

#[derive(Debug)]
enum Slot {
    Loading(watch::Receiver<LoadResult>),
    Live(Live),
    /// A load failed; requests fail at once until `retry_at`.
    Failed {
        error: OpenError,
        retry_at: Timestamp,
    },
}

#[derive(Debug, Default)]
struct Slots {
    map: HashMap<EventId, Slot>,
    shutting_down: bool,
}

struct Inner<S, P, C> {
    store: Arc<S>,
    charts: Arc<P>,
    clock: C,
    config: RegistryConfig,
    slots: Mutex<Slots>,
}

impl<S, P, C> Inner<S, P, C> {
    fn slots(&self) -> MutexGuard<'_, Slots> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Every live event actor of this node. Cheap to clone. Must be used inside a
/// Tokio runtime: loads and actors run as tasks of their own.
pub struct Registry<S, P, C> {
    inner: Arc<Inner<S, P, C>>,
}

impl<S, P, C> Clone for Registry<S, P, C> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// What a lookup found.
enum Found {
    Live(ActorHandle),
    Wait(watch::Receiver<LoadResult>),
}

/// A load or an actor that ended can be retried a bounded number of times:
/// each try either finds a live actor or starts a load.
const TRIES: usize = 3;

impl<S: SeatStore, P: ChartProvider, C: Clock + Clone> Registry<S, P, C> {
    pub fn new(store: Arc<S>, charts: Arc<P>, clock: C, config: RegistryConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                charts,
                clock,
                config,
                slots: Mutex::default(),
            }),
        }
    }

    /// Send a command to the event's actor and wait for its reply, loading
    /// the event first if needed. Every load failure is `unavailable`.
    pub async fn send(&self, event: EventId, request: Request) -> Reply {
        self.submit(event, request).await?.await
    }

    /// Enqueue a command for the event's actor, loading the event first if
    /// needed. A command that reaches an actor as it stops is not applied; it
    /// goes to the reloaded actor instead.
    pub async fn submit(&self, event: EventId, request: Request) -> Result<PendingReply, Failure> {
        let mut request = request;
        for _ in 0..TRIES {
            // Every load failure is `503 unavailable` to the caller.
            let handle = self.handle(event).await.map_err(|_| Failure::Unavailable)?;
            match handle.try_submit(request) {
                Ok(reply) => return Ok(reply),
                Err(Rejected::Overloaded) => return Err(Failure::Overloaded),
                Err(Rejected::Closed(back)) => {
                    request = *back;
                    self.wait_until_gone(event, &handle).await;
                }
            }
        }
        Err(Failure::Unavailable)
    }

    /// Keep the event's actor from being evicted while the guard lives,
    /// loading it first if needed. If the actor stops for another reason, the
    /// guard no longer applies: the holder must ask again.
    pub async fn keep_alive(&self, event: EventId) -> Result<KeepAlive, OpenError> {
        Ok(self.handle(event).await?.keep_alive())
    }

    /// Load each event, one at a time, and return those that failed. For the
    /// server's startup (08 §4.7).
    pub async fn preload(
        &self,
        events: impl IntoIterator<Item = EventId>,
    ) -> Vec<(EventId, OpenError)> {
        let mut failed = Vec::new();
        for event in events {
            if let Err(e) = self.handle(event).await {
                failed.push((event, e));
            }
        }
        failed
    }

    /// How many actors are live, stopping included.
    #[must_use]
    pub fn loaded(&self) -> usize {
        let slots = self.inner.slots();
        slots
            .map
            .values()
            .filter(|s| matches!(s, Slot::Live(_)))
            .count()
    }

    /// Stop taking commands, let every actor commit what it accepted, and
    /// release every lease (08 §14). Returns once every actor has ended.
    /// Commits time out in the store, so this ends; the caller bounds it with
    /// its own deadline.
    pub async fn shutdown(&self) {
        loop {
            let waits: Vec<_> = {
                let mut slots = self.inner.slots();
                slots.shutting_down = true;
                slots.map.retain(|_, s| !matches!(s, Slot::Failed { .. }));
                slots
                    .map
                    .values()
                    .map(|slot| match slot {
                        Slot::Live(live) => {
                            live.handle.stop();
                            Shutdown::Live(live.done.clone())
                        }
                        Slot::Loading(rx) => Shutdown::Loading(rx.clone()),
                        Slot::Failed { .. } => Shutdown::Nothing,
                    })
                    .collect()
            };
            if waits.is_empty() {
                info!("every actor stopped");
                return;
            }
            for wait in waits {
                match wait {
                    Shutdown::Live(mut done) => {
                        let _ = done.wait_for(|&d| d).await;
                    }
                    Shutdown::Loading(mut rx) => {
                        let _ = rx.wait_for(Option::is_some).await;
                    }
                    Shutdown::Nothing => {}
                }
            }
        }
    }

    /// The event's live actor, loading it if needed.
    async fn handle(&self, event: EventId) -> Result<ActorHandle, OpenError> {
        for _ in 0..TRIES {
            let mut rx = match self.find(event)? {
                Found::Live(handle) => return Ok(handle),
                Found::Wait(rx) => rx,
            };
            let loaded = match rx.wait_for(Option::is_some).await {
                Ok(result) => result.clone(),
                // The load task ended without a result: it cannot, but if it
                // did, its entry is gone and the next try loads again.
                Err(_) => None,
            };
            match loaded {
                Some(Ok(live)) => return Ok(live.handle),
                Some(Err(e)) => return Err(e),
                None => {}
            }
        }
        Err(OpenError::Store(StoreError::Unavailable(
            "event load did not settle".into(),
        )))
    }

    /// Look the event up; start its load if it has no entry.
    fn find(&self, event: EventId) -> Result<Found, OpenError> {
        let mut slots = self.inner.slots();
        // Even a live actor takes nothing more once shutdown has begun.
        if slots.shutting_down {
            return Err(OpenError::ShuttingDown);
        }
        match slots.map.get(&event) {
            Some(Slot::Live(live)) => return Ok(Found::Live(live.handle.clone())),
            Some(Slot::Loading(rx)) => return Ok(Found::Wait(rx.clone())),
            Some(Slot::Failed { error, retry_at }) if self.inner.clock.now() < *retry_at => {
                return Err(error.clone());
            }
            Some(Slot::Failed { .. }) | None => {}
        }
        let (tx, rx) = watch::channel(None);
        slots.map.insert(event, Slot::Loading(rx.clone()));
        drop(slots);
        let inner = self.inner.clone();
        // Not the caller's span: the actor outlives the request that loaded it.
        let span = info_span!(parent: None, "actor", event_id = %hex(event));
        tokio::spawn(run_event(inner, event, tx).instrument(span));
        Ok(Found::Wait(rx))
    }

    /// Wait until the actor behind `handle` has ended and left the map.
    async fn wait_until_gone(&self, event: EventId, handle: &ActorHandle) {
        let done = {
            let slots = self.inner.slots();
            match slots.map.get(&event) {
                Some(Slot::Live(live)) if live.handle.same_actor(handle) => live.done.clone(),
                _ => return,
            }
        };
        let mut done = done;
        let _ = done.wait_for(|&d| d).await;
    }
}

enum Shutdown {
    Live(watch::Receiver<bool>),
    Loading(watch::Receiver<LoadResult>),
    Nothing,
}

fn hex(event: EventId) -> String {
    format!("{:032x}", event.0)
}

/// Load the event, run its actor until it ends, then clean up.
async fn run_event<S: SeatStore, P: ChartProvider, C: Clock + Clone>(
    inner: Arc<Inner<S, P, C>>,
    event: EventId,
    loaded: watch::Sender<LoadResult>,
) {
    let opened = CatchUnwind(Box::pin(open(&inner, event)))
        .await
        .unwrap_or(Err(OpenError::Panicked));
    let state = match opened {
        Ok(state) => state,
        Err(error) => {
            warn!(%error, "event load failed");
            let retry_at = Timestamp(inner.clock.now().0.saturating_add(inner.config.retry_ms));
            inner.slots().map.insert(
                event,
                Slot::Failed {
                    error: error.clone(),
                    retry_at,
                },
            );
            loaded.send_replace(Some(Err(error)));
            drop(loaded);
            // Forget the failure once it no longer holds requests back, so
            // that failed events do not accumulate.
            inner.clock.sleep_until(retry_at).await;
            let mut slots = inner.slots();
            if matches!(slots.map.get(&event), Some(Slot::Failed { retry_at: r, .. }) if *r == retry_at)
            {
                slots.map.remove(&event);
            }
            return;
        }
    };
    let epoch = state.epoch();
    let (handle, run) = actor(
        state,
        inner.store.clone(),
        inner.clock.clone(),
        inner.config.actor,
    );
    let (done_tx, done) = watch::channel(false);
    let live = Live { handle, done };
    {
        let mut slots = inner.slots();
        if slots.shutting_down {
            live.handle.stop();
        }
        slots.map.insert(event, Slot::Live(live.clone()));
    }
    loaded.send_replace(Some(Ok(live.clone())));
    drop(loaded);

    let ended = CatchUnwind(Box::pin(run)).await;
    match ended {
        Ok(Ok(())) => match inner.store.release(event, epoch).await {
            Ok(()) => info!(epoch, "actor stopped; lease released"),
            Err(error) => warn!(epoch, %error, "actor stopped; lease not released"),
        },
        // The actor has logged why; its callers were told `unavailable`.
        Ok(Err(_)) => info!(epoch, "actor dropped; the next command reloads the event"),
        Err(()) => error!(epoch, "actor panicked; the next command reloads the event"),
    }
    {
        let mut slots = inner.slots();
        if matches!(slots.map.get(&event), Some(Slot::Live(l)) if l.handle.same_actor(&live.handle))
        {
            slots.map.remove(&event);
        }
    }
    done_tx.send_replace(true);
}

/// 08 §4.7: acquire ownership, get the chart for the pinned version, read the
/// state, and restore it. Holds already past their expiry are expired by the
/// actor before anything else, as the first commit it makes. If anything after
/// the acquisition fails, the lease is released: no actor holds the event.
async fn open<S: SeatStore, P: ChartProvider, C: Clock>(
    inner: &Inner<S, P, C>,
    event: EventId,
) -> Result<EventState, OpenError> {
    let acquired = inner.store.acquire(event).await.map_err(OpenError::Store)?;
    info!(epoch = acquired.epoch, "ownership acquired");
    let read = CatchUnwind(Box::pin(read(inner, event, acquired)))
        .await
        .unwrap_or(Err(OpenError::Panicked));
    if read.is_err()
        && let Err(error) = inner.store.release(event, acquired.epoch).await
    {
        debug!(%error, "lease not released after a failed load");
    }
    read
}

async fn read<S: SeatStore, P: ChartProvider, C: Clock>(
    inner: &Inner<S, P, C>,
    event: EventId,
    acquired: Acquired,
) -> Result<EventState, OpenError> {
    let chart = inner
        .charts
        .chart(acquired.chart_version)
        .await
        .map_err(OpenError::Store)?;
    let loaded = inner
        .store
        .load_event(event)
        .await
        .map_err(OpenError::Store)?;
    // Another node took the event between the two reads.
    if loaded.epoch != acquired.epoch {
        return Err(OpenError::Store(StoreError::Fenced));
    }
    debug!(seq = loaded.seq, holds = loaded.holds.len(), "event read");
    EventState::restore(event, chart, loaded).map_err(OpenError::Restore)
}

/// Resolves to `Err` if polling the future panics, which then drops it: an
/// actor's panic fails its callers (their reply channels close) and leaves the
/// process and the registry working.
struct CatchUnwind<F>(Pin<Box<F>>);

impl<F: Future> Future for CatchUnwind<F> {
    type Output = Result<F::Output, ()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let fut = self.0.as_mut();
        // The future is never polled again after a panic, so no broken state
        // it may hold is observed.
        match std::panic::catch_unwind(AssertUnwindSafe(|| fut.poll(cx))) {
            Ok(poll) => poll.map(Ok),
            Err(_) => Poll::Ready(Err(())),
        }
    }
}
