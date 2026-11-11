//! Publication of committed state to realtime subscribers (08 §4.5, §8.3).
//!
//! The actor owns a [`Publisher`]. After each commit it folds the batch's
//! changes, as sealed by [`EventState::take_batch_into`], into its own copy of
//! the committed statuses; nothing applied but uncommitted ever reaches it
//! (INV-8). At most once per interval it publishes: one public DELTA from the
//! last published `seq` to the committed `seq`, the public SNAPSHOT at that
//! same `seq`, and the committed holds of every token touched since. Every
//! snapshot's `seq` is therefore a delta boundary, so a client resumes from any
//! snapshot it was given through the ring buffer.
//!
//! Readers share an [`Arc<Feed>`]: the current snapshot, the ring buffer of
//! recent deltas and the committed holds sit behind one `RwLock` that is held
//! only to clone or swap `Arc`s, never across an await. Live frames go out on a
//! `tokio::sync::broadcast` channel, encoded once and shared by every
//! subscriber.
//!
//! [`EventState::take_batch_into`]: crate::state::EventState

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use cavea_format::wire::{self, Change, Hello, ServerFrame, View, WireError};
use tokio::sync::broadcast;
use tracing::{debug, error};

use crate::actor::KeepAlive;
use crate::state::{EventState, InvariantViolation};
use crate::types::{EventId, Idx, Timestamp, TokenHash};

/// Deltas are published at most once per interval (08 §8.3)...
pub const INTERVAL_MS: u64 = 100;
/// ...stretched with more than this many subscribers on the event...
pub const STRETCH_SUBSCRIBERS: usize = 20_000;
/// ...up to this.
pub const MAX_INTERVAL_MS: u64 = 500;
/// Delta frames older than this are dropped from the ring buffer.
pub const RING_MS: u64 = 60_000;
/// The ring buffer's frames take at most this many bytes per event; the
/// oldest are dropped first.
pub const RING_BYTES: usize = 4 << 20;
/// Publications a subscriber may fall behind before it lags (08 §8.3).
pub const QUEUE_FRAMES: usize = 64;

/// The publication interval for an event with `subscribers` subscribers:
/// 100 ms up to 20,000, then growing in proportion, so that the frames sent per
/// second for the event stay at 200,000, until it reaches 500 ms at 100,000.
#[must_use]
pub fn interval_ms(subscribers: usize) -> u64 {
    let stretched = (subscribers as u64).saturating_mul(INTERVAL_MS) / STRETCH_SUBSCRIBERS as u64;
    stretched.clamp(INTERVAL_MS, MAX_INTERVAL_MS)
}

/// One encoded server frame, shared: cloning it copies no bytes.
#[derive(Clone)]
pub struct Frame {
    seq: u64,
    bytes: Arc<[u8]>,
}

impl Frame {
    fn new(seq: u64, bytes: Vec<u8>) -> Self {
        Self {
            seq,
            bytes: bytes.into(),
        }
    }

    /// The `seq` a receiver has after this frame: a SNAPSHOT's `seq`, a
    /// DELTA's `to_seq`, or the `seq` a PRIVATE frame was taken at.
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl AsRef<[u8]> for Frame {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("type", &self.bytes.first())
            .field("seq", &self.seq)
            .field("len", &self.bytes.len())
            .finish()
    }
}

/// Why a subscription ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// The actor stopped after committing and publishing everything it had
    /// accepted: the server is shutting down (CLOSE 3), or the event was
    /// evicted as the subscriber attached (subscribe again).
    Stopped,
    /// The actor failed (a commit failed, it was fenced, or it panicked). The
    /// event reloads on the next request: subscribe again with
    /// [`Subscription::seq`], which resumes or sends a snapshot.
    Failed,
}

/// A token's committed hold: expiry and objects, ascending.
#[derive(Debug)]
struct Holding {
    expires_at: Timestamp,
    objects: Box<[Idx]>,
}

/// Holds touched in one publication: `None` for a token with no hold now.
type Touched = HashMap<TokenHash, Option<Arc<Holding>>>;

/// What one publication broadcasts.
#[derive(Clone)]
struct Publication {
    delta: Frame,
    holds: Option<Arc<Touched>>,
}

struct Shared {
    /// `None` only if the first snapshot could not be encoded.
    snapshot: Option<Frame>,
    /// `(from_seq, published at, frame)`; contiguous, ending at the
    /// snapshot's `seq`.
    ring: VecDeque<(u64, Timestamp, Frame)>,
    ring_bytes: usize,
    /// Every live hold as of the snapshot's `seq`.
    holds: HashMap<TokenHash, Arc<Holding>>,
    /// `None` once the actor has ended.
    tx: Option<broadcast::Sender<Publication>>,
    end: Option<End>,
}

impl Shared {
    fn prune(&mut self, now: Timestamp) {
        while let Some((_, at, frame)) = self.ring.front() {
            let old = now.0.saturating_sub(at.0) > RING_MS;
            if !old && self.ring_bytes <= RING_BYTES {
                break;
            }
            self.ring_bytes -= frame.bytes.len();
            self.ring.pop_front();
        }
    }

    /// The frames from `last_seq` to the snapshot's `seq`: none if equal, the
    /// buffered deltas if they reach back to it, otherwise the snapshot. A
    /// `last_seq` of 0 means none seen: always the snapshot.
    fn since(&self, last_seq: u64) -> Option<(u64, Vec<Frame>)> {
        let snapshot = self.snapshot.as_ref()?;
        let frames = if last_seq == 0 {
            vec![snapshot.clone()]
        } else if last_seq == snapshot.seq {
            Vec::new()
        } else if let Ok(i) = self
            .ring
            .binary_search_by_key(&last_seq, |(from, _, _)| *from)
        {
            self.ring.range(i..).map(|(_, _, f)| f.clone()).collect()
        } else {
            vec![snapshot.clone()]
        };
        Some((snapshot.seq, frames))
    }
}

/// One event's published state, shared by the actor and every reader.
pub(crate) struct Feed {
    event: EventId,
    objects: u32,
    chart_hash: [u8; 16],
    subscribers: AtomicUsize,
    shared: RwLock<Shared>,
}

impl fmt::Debug for Feed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Feed")
            .field("objects", &self.objects)
            .field("subscribers", &self.subscribers())
            .finish_non_exhaustive()
    }
}

impl Feed {
    fn read(&self) -> RwLockReadGuard<'_, Shared> {
        self.shared.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, Shared> {
        self.shared.write().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn subscribers(&self) -> usize {
        self.subscribers.load(Ordering::Relaxed)
    }

    /// The current public SNAPSHOT.
    pub(crate) fn snapshot(&self) -> Option<Frame> {
        self.read().snapshot.clone()
    }

    /// For polling: `None` if `after_seq` is current; one DELTA from it if
    /// the ring buffer reaches back to it (buffered deltas merged); otherwise
    /// the SNAPSHOT.
    pub(crate) fn after(&self, after_seq: u64) -> Option<Frame> {
        let (seq, frames, snapshot) = {
            let shared = self.read();
            let (seq, frames) = shared.since(after_seq)?;
            (seq, frames, shared.snapshot.clone()?)
        };
        match frames.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            many => Some(self.merge(after_seq, seq, many).unwrap_or_else(|e| {
                error!(error = %e, "buffered deltas did not merge; sending the snapshot");
                snapshot
            })),
        }
    }

    /// One public DELTA equivalent to the contiguous `frames`.
    fn merge(&self, from: u64, to: u64, frames: &[Frame]) -> Result<Frame, WireError> {
        let mut changes = Vec::new();
        for frame in frames {
            match wire::decode_server(&frame.bytes, View::Public)? {
                ServerFrame::Delta(d) => changes.extend(d.changes(self.objects)?),
                _ => return Err(WireError::UnknownType),
            }
        }
        wire::coalesce(&mut changes);
        let bytes = wire::encode_delta(View::Public, from, to, &changes, self.objects, &[])?;
        Ok(Frame::new(to, bytes))
    }

    /// Attach a subscriber that last saw `last_seq`. `Err` if the actor has
    /// ended.
    pub(crate) fn attach(
        self: &Arc<Self>,
        last_seq: u64,
        token: Option<TokenHash>,
        keep: KeepAlive,
        now: Timestamp,
    ) -> Result<Subscription, End> {
        let (rx, seq, first, holding) = {
            let shared = self.read();
            let ended = shared.end.unwrap_or(End::Failed);
            let tx = shared.tx.as_ref().ok_or(ended)?;
            let (seq, first) = shared.since(last_seq).ok_or(ended)?;
            let holding = token.map(|t| shared.holds.get(&t).cloned());
            // Under the same read lock as the frames above: publication takes
            // the write lock, so the receiver starts right after them.
            (tx.subscribe(), seq, first, holding)
        };
        let subscribers = self.subscribers.fetch_add(1, Ordering::Relaxed) + 1;
        let mut sub = Subscription {
            hello: Hello {
                server_time_ms: now.0,
                chart_hash: self.chart_hash,
                objects: self.objects,
            },
            first,
            feed: self.clone(),
            rx,
            token,
            private: None,
            seq,
            lagged: false,
            _keep: keep,
        };
        if let Some(holding) = holding {
            sub.first.extend(sub.private_frame(holding.as_deref()));
        }
        debug!(
            event_id = %format_args!("{:032x}", self.event.0),
            last_seq,
            seq,
            frames = sub.first.len(),
            subscribers,
            "subscriber attached"
        );
        Ok(sub)
    }
}

/// What [`Subscription::next`] returns.
#[derive(Debug, Clone)]
pub enum Received {
    /// Send this frame.
    Frame(Frame),
    /// The subscriber fell more than [`QUEUE_FRAMES`] publications behind and
    /// frames were dropped. It gets nothing more until
    /// [`Subscription::resync`].
    Lagged,
    /// The actor ended; nothing more will come.
    Ended(End),
}

/// One realtime subscriber of an event. While it lives, the event's actor is
/// not evicted. Its memory is a broadcast receiver and a few fields: every
/// frame is shared.
pub struct Subscription {
    /// Send first, then [`Self::first`].
    pub hello: Hello,
    /// After HELLO: the buffered DELTAs from the subscriber's `last_seq` or a
    /// SNAPSHOT (nothing if it is up to date), then PRIVATE if it gave a hold
    /// token.
    pub first: Vec<Frame>,
    feed: Arc<Feed>,
    rx: broadcast::Receiver<Publication>,
    token: Option<TokenHash>,
    /// The PRIVATE frame that follows the DELTA just returned.
    private: Option<Frame>,
    seq: u64,
    lagged: bool,
    _keep: KeepAlive,
}

impl fmt::Debug for Subscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Subscription")
            .field("seq", &self.seq)
            .field("lagged", &self.lagged)
            .finish_non_exhaustive()
    }
}

impl Subscription {
    /// The `seq` the subscriber has once it has applied every frame handed
    /// out so far, [`Self::first`] included.
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// The next frame to send. Each DELTA continues from the previous one
    /// (its `from_seq` is the `seq` before it). A PRIVATE frame for the
    /// subscriber's token follows the DELTA of the same publication, never
    /// precedes it. Cancel-safe.
    pub async fn next(&mut self) -> Received {
        if self.lagged {
            return Received::Lagged;
        }
        if let Some(private) = self.private.take() {
            return Received::Frame(private);
        }
        loop {
            match self.rx.recv().await {
                // Published between the attach's read and its receiver's
                // first message: already covered by what it was given.
                Ok(p) if p.delta.seq <= self.seq => {}
                Ok(p) => {
                    self.seq = p.delta.seq;
                    let mine = self.token.and_then(|t| p.holds.as_ref()?.get(&t).cloned());
                    if let Some(holding) = mine {
                        self.private = self.private_frame(holding.as_deref());
                    }
                    return Received::Frame(p.delta);
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    self.lagged = true;
                    debug!(skipped, "subscriber lagged");
                    return Received::Lagged;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    let end = self.feed.read().end.unwrap_or(End::Failed);
                    return Received::Ended(end);
                }
            }
        }
    }

    /// Start over from the current SNAPSHOT (and PRIVATE, with a token): the
    /// way out of [`Received::Lagged`]. Frames from [`Self::next`] continue
    /// from the snapshot.
    pub fn resync(&mut self) -> Result<Vec<Frame>, End> {
        let (rx, snapshot, holding) = {
            let shared = self.feed.read();
            let ended = shared.end.unwrap_or(End::Failed);
            let tx = shared.tx.as_ref().ok_or(ended)?;
            let snapshot = shared.snapshot.clone().ok_or(ended)?;
            let holding = self.token.map(|t| shared.holds.get(&t).cloned());
            (tx.subscribe(), snapshot, holding)
        };
        self.rx = rx;
        self.seq = snapshot.seq;
        self.lagged = false;
        self.private = None;
        let mut frames = vec![snapshot];
        if let Some(holding) = holding {
            frames.extend(self.private_frame(holding.as_deref()));
        }
        Ok(frames)
    }

    fn private_frame(&self, holding: Option<&Holding>) -> Option<Frame> {
        let (expires_at, objects) = holding.map_or((0, &[][..]), |h| (h.expires_at.0, &h.objects));
        match wire::encode_private(expires_at, objects, self.feed.objects, &[]) {
            Ok(bytes) => Some(Frame::new(self.seq, bytes)),
            Err(e) => {
                error!(error = %e, "PRIVATE frame not encoded");
                None
            }
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let left = self.feed.subscribers.fetch_sub(1, Ordering::Relaxed) - 1;
        debug!(
            event_id = %format_args!("{:032x}", self.feed.event.0),
            subscribers = left,
            "subscriber detached"
        );
    }
}

/// A token's hold as captured: expiry, and its objects as a range of a list.
pub(crate) type HeldRange = Option<(Timestamp, Range<usize>)>;

/// What one commit changed, as of its last command, captured when the batch
/// was sealed and folded in only once it has committed.
#[derive(Debug, Default)]
pub(crate) struct Sealed {
    pub(crate) seq: u64,
    /// Each changed object's final domain status.
    pub(crate) changes: Vec<Change>,
    /// Each token a command of the batch changed the hold of, with that hold's
    /// expiry and objects (a range of `held`), or `None` if it has none.
    pub(crate) holds: Vec<(TokenHash, HeldRange)>,
    pub(crate) held: Vec<Idx>,
}

impl Sealed {
    pub(crate) fn clear(&mut self) {
        self.changes.clear();
        self.holds.clear();
        self.held.clear();
    }
}

/// The actor's side: committed state and the changes not yet published.
pub(crate) struct Publisher {
    feed: Arc<Feed>,
    tx: broadcast::Sender<Publication>,
    /// Committed domain statuses.
    status: Vec<u8>,
    /// Public codes as last published, one bit per object.
    shown: Vec<u64>,
    /// Committed `seq`.
    seq: u64,
    /// `seq` of the current snapshot and the last delta's `to_seq`.
    published: u64,
    last_publish: Option<Timestamp>,
    /// Committed changes since `published`, in commit order.
    changes: Vec<Change>,
    /// Committed holds of tokens touched since `published`.
    holds: HashMap<TokenHash, HeldRange>,
    held: Vec<Idx>,
}

fn wire_corrupt(_: WireError) -> InvariantViolation {
    InvariantViolation("published state did not encode")
}

impl Publisher {
    /// The feed of a freshly loaded `state`, all of which is committed, with
    /// its first snapshot published. `Err` (with a feed that has ended) if
    /// that snapshot cannot be encoded.
    pub(crate) fn new(state: &EventState) -> (Arc<Feed>, Result<Self, InvariantViolation>) {
        let chart = state.chart();
        let status = state.statuses().to_vec();
        let objects = u32::try_from(status.len()).unwrap_or(u32::MAX);
        let snapshot = wire::encode_snapshot(View::Public, state.seq(), &status, &[]);
        let holds = state
            .holds()
            .map(|h| {
                let mut objects: Box<[Idx]> = h.objects.into();
                objects.sort_unstable();
                let holding = Holding {
                    expires_at: h.expires_at,
                    objects,
                };
                (h.token, Arc::new(holding))
            })
            .collect();
        let (tx, _) = broadcast::channel(QUEUE_FRAMES);
        let ok = snapshot.is_ok();
        let feed = Arc::new(Feed {
            event: state.event(),
            objects,
            chart_hash: chart.hash(),
            subscribers: AtomicUsize::new(0),
            shared: RwLock::new(Shared {
                snapshot: snapshot.ok().map(|b| Frame::new(state.seq(), b)),
                ring: VecDeque::new(),
                ring_bytes: 0,
                holds,
                tx: ok.then(|| tx.clone()),
                end: (!ok).then_some(End::Failed),
            }),
        });
        if !ok {
            return (feed, Err(wire_corrupt(WireError::BadCount)));
        }
        let mut shown = vec![0u64; status.len().div_ceil(64)];
        for (word, chunk) in shown.iter_mut().zip(status.chunks(64)) {
            for (bit, &s) in chunk.iter().enumerate() {
                *word |= u64::from(s != 0) << bit;
            }
        }
        let publisher = Self {
            feed: feed.clone(),
            tx,
            status,
            shown,
            seq: state.seq(),
            published: state.seq(),
            last_publish: None,
            changes: Vec::new(),
            holds: HashMap::new(),
            held: Vec::new(),
        };
        (feed, Ok(publisher))
    }

    /// Fold in a batch that has committed.
    pub(crate) fn commit(&mut self, sealed: &Sealed) {
        for &change in &sealed.changes {
            if let Some(s) = self.status.get_mut(change.idx as usize)
                && *s != change.code
            {
                *s = change.code;
                self.changes.push(change);
            }
        }
        for (token, hold) in &sealed.holds {
            let hold = hold.as_ref().map(|(expires_at, range)| {
                let start = self.held.len();
                self.held
                    .extend_from_slice(sealed.held.get(range.clone()).unwrap_or_default());
                (*expires_at, start..self.held.len())
            });
            self.holds.insert(*token, hold);
        }
        self.seq = self.seq.max(sealed.seq);
    }

    /// When the next publication is due, or `None` if everything committed is
    /// published. The first commit after a quiet interval is due at once.
    pub(crate) fn due(&self) -> Option<Timestamp> {
        if self.seq == self.published {
            return None;
        }
        let interval = interval_ms(self.feed.subscribers());
        Some(
            self.last_publish
                .map_or(Timestamp(0), |t| Timestamp(t.0.saturating_add(interval))),
        )
    }

    /// Publish everything committed since the last publication: one DELTA,
    /// the SNAPSHOT at its `to_seq`, and the touched tokens' holds.
    pub(crate) fn publish(&mut self, now: Timestamp) -> Result<(), InvariantViolation> {
        if let Some(publication) = self.stage(now)? {
            // Outside the lock: a subscriber attached since the swap already
            // has this publication and skips it by its `seq`.
            let _ = self.tx.send(publication);
        }
        Ok(())
    }

    /// Encode the publication and swap it in for readers; the caller
    /// broadcasts it.
    fn stage(&mut self, now: Timestamp) -> Result<Option<Publication>, InvariantViolation> {
        if self.seq == self.published {
            return Ok(None);
        }
        wire::coalesce(&mut self.changes);
        // Drop what the public cannot see: a code the subscribers already
        // have, such as held to booked.
        let shown = &mut self.shown;
        self.changes.retain(|c| {
            let (i, bit) = (c.idx as usize / 64, 1u64 << (c.idx % 64));
            let Some(word) = shown.get_mut(i) else {
                return false;
            };
            let visible = (*word & bit != 0) != (c.code != 0);
            if visible {
                *word ^= bit;
            }
            visible
        });
        let objects = self.feed.objects;
        let delta = wire::encode_delta(
            View::Public,
            self.published,
            self.seq,
            &self.changes,
            objects,
            &[],
        )
        .map_err(wire_corrupt)?;
        let snapshot = wire::encode_snapshot(View::Public, self.seq, &self.status, &[])
            .map_err(wire_corrupt)?;
        let held = &self.held;
        let holds: Touched = self
            .holds
            .drain()
            .map(|(token, hold)| {
                let hold = hold.map(|(expires_at, range)| {
                    let objects = held.get(range).unwrap_or_default().into();
                    Arc::new(Holding {
                        expires_at,
                        objects,
                    })
                });
                (token, hold)
            })
            .collect();
        let delta = Frame::new(self.seq, delta);
        let holds = (!holds.is_empty()).then(|| Arc::new(holds));
        {
            let mut shared = self.feed.write();
            shared.snapshot = Some(Frame::new(self.seq, snapshot));
            shared.ring_bytes += delta.bytes.len();
            shared.ring.push_back((self.published, now, delta.clone()));
            shared.prune(now);
            for (token, hold) in holds.iter().flat_map(|h| h.iter()) {
                match hold {
                    Some(hold) => shared.holds.insert(*token, hold.clone()),
                    None => shared.holds.remove(token),
                };
            }
        }
        self.published = self.seq;
        self.last_publish = Some(now);
        self.changes.clear();
        self.held.clear();
        Ok(Some(Publication { delta, holds }))
    }

    /// Drop expired frames from the ring buffer while no delta is published.
    pub(crate) fn prune(&self, now: Timestamp) {
        self.feed.write().prune(now);
    }

    /// End every subscription with `end`.
    pub(crate) fn end(self, end: End) {
        self.feed.write().end.get_or_insert(end);
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        // Every path out of the actor, a panic included. Receivers close once
        // the last sender, ours, is dropped after this.
        let mut shared = self.feed.write();
        shared.end.get_or_insert(End::Failed);
        shared.tx = None;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]

    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    use super::*;
    use crate::actor::{ActorConfig, actor};
    use crate::chart::ChartView;
    use crate::memory::MemoryStore;
    use crate::store::LoadedEvent;
    use crate::types::{Authority, Command, ManualClock, ObjectRef, Request, Status};

    fn state() -> EventState {
        let chart = ChartView::new((0..8).map(|i| (i, format!("S-{i}")))).unwrap();
        let loaded = LoadedEvent {
            epoch: 1,
            seq: 0,
            holds: Vec::new(),
            objects: Vec::new(),
            idempotency: Vec::new(),
        };
        EventState::restore(EventId(1), Arc::new(chart), loaded).unwrap()
    }

    fn keep() -> KeepAlive {
        let (handle, _run) = actor(
            state(),
            Arc::new(MemoryStore::new()),
            ManualClock::default(),
            ActorConfig::default(),
        );
        handle.keep_alive()
    }

    fn commit_block(state: &mut EventState, publisher: &mut Publisher, idx: u32) {
        let request = Request {
            actor: "sk".into(),
            authority: Authority::Secret,
            command: Command::ChangeStatus {
                objects: vec![ObjectRef::Idx(idx)],
                status: Status::BLOCKED,
            },
            idempotency: None,
        };
        state.apply(&request, Timestamp(0)).unwrap().unwrap();
        let mut sealed = Sealed::default();
        state.take_batch_into(&mut sealed).unwrap().unwrap();
        publisher.commit(&sealed);
    }

    fn poll_next(sub: &mut Subscription) -> Option<Received> {
        let mut next = std::pin::pin!(sub.next());
        match next.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(r) => Some(r),
            Poll::Pending => None,
        }
    }

    fn seq_of(r: Option<Received>) -> Option<u64> {
        match r {
            Some(Received::Frame(f)) => Some(f.seq()),
            _ => None,
        }
    }

    /// The actor swaps a publication in, then broadcasts it outside the lock.
    /// A subscriber attaching in between gets it from the swap and must not
    /// get it again from the broadcast; one attaching before gets it once.
    #[test]
    fn a_subscriber_attached_between_swap_and_broadcast_gets_each_frame_once() {
        let mut state = state();
        let (feed, publisher) = Publisher::new(&state);
        let mut publisher = publisher.unwrap();
        let mut before = feed.attach(0, None, keep(), Timestamp(0)).unwrap();
        assert_eq!(before.seq(), 0);

        commit_block(&mut state, &mut publisher, 1);
        let publication = publisher.stage(Timestamp(0)).unwrap().unwrap();
        let mut between = feed.attach(0, None, keep(), Timestamp(0)).unwrap();
        assert_eq!(
            between.first.iter().map(Frame::seq).collect::<Vec<_>>(),
            [1]
        );
        publisher.tx.send(publication).ok().unwrap();

        assert_eq!(seq_of(poll_next(&mut before)), Some(1));
        assert!(poll_next(&mut between).is_none(), "already had seq 1");

        commit_block(&mut state, &mut publisher, 2);
        publisher.publish(Timestamp(100)).unwrap();
        assert_eq!(seq_of(poll_next(&mut before)), Some(2));
        assert_eq!(seq_of(poll_next(&mut between)), Some(2));
    }
}
