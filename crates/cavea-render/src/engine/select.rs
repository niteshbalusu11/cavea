//! Taps, hover and focus (04 §15) and the selection (04 §16).
//!
//! The selected seats are kept in the order selected, at most
//! `MAX_SELECTED`. Hold requests are REN-034: until then the engine selects
//! as in a manual session (`session: "manual"`, D-14), where the host holds at
//! checkout: a tap selects at once and deselecting sends nothing. A selected
//! seat whose public status turns unavailable was taken by someone else and
//! leaves the selection with `ObjectUnavailable`.

use cavea_core::rules::Rules;
use cavea_format::full_label::{LabelIndex, full_label, seat_label};

use super::*;
use crate::{
    hit::hit_test,
    protocol::{ObjectInfo, ObjectRef},
    theme::status,
    tuning::{
        HIT_TOLERANCE_MOUSE_PX, HIT_TOLERANCE_TOUCH_PX, MAX_SELECTED, SELECT_MIN_SEAT_MOUSE_PX,
        SELECT_MIN_SEAT_TOUCH_PX, ZOOM_TO_SELECT_SEAT_PX,
    },
};

#[derive(Debug)]
pub(super) struct SelectState {
    selected: Vec<u32>,
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
            rules: Rules::new(Some(MAX_SELECTED), 0).unwrap_or_default(),
            valid: true,
            changed: false,
        }
    }
}

impl SelectState {
    fn count(&self) -> u32 {
        // At most `MAX_SELECTED`.
        self.selected.len() as u32
    }
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

    /// Moves the focus ring (`None` removes it) and emits `ObjectFocus`. The
    /// entry point for keyboard navigation (A11Y-001), which will call it from
    /// `Key`; until then only tests do.
    pub fn set_focus(&mut self, idx: Option<u32>) {
        let seats = self.scene.as_ref().map_or(0, |l| l.scene.pos.len());
        if idx.is_some_and(|idx| idx as usize >= seats) {
            return self.error(ErrorCode::InvalidInput, "focus is not a seat");
        }
        if idx != self.focus {
            self.focus = idx;
            let object = idx.and_then(|idx| self.anchored(idx));
            self.events.push(Event::ObjectFocus(object));
            self.redraw();
        }
    }

    /// An object's details and its centre on screen.
    fn anchored(&self, idx: u32) -> Option<(ObjectInfo, [f32; 2])> {
        let pos = self.scene.as_ref()?.scene.pos.get(idx as usize)?;
        let at = match (self.camera, self.stage()) {
            (Some(camera), Some(stage)) => {
                camera.to_screen([f64::from(pos[0]), f64::from(pos[1])], stage.size)
            }
            _ => [0.0; 2],
        };
        Some((self.info(idx), at.map(|v| v as f32)))
    }

    fn labels(&self, seats: &[u32]) -> Vec<String> {
        let Some(loaded) = &self.scene else {
            return Vec::new();
        };
        let label = |idx| full_label(&loaded.scene.chart, idx).map(|l| l.as_str().to_owned());
        seats
            .iter()
            .map(|&idx| label(idx).unwrap_or_default())
            .collect()
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
    fn refresh(&mut self, idx: u32) {
        let state = if self.selection.selected.contains(&idx) {
            Selection::Held
        } else {
            Selection::None
        };
        let Some(loaded) = &mut self.scene else {
            return;
        };
        let i = idx as usize;
        let (Some(&status), Some(visual)) =
            (loaded.scene.status.get(i), loaded.scene.visual.get_mut(i))
        else {
            return;
        };
        *visual = picker_visual(status, state);
        grow_dirty(&mut loaded.dirty, i..i + 1);
        self.redraw();
    }

    /// Whether seat `idx` is free to select.
    fn available(&self, idx: u32) -> bool {
        let scene = self.scene.as_ref().map(|l| &l.scene);
        scene.and_then(|s| s.status.get(idx as usize)) == Some(&status::AVAILABLE)
    }

    /// A tap or click on seat `idx`: deselects it, selects it, or refuses it
    /// with `SelectionInvalid`.
    fn tap(&mut self, idx: u32) {
        if self.selection.selected.contains(&idx) {
            return self.deselect_seat(idx);
        }
        let rule = if !self.available(idx) {
            "not_free"
        } else if let Err(v) = self.selection.rules.check_add(self.selection.count(), 1) {
            v.code()
        } else {
            return self.select_seat(idx);
        };
        let objects = self.labels(&[idx]);
        let rule = rule.to_owned();
        self.events.push(Event::SelectionInvalid { rule, objects });
    }

    fn select_seat(&mut self, idx: u32) {
        self.selection.selected.push(idx);
        self.selection.changed = true;
        self.refresh(idx);
    }

    fn deselect_seat(&mut self, idx: u32) {
        let sel = &mut self.selection;
        if let Some(at) = sel.selected.iter().position(|&i| i == idx) {
            sel.selected.remove(at);
            sel.changed = true;
            self.refresh(idx);
        }
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
            if seats.contains(&idx) || self.selection.selected.contains(&idx) {
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
        let seats = std::mem::take(&mut self.selection.selected);
        self.selection.changed |= !seats.is_empty();
        seats.into_iter().for_each(|idx| self.refresh(idx));
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

    /// After a status change in `range`: selected seats that became
    /// unavailable leave the selection with `ObjectUnavailable`.
    pub(super) fn status_changed(&mut self, range: Range<u32>) {
        let taken: Vec<u32> = (self.selection.selected.iter().copied())
            .filter(|idx| range.contains(idx) && !self.available(*idx))
            .collect();
        let labels = self.labels(&taken);
        self.selection.selected.retain(|idx| !taken.contains(idx));
        self.selection.changed |= !taken.is_empty();
        for label in labels {
            self.events.push(Event::ObjectUnavailable { label });
        }
        // The fill reset the range; selected seats keep their state.
        let kept: Vec<u32> = (self.selection.selected.iter().copied())
            .filter(|idx| range.contains(idx))
            .collect();
        kept.into_iter().for_each(|idx| self.refresh(idx));
        self.flush_selection();
    }

    /// Forgets the selection, hover and focus of the chart being replaced.
    pub(super) fn reset_selection(&mut self) {
        let sel = &mut self.selection;
        sel.changed |= !sel.selected.is_empty();
        sel.selected.clear();
        self.hover_at = None;
        self.set_hover(None);
        self.set_focus(None);
    }

    /// Emits what changed since the last call: `SelectionChanged`, then
    /// `SelectionValid` or `SelectionInvalid` when readiness for checkout
    /// changes.
    pub(super) fn flush_selection(&mut self) {
        if std::mem::take(&mut self.selection.changed) {
            let selected = (self.selection.selected.iter())
                .map(|&idx| self.info(idx))
                .collect();
            self.events.push(Event::SelectionChanged { selected });
        }
        let check = self.selection.rules.check_checkout(self.selection.count());
        if check.is_ok() != self.selection.valid {
            self.selection.valid = check.is_ok();
            self.events.push(match check {
                Ok(()) => Event::SelectionValid,
                Err(v) => Event::SelectionInvalid {
                    rule: v.code().to_owned(),
                    objects: self.labels(&self.selection.selected),
                },
            });
        }
    }
}
