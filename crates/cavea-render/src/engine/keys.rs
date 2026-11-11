//! The keyboard baseline (`docs/11-accessibility.md` §3, A11Y-001): a virtual
//! focus moved by the arrow keys (`crate::focus`), Enter or Space to select
//! it as a tap would, Escape to hide it, plus, minus and 0 to zoom.
//!
//! The engine keeps the focused seat while the ring is hidden, so tabbing
//! away and back, or a tap (which moves the focus to the tapped seat without
//! showing it), resumes from it. `ObjectFocus` reports the focus while it is
//! shown: when it moves or appears, `None` when it is hidden, and again when
//! the focused seat's state changes or the camera settles after moving it on
//! screen, with its centre where it is then.

use super::*;
use crate::{
    focus::{self, Dir},
    protocol::Modifiers,
    tuning::{FOCUS_MARGIN_PX, LOD_DETAIL_SEAT_PX, LOD_SEATS_SEAT_PX, ZOOM_STEP},
};

impl<B: GpuBackend> Engine<B> {
    /// Whether `Key` takes `key` (a DOM `KeyboardEvent.key`) now: if so the
    /// host prevents its default, otherwise it belongs to the page. Keys
    /// with Ctrl, Alt or Meta are the page's; Escape is the map's only while
    /// the focus is shown.
    pub fn takes_key(&self, key: &str, m: Modifiers) -> bool {
        if m.ctrl || m.alt || m.meta {
            return false;
        }
        match key {
            "Escape" => self.focus_shown,
            _ => KEYS.contains(&key),
        }
    }

    pub(super) fn key(&mut self, key: &str, m: Modifiers) {
        if !self.takes_key(key, m) {
            return;
        }
        let dir = match key {
            "ArrowLeft" => Dir::Left,
            "ArrowRight" => Dir::Right,
            "ArrowUp" => Dir::Up,
            "ArrowDown" => Dir::Down,
            "Enter" | " " => {
                if let Some(idx) = self.focus.filter(|_| self.focus_shown) {
                    // As a tap, but never zoom-to-select (11 §3).
                    self.tap(idx);
                }
                return;
            }
            "Escape" => return self.show_focus(false),
            "0" => {
                if let Some(bounds) = self.scene.as_ref().map(|l| l.scene.bounds) {
                    self.enqueue_camera(bounds, true, None);
                }
                return;
            }
            _ => {
                let factor = if key == "-" { 1.0 / ZOOM_STEP } else { ZOOM_STEP };
                return self.zoom_step(factor);
            }
        };
        let Some(loaded) = &self.scene else {
            return;
        };
        let scene = &loaded.scene;
        let target = self.camera_target();
        let next = match self.focus.filter(|i| scene.active.contains(i)) {
            // At an edge the focus stays, and is shown if it was not.
            Some(from) => focus::step(scene, from, dir).unwrap_or(from),
            None => {
                let Some(centre) = target.map(|c| c.centre.map(|v| v as f32)) else {
                    return;
                };
                let Some(idx) = focus::nearest(scene, centre) else {
                    return;
                };
                idx
            }
        };
        if self.focus_shown && self.focus == Some(next) {
            return;
        }
        self.focus = Some(next);
        self.focus_shown = true;
        self.follow(next);
        self.emit_focus();
        self.redraw();
    }

    /// Shows or hides the ring at the remembered focus: the host's focus
    /// entering (by keyboard) or leaving the map.
    pub(super) fn show_focus(&mut self, shown: bool) {
        let shown = shown && self.focus.is_some();
        if shown != self.focus_shown {
            self.focus_shown = shown;
            self.emit_focus();
            self.redraw();
        }
    }

    /// The camera a running fly-to ends at, else the camera.
    fn camera_target(&self) -> Option<Camera> {
        let flying = self.animations.iter().find_map(|a| match a {
            Animation::Fly { to, .. } => Some(*to),
            Animation::Inertia { .. } => None,
        });
        flying.or(self.camera)
    }

    /// Brings `idx` into view with the smallest movement: below the seats
    /// level (seats not drawn) the camera zooms to the detail level centred
    /// on it; otherwise it pans just enough to keep its centre
    /// `FOCUS_MARGIN_PX` inside the view (at most a quarter of the view).
    fn follow(&mut self, idx: u32) {
        let (Some(stage), Some(camera), Some(loaded)) =
            (self.stage(), self.camera_target(), &self.scene)
        else {
            return;
        };
        let Some(&[x, y]) = loaded.scene.pos.get(idx as usize) else {
            return;
        };
        let pos = [f64::from(x), f64::from(y)];
        let seat = f64::from(loaded.scene.seat_size);
        let mut to = camera;
        if seat * camera.zoom < LOD_SEATS_SEAT_PX {
            to.zoom = LOD_DETAIL_SEAT_PX / seat;
            to.centre = pos;
        } else {
            let at = camera.to_screen(pos, stage.size);
            let room = [stage.size.width, stage.size.height];
            for axis in 0..2 {
                let (v, side) = (at[axis], room[axis]);
                let m = FOCUS_MARGIN_PX.min(side / 4.0);
                let shift = (v - m).min(0.0) + (v - (side - m)).max(0.0);
                to.centre[axis] += shift / camera.zoom;
            }
        }
        to.clamp(stage.chart, stage.size, stage.limits);
        if Some(to) != self.camera_target() {
            let duration = camera::fly_duration_ms(&camera, &to, stage.size);
            self.move_to(to, Some(duration));
        }
    }

    /// One zoom step about the focus shown, else the view's centre.
    fn zoom_step(&mut self, factor: f64) {
        let (Some(stage), Some(camera)) = (self.stage(), self.camera) else {
            return;
        };
        let pos = self
            .focus
            .filter(|_| self.focus_shown)
            .and_then(|i| self.scene.as_ref()?.scene.pos.get(i as usize).copied());
        let at = pos.map_or([stage.size.width / 2.0, stage.size.height / 2.0], |[x, y]| {
            camera.to_screen([f64::from(x), f64::from(y)], stage.size)
        });
        self.act(Action::ZoomStep { factor, at });
    }

    /// Emits `ObjectFocus` for the focus shown (`None` if hidden), anchored
    /// where the camera will settle, and remembers what it reported.
    pub(super) fn emit_focus(&mut self) {
        let shown = self.focus.filter(|_| self.focus_shown);
        let camera = self.camera_target();
        let object = shown.and_then(|idx| self.anchored_at(idx, camera));
        self.focus_seen = shown.map(|idx| (camera, self.visual_of(idx)));
        self.events.push(Event::ObjectFocus(object));
    }

    /// At the end of a frame: reports the focus again if its seat's state
    /// changed, or the camera came to rest elsewhere than it was reported.
    pub(super) fn refocus(&mut self) {
        let Some(idx) = self.focus.filter(|_| self.focus_shown) else {
            return;
        };
        if !self.animations.is_empty() || self.recogniser.pressed() {
            return;
        }
        if self.focus_seen != Some((self.camera, self.visual_of(idx))) {
            self.emit_focus();
        }
    }

    fn visual_of(&self, idx: u32) -> u8 {
        let visual = self.scene.as_ref().map(|l| &l.scene.visual);
        visual.and_then(|v| v.get(idx as usize)).copied().unwrap_or(0)
    }
}

/// The keys `Key` takes besides Escape (11 §3, the baseline).
const KEYS: [&str; 10] = [
    "ArrowLeft",
    "ArrowRight",
    "ArrowUp",
    "ArrowDown",
    "Enter",
    " ",
    "+",
    "=",
    "-",
    "0",
];
