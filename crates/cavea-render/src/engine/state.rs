//! Seat state from the host (04 §17): snapshots, deltas and the buyer's own
//! holds.
//!
//! State arrives decoded ([`Command::from_frame`] for server frames, or typed
//! arrays from a custom adapter) and is checked here against the chart: a
//! snapshot of the wrong object count, a code above 1 or an `idx` past the
//! objects is an `InvalidInput` error that changes nothing. Objects past the
//! seats (tables, booths, areas) are not drawn yet; their codes are ignored.

use super::*;
use crate::{protocol::StatusChange, tuning::MAX_SELECTED};

#[derive(Debug, Default)]
pub(super) struct State {
    /// The `seq` of the state shown; `None` before the first snapshot.
    pub(super) seq: Option<u64>,
    /// `Resync` was emitted and no snapshot has arrived since.
    resync_asked: bool,
    /// State that arrived before any chart, applied when one loads (04 §4).
    early_snapshot: Option<(u64, Vec<u8>)>,
    early_private: Option<(Vec<u32>, u64)>,
}

impl<B: GpuBackend> Engine<B> {
    /// Objects in the loaded chart: seats, then tables, booths and areas.
    pub(super) fn objects(&self) -> Option<usize> {
        let chart = &self.scene.as_ref()?.scene.chart;
        Some(chart.meta.seat_count as usize + chart.objects.len())
    }

    /// The loaded chart's state is forgotten; state kept from before the
    /// first chart is applied to it.
    pub(super) fn chart_loaded(&mut self) {
        let early = std::mem::take(&mut self.state);
        if let Some((seq, codes)) = early.early_snapshot {
            self.set_snapshot(seq, codes);
        }
        if let Some((objects, expiry)) = early.early_private {
            self.set_private_holds(objects, expiry);
        }
    }

    pub(super) fn set_snapshot(&mut self, seq: u64, codes: Vec<u8>) {
        let Some(objects) = self.objects() else {
            self.state.early_snapshot = Some((seq, codes));
            return;
        };
        if codes.len() != objects || codes.iter().any(|&c| c > status::UNAVAILABLE) {
            let message = "snapshot: one code per object of the chart, 0 or 1";
            return self.error(ErrorCode::InvalidInput, message);
        }
        self.state.seq = Some(seq);
        self.state.resync_asked = false;
        self.selection.released.clear();
        let Some(loaded) = &mut self.scene else {
            return;
        };
        let scene = &mut loaded.scene;
        let seats = scene.status.len();
        scene
            .status
            .copy_from_slice(codes.get(..seats).unwrap_or_default());
        for (v, &s) in scene.visual.iter_mut().zip(&scene.status) {
            *v = picker_visual(s, Selection::None);
        }
        grow_dirty(&mut loaded.dirty, 0..seats);
        self.redraw();
        for idx in self.selection.seats() {
            self.refresh(idx);
        }
        self.knock_out(None);
    }

    pub(super) fn apply_delta(&mut self, from_seq: u64, to_seq: u64, changes: &[StatusChange]) {
        let Some(objects) = self.objects() else {
            return self.resync();
        };
        let mut next = 0;
        let ok = changes.iter().all(|c| {
            let ok = c.idx >= next && c.status <= status::UNAVAILABLE;
            next = c.idx.saturating_add(1);
            ok
        });
        if !ok || next as usize > objects {
            let message = "delta changes: in increasing idx within the chart, codes 0 or 1";
            return self.error(ErrorCode::InvalidInput, message);
        }
        if to_seq <= from_seq {
            return self.error(ErrorCode::InvalidInput, "delta to_seq not above from_seq");
        }
        if self.state.seq != Some(from_seq) {
            return self.resync();
        }
        self.state.seq = Some(to_seq);
        let named = |idx: u32| changes.binary_search_by_key(&idx, |c| c.idx).is_ok();
        // A status change for a released seat is newer than the release.
        let released = &mut self.selection.released;
        let unreleased: Vec<u32> = released.iter().copied().filter(|&i| named(i)).collect();
        released.retain(|&idx| !named(idx));
        let Some(loaded) = &mut self.scene else {
            return;
        };
        let scene = &mut loaded.scene;
        let mut span: Option<Range<usize>> = None;
        for c in changes {
            let i = c.idx as usize;
            let (Some(status), Some(visual)) = (scene.status.get_mut(i), scene.visual.get_mut(i))
            else {
                break;
            };
            if *status != c.status {
                *status = c.status;
                *visual = picker_visual(c.status, Selection::None);
                span = Some(span.map_or(i..i + 1, |s| s.start..i + 1));
            }
        }
        if let Some(span) = span {
            grow_dirty(&mut loaded.dirty, span);
            self.redraw();
        }
        let seats = self.selection.seats().into_iter().filter(|&i| named(i));
        for idx in seats.chain(unreleased) {
            self.refresh(idx);
        }
        self.knock_out(Some(changes));
    }

    pub(super) fn set_private_holds(&mut self, mut objects: Vec<u32>, expires_at_ms: u64) {
        let Some(count) = self.objects() else {
            self.state.early_private = Some((objects, expires_at_ms));
            return;
        };
        let increasing = objects.windows(2).all(|w| w.first() < w.get(1));
        if objects.len() > MAX_SELECTED as usize
            || !increasing
            || objects.last().is_some_and(|&i| i as usize >= count)
        {
            let message = "private holds: at most 100, in increasing idx within the chart";
            return self.error(ErrorCode::InvalidInput, message);
        }
        let seats = self.scene.as_ref().map_or(0, |l| l.scene.status.len());
        objects.retain(|&i| (i as usize) < seats);
        self.set_private(objects, expires_at_ms);
    }

    fn resync(&mut self) {
        if !std::mem::replace(&mut self.state.resync_asked, true) {
            let seq = self.state.seq;
            self.events.push(Event::Resync { seq });
        }
    }
}
