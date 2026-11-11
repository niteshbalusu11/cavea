//! Seat state from the host (04 §17): snapshots, deltas, the buyer's own
//! holds, and the pulse of seats that just became unavailable.
//!
//! State arrives decoded ([`Command::from_frame`] for server frames, or typed
//! arrays from a custom adapter) and is checked here against the chart: a
//! snapshot of the wrong object count, a code above 1 or an `idx` past the
//! objects is an `InvalidInput` error that changes nothing. Objects past the
//! seats (tables, booths, areas) are not drawn yet; their codes are ignored.

use super::*;
use crate::{
    protocol::StatusChange,
    theme::visual,
    tuning::{MAX_SELECTED, PULSE_MS},
};

/// `fx` bit 2 (04 §8).
const FX_PULSE: u8 = 4;

#[derive(Debug, Default)]
pub(super) struct State {
    /// The `seq` of the state shown; `None` before the first snapshot.
    pub(super) seq: Option<u64>,
    /// `Resync` was emitted and no snapshot has arrived since.
    resync_asked: bool,
    /// State that arrived before any chart, applied when one loads (04 §4).
    early_snapshot: Option<(u64, Vec<u8>)>,
    early_private: Option<(Vec<u32>, u64)>,
    /// Seats pulsing, oldest first, one batch per frame's deltas.
    pub(super) pulses: Vec<Pulse>,
}

#[derive(Debug)]
pub(super) struct Pulse {
    seats: Vec<u32>,
    /// The dirty range that covers `seats`.
    span: Range<usize>,
    /// Set by the first `Tick` after the batch began.
    end_ms: Option<f64>,
}

impl<B: GpuBackend> Engine<B> {
    /// Objects in the loaded chart: seats, then tables, booths and areas.
    pub fn objects(&self) -> Option<usize> {
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
        for idx in std::mem::take(&mut self.selection.released) {
            self.selection.confirm_release(idx);
        }
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
        loaded.sections.recount(&scene.chart, &scene.visual);
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
        let released = &self.selection.released;
        let unreleased: Vec<u32> = released.iter().copied().filter(|&i| named(i)).collect();
        for &idx in &unreleased {
            self.selection.confirm_release(idx);
        }
        // Seats pulse only where they are drawn: not at overview (04 §10).
        let pulsing = !self.reduced_motion && self.seat_alpha() > 0.0;
        let Some(loaded) = &mut self.scene else {
            return;
        };
        let scene = &mut loaded.scene;
        let mut pulse = Vec::new();
        let mut span: Option<Range<usize>> = None;
        for c in changes {
            let i = c.idx as usize;
            let section = scene.section_of(i);
            let (Some(status), Some(visual)) = (scene.status.get_mut(i), scene.visual.get_mut(i))
            else {
                break;
            };
            if *status != c.status {
                *status = c.status;
                let was = *visual;
                if pulsing && was == visual::AVAILABLE {
                    pulse.push(c.idx);
                }
                *visual = picker_visual(c.status, Selection::None);
                loaded.sections.change(section, was, *visual);
                span = Some(span.map_or(i..i + 1, |s| s.start..i + 1));
            }
        }
        if let Some(span) = span.clone() {
            grow_dirty(&mut loaded.dirty, span);
            self.redraw();
        }
        let seats = self.selection.seats().into_iter().filter(|&i| named(i));
        for idx in seats.chain(unreleased) {
            self.refresh(idx);
        }
        if let (true, Some(loaded), Some(span)) = (pulsing, &mut self.scene, span) {
            let scene = &mut loaded.scene;
            pulse.retain(|&i| scene.visual.get(i as usize) == Some(&visual::UNAVAILABLE));
            for &i in &pulse {
                if let Some(fx) = scene.fx.get_mut(i as usize) {
                    *fx |= FX_PULSE;
                }
            }
            match self.state.pulses.last_mut() {
                Some(p) if p.end_ms.is_none() => {
                    p.seats.extend(pulse);
                    p.span = p.span.start.min(span.start)..p.span.end.max(span.end);
                }
                _ if !pulse.is_empty() => self.state.pulses.push(Pulse {
                    seats: pulse,
                    span,
                    end_ms: None,
                }),
                _ => {}
            }
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

    /// Starts the newest pulse batch at `now_ms`, ends those past their time
    /// (all of them with `None`), and keeps frames coming while any run.
    pub(super) fn step_pulses(&mut self, now_ms: Option<f64>) {
        let pulses = &mut self.state.pulses;
        if let (Some(now), Some(last)) = (now_ms, pulses.last_mut()) {
            last.end_ms.get_or_insert(now + PULSE_MS);
        }
        let Some(loaded) = &mut self.scene else {
            return pulses.clear();
        };
        if pulses.is_empty() {
            return;
        }
        while let Some(p) = pulses.first() {
            if now_ms.is_some_and(|now| p.end_ms.is_none_or(|end| now < end)) {
                break;
            }
            let p = pulses.remove(0);
            for i in p.seats {
                if let Some(fx) = loaded.scene.fx.get_mut(i as usize) {
                    *fx &= !FX_PULSE;
                }
            }
            grow_dirty(&mut loaded.dirty, p.span);
        }
        // The shader animates pulses with the frame time.
        self.dirty = true;
        if !self.state.pulses.is_empty() {
            self.want_frame();
        }
    }
}
