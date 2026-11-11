//! Taps, hover and focus (04 §15) and the selection engine (04 §16).
//!
//! Per seat, the selection is one of: not selected; selected and pending (its
//! hold not yet requested, or in flight); selected and held. The engine keeps
//! the selected seats in order (at most `MAX_SELECTED`) and three lists for the
//! hold protocol: seats whose hold request has not gone out, the one request
//! in flight, and held seats deselected since whose release has not gone out.
//! At most one hold request is in flight; everything else waits for its
//! result and goes out in the next requests ("coalescing", 04 §16):
//!
//! - Taps of one frame, and every tap made while a hold is in flight, go out
//!   together in the next `HoldRequest`.
//! - A seat tapped on and off again before its request went out is in no
//!   request at all.
//! - A seat deselected while its hold is in flight stays in that request. If
//!   the hold succeeds it is released in the next `ReleaseRequest`; if not,
//!   nothing is sent. Tapped on again before the result, it is simply
//!   selected again and the result applies to it.
//! - A held seat deselected while a hold is in flight is released after the
//!   result; tapped on again before that, it is held again with no request.
//! - Releases go out before holds, and only when no hold is in flight, so a
//!   release never overtakes the hold of the same seat. Releases have no
//!   result; the host sends requests in the order emitted.
//!
//! - A seat deselected after its release went out is "released" until a
//!   private set without it, or a status change for it, arrives.
//!
//! Own holds: the buyer's hold makes a seat's public status unavailable, so
//! the engine tells its own seats from others' (04 §17): held seats, seats
//! whose hold is in flight, and seats in the private set (`SetPrivateHolds`)
//! are never knocked out by a status change. A seat that enters the private
//! set is held (selected again after a reload); a held seat that leaves it
//! leaves the selection with `HoldLost`. Seats the buyer gave up (to release,
//! released, or deselected in flight) are drawn available, can be selected
//! again, and are ignored by the private set until it confirms the release.

use cavea_core::rules::Rules;
use cavea_format::full_label::{LabelIndex, full_label, seat_label};

use super::*;
use crate::{
    hit::hit_test,
    protocol::{HoldOutcome, ObjectInfo, ObjectRef, SessionMode, StatusChange},
    tuning::{
        HIT_TOLERANCE_MOUSE_PX, HIT_TOLERANCE_TOUCH_PX, MAX_SELECTED, SELECT_MIN_SEAT_MOUSE_PX,
        SELECT_MIN_SEAT_TOUCH_PX, ZOOM_TO_SELECT_SEAT_PX,
    },
};

/// Server failure codes that mean someone else has the object (08 §6.4).
const NOT_FREE: [&str; 2] = ["not_free", "object_not_free"];
/// The failure code for a hold token that expired or never existed (08 §6.4).
const TOKEN_GONE: &str = "not_found";

#[derive(Debug, Clone, Copy)]
struct Selected {
    idx: u32,
    /// The hold is confirmed; always so in a manual session.
    held: bool,
}

#[derive(Debug)]
struct InFlight {
    request: u32,
    seats: Vec<u32>,
    /// Seats of `seats` the buyer deselected after the request went out.
    dropped: Vec<u32>,
}

#[derive(Debug)]
pub(super) struct SelectState {
    selected: Vec<Selected>,
    to_hold: Vec<u32>,
    to_release: Vec<u32>,
    in_flight: Option<InFlight>,
    pub(super) released: Vec<u32>,
    /// The last private set, ascending, and its expiry if it is not empty.
    pub(super) private: Vec<u32>,
    expiry: Option<u64>,
    next_request: u32,
    mode: SessionMode,
    rules: Rules,
    /// Whether the selection is ready for checkout, as last reported.
    valid: bool,
    /// The selection changed since `SelectionChanged` was last emitted.
    changed: bool,
}

impl Default for SelectState {
    fn default() -> Self {
        Self {
            selected: Vec::new(),
            to_hold: Vec::new(),
            to_release: Vec::new(),
            in_flight: None,
            released: Vec::new(),
            private: Vec::new(),
            expiry: None,
            next_request: 1,
            mode: SessionMode::Hold,
            rules: Rules::new(Some(MAX_SELECTED), 0).unwrap_or_default(),
            valid: true,
            changed: false,
        }
    }
}

impl SelectState {
    fn find(&self, idx: u32) -> Option<usize> {
        self.selected.iter().position(|s| s.idx == idx)
    }

    /// Seats the buyer gave up that may still be held under the token.
    fn given_up(&self, idx: u32) -> bool {
        let dropped = self.in_flight.as_ref().map(|f| f.dropped.as_slice());
        has(&self.to_release, idx)
            || has(&self.released, idx)
            || has(dropped.unwrap_or_default(), idx)
    }

    /// Seats held under the buyer's token, or that may be, though not selected:
    /// drawn available and selectable whatever their public status.
    pub(super) fn ours(&self, idx: u32) -> bool {
        self.given_up(idx) || has(&self.private, idx)
    }

    /// Whether a public status of unavailable may be this buyer's own hold
    /// on the selected seat `s`.
    fn own(&self, s: &Selected) -> bool {
        let flight = self.in_flight.as_ref().map(|f| f.seats.as_slice());
        let requested = s.held || has(flight.unwrap_or_default(), s.idx);
        (self.mode == SessionMode::Hold && requested)
            || has(&self.private, s.idx)
            || has(&self.released, s.idx)
    }

    /// A status change or a snapshot newer than the release of `idx` confirms
    /// it: the seat is no longer the buyer's, whatever an older set said.
    pub(super) fn confirm_release(&mut self, idx: u32) {
        remove(&mut self.released, idx);
        remove(&mut self.private, idx);
    }

    /// Every seat whose visual state depends on the selection.
    pub(super) fn seats(&self) -> Vec<u32> {
        let mut seats: Vec<u32> = self.selected.iter().map(|s| s.idx).collect();
        for list in [&self.to_release, &self.released, &self.private] {
            seats.extend_from_slice(list);
        }
        if let Some(f) = &self.in_flight {
            seats.extend_from_slice(&f.dropped);
        }
        seats
    }

    fn request_id(&mut self) -> u32 {
        let id = self.next_request;
        self.next_request = id.wrapping_add(1).max(1);
        id
    }

    fn count(&self) -> u32 {
        // At most `MAX_SELECTED`.
        self.selected.len() as u32
    }
}

/// Whether `list` holds `idx`: one scan for every short list.
#[inline(never)]
fn has(list: &[u32], idx: u32) -> bool {
    list.contains(&idx)
}

fn remove(list: &mut Vec<u32>, idx: u32) -> bool {
    let at = list.iter().position(|&i| i == idx);
    at.map(|at| list.remove(at)).is_some()
}

/// At most 64 bytes of host text, for an error message.
fn excerpt(text: &str) -> &str {
    let mut end = text.len().min(64);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or_default()
}

impl<B: GpuBackend> Engine<B> {
    /// Recognised taps, long presses and hover (04 §14).
    pub(super) fn on_intent(&mut self, intent: Intent) {
        #[cfg(test)]
        self.intents.push(intent);
        match intent {
            Intent::Tap { at, pointer } => {
                let touch = pointer != PointerType::Mouse;
                if touch {
                    // A touch ends the tooltip a long press opened.
                    self.set_hover(None);
                }
                let tolerance = if touch {
                    HIT_TOLERANCE_TOUCH_PX
                } else {
                    HIT_TOLERANCE_MOUSE_PX
                };
                let Some((idx, seat_cu)) = self.hit(at, tolerance) else {
                    return;
                };
                // The keyboard resumes from the tapped seat; the ring hides.
                self.show_focus(false);
                self.focus = Some(idx);
                let min = if touch {
                    SELECT_MIN_SEAT_TOUCH_PX
                } else {
                    SELECT_MIN_SEAT_MOUSE_PX
                };
                match (self.stage(), self.camera) {
                    (Some(stage), Some(camera)) if seat_cu * camera.zoom < min => {
                        // Zoom-to-select: too small to pick reliably.
                        let mut to = Camera {
                            centre: camera.to_world(at, stage.size),
                            zoom: ZOOM_TO_SELECT_SEAT_PX / seat_cu,
                            rotation: camera.rotation,
                        };
                        to.clamp(stage.chart, stage.size, stage.limits);
                        let duration = camera::fly_duration_ms(&camera, &to, stage.size);
                        self.move_to(to, Some(duration));
                    }
                    _ => self.tap(idx),
                }
            }
            Intent::LongPress { at } => {
                let idx = self.hit(at, HIT_TOLERANCE_TOUCH_PX).map(|(idx, _)| idx);
                self.set_hover(idx);
            }
            Intent::Hover(at) => {
                self.hover_at = at;
                self.rehover();
            }
        }
    }

    /// Hit-tests the last hover position again, as after a camera move.
    pub(super) fn rehover(&mut self) {
        if let Some(at) = self.hover_at {
            let idx = self.hit(at, HIT_TOLERANCE_MOUSE_PX).map(|(idx, _)| idx);
            self.set_hover(idx);
        } else if self.hover.is_some() {
            self.set_hover(None);
        }
    }

    /// The seat under the screen point `at` with a tolerance of
    /// `tolerance_px` CSS pixels, and its size in chart units.
    fn hit(&self, at: [f64; 2], tolerance_px: f64) -> Option<(u32, f64)> {
        let (scene, camera, size) = (
            &self.scene.as_ref()?.scene,
            self.camera?,
            self.stage()?.size,
        );
        let [x, y] = camera.to_world(at, size);
        let idx = hit_test(
            scene,
            [x as f32, y as f32],
            (tolerance_px / camera.zoom) as f32,
        )?;
        let row = scene
            .chart
            .rows
            .get(*scene.row.get(idx as usize)? as usize)?;
        Some((idx, f64::from(row.seat_size) * 0.25))
    }

    fn set_hover(&mut self, idx: Option<u32>) {
        if idx != self.hover {
            self.hover = idx;
            let object = idx.and_then(|idx| self.anchored(idx));
            self.events.push(Event::ObjectHover(object));
            self.redraw();
        }
    }

    /// Moves the focus and shows it (`None` removes it), emitting
    /// `ObjectFocus` if that changes what is shown. Keys move it through
    /// `keys`; tests and a new chart call this.
    pub fn set_focus(&mut self, idx: Option<u32>) {
        let seats = self.scene.as_ref().map_or(0, |l| l.scene.pos.len());
        if idx.is_some_and(|idx| idx as usize >= seats) {
            return self.error(ErrorCode::InvalidInput, "focus is not a seat");
        }
        let was = self.focus.filter(|_| self.focus_shown);
        self.focus = idx;
        self.focus_shown = idx.is_some();
        if idx != was {
            self.emit_focus();
            self.redraw();
        }
    }

    /// An object's details and its centre on screen.
    fn anchored(&self, idx: u32) -> Option<(ObjectInfo, [f32; 2])> {
        self.anchored_at(idx, self.camera)
    }

    /// An object's details and its centre on screen under `camera`.
    pub(super) fn anchored_at(
        &self,
        idx: u32,
        camera: Option<Camera>,
    ) -> Option<(ObjectInfo, [f32; 2])> {
        let pos = self.scene.as_ref()?.scene.pos.get(idx as usize)?;
        let at = match (camera, self.stage()) {
            (Some(camera), Some(stage)) => {
                camera.to_screen([f64::from(pos[0]), f64::from(pos[1])], stage.size)
            }
            _ => [0.0; 2],
        };
        Some((self.info(idx), at.map(|v| v as f32)))
    }

    fn label(&self, idx: u32) -> String {
        let Some(loaded) = &self.scene else {
            return String::new();
        };
        full_label(&loaded.scene.chart, idx)
            .map(|l| l.as_str().to_owned())
            .unwrap_or_default()
    }

    fn labels(&self, seats: &[u32]) -> Vec<String> {
        seats.iter().map(|&idx| self.label(idx)).collect()
    }

    pub(super) fn info(&self, idx: u32) -> ObjectInfo {
        let Some(loaded) = &self.scene else {
            return ObjectInfo::default();
        };
        let chart = &loaded.scene.chart;
        let category = chart
            .seat_category
            .get(idx as usize)
            .and_then(|&c| chart.categories.get(usize::from(c)))
            .and_then(|c| chart.strings.get(c.key));
        let Ok(parts) = seat_label(chart, idx) else {
            return ObjectInfo::default();
        };
        ObjectInfo {
            label: parts
                .full(chart)
                .map(|l| l.as_str().to_owned())
                .unwrap_or_default(),
            category: category.unwrap_or_default().to_owned(),
            section: parts.section.to_owned(),
            row: parts.row.to_owned(),
            seat: parts.seat.as_str().to_owned(),
            flags: chart.seat_flags.get(idx as usize).copied().unwrap_or(0),
        }
    }

    /// Recomputes one seat's visual state and marks it for upload.
    pub(super) fn refresh(&mut self, idx: u32) {
        let state = match self
            .selection
            .find(idx)
            .and_then(|i| self.selection.selected.get(i))
        {
            Some(s) if s.held => Selection::Held,
            Some(_) => Selection::Pending,
            None => Selection::None,
        };
        let ours = self.selection.ours(idx);
        let Some(loaded) = &mut self.scene else {
            return;
        };
        let i = idx as usize;
        let section = loaded.scene.section_of(i);
        let (Some(&status), Some(visual)) =
            (loaded.scene.status.get(i), loaded.scene.visual.get_mut(i))
        else {
            return;
        };
        let was = *visual;
        // A seat the buyer may still hold reads as theirs to take again.
        *visual = picker_visual(if ours { 0 } else { status }, state);
        loaded.sections.change(section, was, *visual);
        grow_dirty(&mut loaded.dirty, i..i + 1);
        self.redraw();
    }

    /// A tap or click on seat `idx`.
    pub(super) fn tap(&mut self, idx: u32) {
        if self.selection.find(idx).is_some() {
            self.deselect_seat(idx);
            return;
        }
        let rule = if !self.available(idx) {
            "not_free"
        } else if let Err(v) = self.selection.rules.check_add(self.selection.count(), 1) {
            v.code()
        } else {
            return self.select_seat(idx);
        };
        let objects = vec![self.label(idx)];
        let rule = rule.to_owned();
        self.events.push(Event::SelectionInvalid { rule, objects });
    }

    /// Whether the buyer may select `idx`: free, or still held by them.
    fn available(&self, idx: u32) -> bool {
        let status = self
            .scene
            .as_ref()
            .and_then(|l| l.scene.status.get(idx as usize));
        status == Some(&crate::theme::status::AVAILABLE) || self.selection.ours(idx)
    }

    fn select_seat(&mut self, idx: u32) {
        let sel = &mut self.selection;
        let held = if remove(&mut sel.to_release, idx) {
            true
        } else if let Some(f) = sel.in_flight.as_mut().filter(|f| has(&f.dropped, idx)) {
            remove(&mut f.dropped, idx);
            false
        } else if !remove(&mut sel.released, idx) && has(&sel.private, idx)
            || sel.mode == SessionMode::Manual
        {
            true
        } else {
            sel.to_hold.push(idx);
            false
        };
        sel.selected.push(Selected { idx, held });
        sel.changed = true;
        self.refresh(idx);
    }

    fn deselect_seat(&mut self, idx: u32) {
        let sel = &mut self.selection;
        let Some(at) = sel.find(idx) else {
            return;
        };
        let seat = sel.selected.remove(at);
        if sel.mode == SessionMode::Hold {
            if seat.held {
                sel.to_release.push(idx);
            } else if !remove(&mut sel.to_hold, idx)
                && let Some(f) = &mut sel.in_flight
            {
                f.dropped.push(idx);
            }
        }
        sel.changed = true;
        self.refresh(idx);
    }

    /// The `idx` of a seat's full label: for hosts that hold state by label
    /// (05 §4), converted once per snapshot or delta, never per frame.
    pub fn idx_of(&mut self, label: &str) -> Option<u32> {
        let loaded = self.scene.as_mut()?;
        let chart = &loaded.scene.chart;
        (loaded
            .labels
            .get_or_insert_with(|| LabelIndex::build(chart)))
        .find(chart, label)
    }

    /// A seat's details, public status code (0 available, 1 unavailable) and
    /// position in its row (its place from 1, and the row's seats), for the
    /// SDK's `findObject` (05 §6) and the proxy's name (11 §5).
    pub fn object(&mut self, label: &str) -> Option<(ObjectInfo, u8, [u32; 2])> {
        let idx = self.idx_of(label)?;
        let scene = &self.scene.as_ref()?.scene;
        let status = *scene.status.get(idx as usize)?;
        let row = scene
            .chart
            .rows
            .get(*scene.row.get(idx as usize)? as usize)?;
        let place = [idx - row.seat_first + 1, u32::from(row.seat_count)];
        Some((self.info(idx), status, place))
    }

    /// The chart's bounds on the floor shown, which the first frame fits.
    pub fn chart_bounds(&self) -> Option<Bounds> {
        Some(self.scene.as_ref()?.scene.bounds)
    }

    /// Per section of the chart, its seats not drawn unavailable (04 §5).
    pub fn sections_open(&self) -> &[u32] {
        self.scene.as_ref().map_or(&[], |l| &l.sections.open)
    }

    /// Resolves labels and indices to seats. Unknown objects are reported in
    /// one `Error` and the result is `None`.
    fn resolve(&mut self, objects: &[ObjectRef]) -> Option<Vec<u32>> {
        let Some(loaded) = &mut self.scene else {
            self.error(ErrorCode::InvalidInput, "no chart is loaded");
            return None;
        };
        let chart = &loaded.scene.chart;
        let mut seats = Vec::with_capacity(objects.len());
        let (mut unknown, mut first) = (0usize, None);
        for object in objects {
            let idx = match object {
                ObjectRef::Idx(idx) => Some(*idx).filter(|&i| i < chart.meta.seat_count),
                ObjectRef::Label(label) => loaded
                    .labels
                    .get_or_insert_with(|| LabelIndex::build(chart))
                    .find(chart, label),
            };
            match idx {
                Some(idx) => seats.push(idx),
                None => {
                    unknown += 1;
                    first.get_or_insert(object);
                }
            }
        }
        if let Some(first) = first {
            let name = match first {
                ObjectRef::Idx(idx) => idx.to_string(),
                ObjectRef::Label(label) => format!("\"{}\"", excerpt(label)),
            };
            let message = format!("{unknown} unknown objects, the first {name}");
            self.error(ErrorCode::InvalidInput, message);
            return None;
        }
        Some(seats)
    }

    /// `Select`: as taps, all or nothing. Unknown objects are an error;
    /// unavailable ones, or too many, are refused with `SelectionInvalid`
    /// (too many: the distinct seats up to the first one over the maximum).
    pub(super) fn select_objects(&mut self, objects: &[ObjectRef]) {
        let Some(requested) = self.resolve(objects) else {
            return;
        };
        // Distinct seats not yet selected, in the order given. The maximum
        // bounds them, so the scan is short whatever the host sends.
        let count = self.selection.count();
        let mut seats: Vec<u32> = Vec::new();
        let mut refused = None;
        for idx in requested {
            if has(&seats, idx) || self.selection.find(idx).is_some() {
                continue;
            }
            seats.push(idx);
            let adding = u32::try_from(seats.len()).unwrap_or(u32::MAX);
            if let Err(v) = self.selection.rules.check_add(count, adding) {
                refused = Some(v.code());
                break;
            }
        }
        let unavailable: Vec<u32> = seats
            .iter()
            .copied()
            .filter(|&idx| !self.available(idx))
            .collect();
        let refused = if !unavailable.is_empty() {
            Some(("not_free", unavailable))
        } else {
            refused.map(|rule| (rule, seats.clone()))
        };
        if let Some((rule, objects)) = refused {
            let objects = self.labels(&objects);
            self.events.push(Event::SelectionInvalid {
                rule: rule.to_owned(),
                objects,
            });
            return;
        }
        for idx in seats {
            self.select_seat(idx);
        }
        self.flush_selection();
    }

    /// `Deselect`; objects not selected are ignored.
    pub(super) fn deselect_objects(&mut self, objects: &[ObjectRef]) {
        if let Some(seats) = self.resolve(objects) {
            seats.into_iter().for_each(|idx| self.deselect_seat(idx));
            self.flush_selection();
        }
    }

    pub(super) fn clear_selection(&mut self) {
        let seats: Vec<u32> = self.selection.selected.iter().map(|s| s.idx).collect();
        seats.into_iter().for_each(|idx| self.deselect_seat(idx));
        self.flush_selection();
    }

    pub(super) fn hold_result(&mut self, request: u32, outcome: &HoldOutcome) {
        let flight = self.selection.in_flight.take_if(|f| f.request == request);
        let Some(flight) = flight else {
            return self.error(ErrorCode::InvalidInput, "unknown hold request");
        };
        let failures = match outcome {
            HoldOutcome::Ok => {
                for &idx in &flight.seats {
                    let sel = &mut self.selection;
                    if has(&flight.dropped, idx) {
                        sel.to_release.push(idx);
                    } else if let Some(at) = sel.find(idx)
                        && let Some(seat) = sel.selected.get_mut(at)
                    {
                        seat.held = true;
                        self.refresh(idx);
                    }
                }
                return self.flush_selection();
            }
            HoldOutcome::Failed(failures) => failures,
        };
        // Deselected in flight and not held: no longer the buyer's to draw.
        for &idx in &flight.dropped {
            self.refresh(idx);
        }
        if failures.iter().any(|f| f.code == TOKEN_GONE) {
            return self.token_gone();
        }
        // Holds are all or nothing (02 §4.3): no seat of the request is held.
        // The seats the server names are refused; the others are requested
        // again. With none named, all are refused.
        let labels = self.labels(&flight.seats);
        let mut refused: Vec<(u32, &str)> = Vec::new();
        for failure in failures {
            let named =
                flight
                    .seats
                    .iter()
                    .zip(&labels)
                    .find(|&(&idx, label)| match &failure.object {
                        ObjectRef::Idx(i) => *i == idx,
                        ObjectRef::Label(l) => l == label,
                    });
            if let Some((&idx, _)) = named {
                refused.push((idx, failure.code.as_str()));
            }
        }
        if refused.is_empty() {
            let code = failures.first().map_or("hold_failed", |f| f.code.as_str());
            refused = flight.seats.iter().map(|&idx| (idx, code)).collect();
        }
        let mut invalid: Vec<(&str, Vec<String>)> = Vec::new();
        for (&idx, label) in flight.seats.iter().zip(labels) {
            if has(&flight.dropped, idx) {
                continue;
            }
            let Some(&(_, code)) = refused.iter().find(|(i, _)| *i == idx) else {
                // Not refused: requested again, unless the private set has
                // shown it held meanwhile.
                let sel = &mut self.selection;
                if sel
                    .find(idx)
                    .and_then(|at| sel.selected.get(at))
                    .is_some_and(|s| !s.held)
                {
                    sel.to_hold.push(idx);
                }
                continue;
            };
            if let Some(at) = self.selection.find(idx) {
                self.selection.selected.remove(at);
                self.selection.changed = true;
                self.refresh(idx);
            }
            if NOT_FREE.contains(&code) {
                self.events.push(Event::ObjectUnavailable { label });
            } else if let Some((_, objects)) = invalid.iter_mut().find(|(c, _)| *c == code) {
                objects.push(label);
            } else {
                invalid.push((code, vec![label]));
            }
        }
        for (rule, objects) in invalid {
            let rule = rule.to_owned();
            self.events.push(Event::SelectionInvalid { rule, objects });
        }
        self.flush_selection();
    }

    pub(super) fn set_rules(
        &mut self,
        max: Option<u32>,
        min: Option<u32>,
        validators: &[String],
        group_size: Option<u32>,
    ) {
        if !validators.is_empty() || group_size.is_some_and(|n| n != 1) {
            return self.error(
                ErrorCode::Unsupported,
                "validators and group selection are not implemented in this build",
            );
        }
        let max = max.unwrap_or(MAX_SELECTED);
        let rules = match Rules::new(Some(max), min.unwrap_or(0)) {
            _ if max > MAX_SELECTED => {
                return self.error(ErrorCode::InvalidInput, "maximum selection above 100");
            }
            _ if self.selection.count() > max => {
                return self.error(
                    ErrorCode::InvalidInput,
                    "maximum selection below the current selection",
                );
            }
            Ok(rules) => rules,
            Err(e) => return self.error(ErrorCode::InvalidInput, e.to_string()),
        };
        self.selection.rules = rules;
        self.flush_selection();
    }

    pub(super) fn set_session_mode(&mut self, mode: SessionMode) {
        let sel = &self.selection;
        if sel.selected.is_empty() && sel.in_flight.is_none() && sel.to_release.is_empty() {
            self.selection.mode = mode;
        } else {
            self.error(
                ErrorCode::InvalidInput,
                "the session mode changes only with nothing selected or outstanding",
            );
        }
    }

    /// The hold token is gone (expired or unknown), and every hold with it:
    /// the whole selection leaves with `HoldLost`, and nothing is released.
    fn token_gone(&mut self) {
        let seats = self.selection.seats();
        let sel = &mut self.selection;
        let lost: Vec<u32> = sel.selected.iter().map(|s| s.idx).collect();
        *sel = SelectState {
            mode: sel.mode,
            rules: sel.rules,
            next_request: sel.next_request,
            valid: sel.valid,
            changed: !lost.is_empty() || sel.expiry.is_some(),
            ..SelectState::default()
        };
        seats.into_iter().for_each(|idx| self.refresh(idx));
        if !lost.is_empty() {
            let labels = self.labels(&lost);
            self.events.push(Event::HoldLost { labels });
        }
        self.flush_selection();
    }

    /// After a status change: selected seats now unavailable whose status is
    /// not the buyer's own hold leave the selection with `ObjectUnavailable`.
    /// `changes` limits the check to the seats a delta named.
    pub(super) fn knock_out(&mut self, changes: Option<&[StatusChange]>) {
        let changed = |idx| changes.is_none_or(|c| c.binary_search_by_key(&idx, |c| c.idx).is_ok());
        let Some(loaded) = &self.scene else {
            return;
        };
        let status = &loaded.scene.status;
        let sel = &self.selection;
        let out: Vec<u32> = (sel.selected.iter())
            .filter(|s| changed(s.idx) && status.get(s.idx as usize) != Some(&0) && !sel.own(s))
            .map(|s| s.idx)
            .collect();
        for idx in out {
            let sel = &mut self.selection;
            remove(&mut sel.to_hold, idx);
            if let Some(at) = sel.find(idx) {
                sel.selected.remove(at);
            }
            sel.changed = true;
            let label = self.label(idx);
            self.events.push(Event::ObjectUnavailable { label });
            self.refresh(idx);
        }
        self.flush_selection();
    }

    /// A new private set (04 §17), within the seats.
    pub(super) fn set_private(&mut self, objects: Vec<u32>, expires_at_ms: u64) {
        let sel = &mut self.selection;
        let old = std::mem::replace(&mut sel.private, objects);
        let expiry = (!sel.private.is_empty()).then_some(expires_at_ms);
        sel.changed |= expiry != sel.expiry;
        sel.expiry = expiry;
        let mut lost = Vec::new();
        for &idx in &old {
            // The buyer's own releases are not news; a set without the seat
            // confirms one.
            if sel.given_up(idx) || has(&sel.private, idx) {
                continue;
            }
            if let Some(at) = sel.find(idx)
                && sel.selected.get(at).is_some_and(|s| s.held)
            {
                sel.selected.remove(at);
                sel.changed = true;
                lost.push(idx);
            }
        }
        for i in 0..sel.private.len() {
            let idx = sel.private.get(i).copied().unwrap_or_default();
            if sel.given_up(idx) || has(&old, idx) {
                continue;
            }
            if let Some(at) = sel.find(idx) {
                if let Some(seat) = sel.selected.get_mut(at) {
                    seat.held = true;
                }
            } else if sel.count() < MAX_SELECTED {
                sel.selected.push(Selected { idx, held: true });
            } else {
                continue;
            }
            remove(&mut sel.to_hold, idx);
            sel.changed = true;
        }
        let private = std::mem::take(&mut sel.private);
        sel.released.retain(|&idx| has(&private, idx));
        sel.private = private;
        for idx in old.into_iter().chain(self.selection.private.clone()) {
            self.refresh(idx);
        }
        if !lost.is_empty() {
            let labels = self.labels(&lost);
            self.events.push(Event::HoldLost { labels });
        }
        self.flush_selection();
    }

    /// Forgets the selection, hover and focus of the chart being replaced.
    /// No release requests: the holds stay on the token, which the host owns.
    pub(super) fn reset_selection(&mut self) {
        let had = !self.selection.selected.is_empty() || self.selection.expiry.is_some();
        self.selection = SelectState {
            mode: self.selection.mode,
            rules: self.selection.rules,
            next_request: self.selection.next_request,
            valid: self.selection.valid,
            changed: had,
            ..SelectState::default()
        };
        self.hover_at = None;
        self.set_hover(None);
        self.set_focus(None);
    }

    /// Emits what changed since the last call, in this order:
    /// `SelectionChanged`, `SelectionValid` or `SelectionInvalid` for the
    /// minimum, then the next release and hold requests.
    pub(super) fn flush_selection(&mut self) {
        if std::mem::take(&mut self.selection.changed) {
            let selected = (self.selection.selected.iter())
                .map(|s| self.info(s.idx))
                .collect();
            let expires_at_ms = self.selection.expiry;
            self.events.push(Event::SelectionChanged {
                selected,
                expires_at_ms,
            });
        }
        let check = self.selection.rules.check_checkout(self.selection.count());
        if check.is_ok() != self.selection.valid {
            self.selection.valid = check.is_ok();
            self.events.push(match check {
                Ok(()) => Event::SelectionValid,
                Err(v) => {
                    let seats: Vec<u32> = self.selection.selected.iter().map(|s| s.idx).collect();
                    Event::SelectionInvalid {
                        rule: v.code().to_owned(),
                        objects: self.labels(&seats),
                    }
                }
            });
        }
        let sel = &mut self.selection;
        if sel.in_flight.is_some() || sel.mode == SessionMode::Manual {
            return;
        }
        if !sel.to_release.is_empty() {
            let seats = std::mem::take(&mut sel.to_release);
            sel.released.extend(&seats);
            let request = sel.request_id();
            let labels = self.labels(&seats);
            self.events.push(Event::ReleaseRequest { request, labels });
        }
        let sel = &mut self.selection;
        if !sel.to_hold.is_empty() {
            let seats = std::mem::take(&mut sel.to_hold);
            let request = sel.request_id();
            let labels = self.labels(&seats);
            self.selection.in_flight = Some(InFlight {
                request,
                seats,
                dropped: Vec::new(),
            });
            self.events.push(Event::HoldRequest { request, labels });
        }
    }
}
