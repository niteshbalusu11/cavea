//! The gesture recogniser (04 §14). It turns `Pointer` and `Wheel` input into
//! [`Action`]s for the engine: camera moves, and [`Intent`]s for hit testing and
//! selection. It reads no clock; every decision uses the input's timestamps or
//! the `Tick` time it is given, so a recorded trace replays identically.

use std::f64::consts::LN_2;

use crate::{
    camera::length,
    protocol::{GestureHandling, PointerKind, PointerType},
    tuning::{
        DOUBLE_TAP_DISTANCE_PX, DOUBLE_TAP_MS, DRAG_SLOP_MOUSE_PX, DRAG_SLOP_TOUCH_PX,
        LONG_PRESS_MS, MAX_POINTERS, PINCH_WHEEL_PX_PER_DOUBLING, TWO_FINGER_TAP_MS,
        VELOCITY_WINDOW_MS, WHEEL_MAC_NOTCH_PX, WHEEL_NOTCH_MIN_PX, WHEEL_STREAM_GAP_MS,
        WHEEL_ZOOM_PX_PER_DOUBLING, ZOOM_STEP,
    },
};

/// What a gesture asks of the engine, in the order recognised. Positions are
/// CSS pixels on the canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    /// A gesture began: stop camera animations.
    Interrupt,
    /// Move the chart by this many pixels.
    Pan([f64; 2]),
    /// Zoom by `factor` about the point `at`, now.
    Zoom {
        factor: f64,
        at: [f64; 2],
    },
    /// Zoom by `factor` about `at`, animated (double tap, two-finger tap).
    ZoomStep {
        factor: f64,
        at: [f64; 2],
    },
    /// A drag was released at `velocity` pixels per millisecond.
    Fling {
        velocity: [f64; 2],
        time_ms: f64,
    },
    Intent(Intent),
}

/// Recognised input that hit testing and selection consume (04 §15, §16).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Intent {
    /// A tap or click that was not part of a drag, pinch or double tap.
    Tap { at: [f64; 2], pointer: PointerType },
    /// A touch or pen press held still for `LONG_PRESS_MS`: show the tooltip.
    LongPress { at: [f64; 2] },
    /// A mouse or pen over the canvas with nothing pressed; `None` once it leaves.
    Hover(Option<[f64; 2]>),
}

impl GestureHandling {
    /// Whether the map takes a wheel event. When it does not, the host leaves
    /// the event to the page.
    pub fn takes_wheel(self, ctrl: bool) -> bool {
        match self {
            Self::Greedy => true,
            Self::Cooperative => ctrl,
            Self::None => false,
        }
    }

    /// The CSS `touch-action` for the canvas: the browser handles exactly the
    /// touch gestures the map leaves to the page.
    pub fn touch_action(self) -> &'static str {
        match self {
            Self::Greedy => "none",
            Self::Cooperative => "pan-x pan-y",
            Self::None => "auto",
        }
    }

    /// Whether a one-pointer drag of this type pans the map.
    fn drags(self, pointer: PointerType) -> bool {
        match self {
            Self::Greedy => true,
            Self::Cooperative => pointer == PointerType::Mouse,
            Self::None => false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Tracked {
    id: u32,
    kind: PointerType,
    start: [f64; 2],
    pos: [f64; 2],
}

/// Drag positions kept for the release velocity.
const SAMPLES: usize = 8;

#[derive(Debug, Default)]
pub struct Recogniser {
    pub mode: GestureHandling,
    pointers: [Option<Tracked>; MAX_POINTERS],
    /// When the current gesture's first pointer went down.
    start_ms: f64,
    /// Most pointers down at once in the current gesture.
    most: usize,
    /// No pointer has passed the drag threshold, and no third pointer or
    /// cancel has made the gesture something other than a tap.
    still: bool,
    long_pressed: bool,
    /// A cancel ended the gesture; remaining pointers are ignored until all lift.
    cancelled: bool,
    /// One pointer is dragging.
    dragging: bool,
    /// Where the second pointer joined the first: the two-finger tap anchor.
    pair_mid: [f64; 2],
    /// Recent drag positions as (time, position), a ring of `SAMPLES`.
    samples: [(f64, [f64; 2]); SAMPLES],
    sampled: usize,
    /// The last tap's release time, position and pointer type, for double taps.
    last_tap: Option<(f64, [f64; 2], PointerType)>,
    /// The current wheel stream: last event time and whether it is a mouse wheel.
    wheel: Option<(f64, bool)>,
}

impl Recogniser {
    pub fn pointer(
        &mut self,
        kind: PointerKind,
        id: u32,
        pointer: PointerType,
        at: [f64; 2],
        time_ms: f64,
        out: &mut Vec<Action>,
    ) {
        match kind {
            PointerKind::Down => self.down(id, pointer, at, time_ms, out),
            PointerKind::Move => {
                self.moved(id, pointer, at, time_ms, out);
            }
            PointerKind::Up => {
                if self.moved(id, pointer, at, time_ms, out) {
                    self.up(id, time_ms, out);
                }
            }
            PointerKind::Cancel => self.cancel(id, out),
        }
    }

    /// A wheel event the gesture handling mode lets the map take.
    pub fn wheel(
        &mut self,
        delta: [f64; 2],
        at: [f64; 2],
        ctrl: bool,
        time_ms: f64,
        out: &mut Vec<Action>,
    ) {
        let mouse = match self.wheel {
            Some((last, mouse)) if time_ms - last <= WHEEL_STREAM_GAP_MS => {
                mouse && delta[0] == 0.0
            }
            _ => {
                delta[0] == 0.0
                    && (delta[1].abs() >= WHEEL_NOTCH_MIN_PX
                        || is_whole_nonzero(delta[1] / WHEEL_MAC_NOTCH_PX))
            }
        };
        self.wheel = Some((time_ms, mouse));
        out.push(Action::Interrupt);
        if ctrl || mouse {
            let per_doubling = if mouse {
                WHEEL_ZOOM_PX_PER_DOUBLING
            } else {
                PINCH_WHEEL_PX_PER_DOUBLING
            };
            let factor = (-delta[1] * LN_2 / per_doubling).exp();
            out.push(Action::Zoom { factor, at });
        } else {
            out.push(Action::Pan([-delta[0], -delta[1]]));
        }
    }

    /// Recognises a long press once its time has come.
    pub fn tick(&mut self, now_ms: f64, out: &mut Vec<Action>) {
        if let Some(p) = self.pending_long_press()
            && now_ms - self.start_ms >= LONG_PRESS_MS
        {
            self.long_pressed = true;
            out.push(Action::Intent(Intent::LongPress { at: p.pos }));
        }
    }

    /// Whether a long press may be recognised on a later `Tick`.
    pub fn wants_tick(&self) -> bool {
        self.pending_long_press().is_some()
    }

    fn pending_long_press(&self) -> Option<Tracked> {
        let mut down = self.pointers.iter().flatten();
        match (down.next(), down.next()) {
            (Some(p), None)
                if self.still
                    && self.most == 1
                    && !self.long_pressed
                    && p.kind != PointerType::Mouse =>
            {
                Some(*p)
            }
            _ => None,
        }
    }

    fn count(&self) -> usize {
        self.pointers.iter().flatten().count()
    }

    fn index(&self, id: u32) -> Option<usize> {
        self.pointers
            .iter()
            .position(|p| p.is_some_and(|p| p.id == id))
    }

    fn down(
        &mut self,
        id: u32,
        pointer: PointerType,
        at: [f64; 2],
        time_ms: f64,
        out: &mut Vec<Action>,
    ) {
        // A press from a pointer already down means its release was lost; a
        // mouse has one pointer.
        for slot in &mut self.pointers {
            if slot.is_some_and(|p| {
                p.id == id || (pointer == PointerType::Mouse && p.kind == PointerType::Mouse)
            }) {
                *slot = None;
            }
        }
        let count = self.count();
        if count == 0 {
            self.start_ms = time_ms;
            self.most = 0;
            self.still = true;
            self.long_pressed = false;
            self.cancelled = false;
            if self.mode != GestureHandling::None {
                out.push(Action::Interrupt);
            }
        }
        // A pinch measures from where both fingers were when the second joined.
        let first = self.pointers.iter_mut().flatten().next().map(|p| {
            p.start = p.pos;
            p.pos
        });
        let Some(slot) = self.pointers.iter_mut().find(|p| p.is_none()) else {
            return;
        };
        *slot = Some(Tracked {
            id,
            kind: pointer,
            start: at,
            pos: at,
        });
        self.most = self.most.max(count + 1);
        self.dragging = false;
        if let (1, Some(first)) = (count, first) {
            self.pair_mid = mid(first, at);
        } else if count >= 2 {
            self.still = false;
        }
    }

    /// Applies a move; returns whether the pointer is tracked.
    fn moved(
        &mut self,
        id: u32,
        pointer: PointerType,
        at: [f64; 2],
        time_ms: f64,
        out: &mut Vec<Action>,
    ) -> bool {
        let Some(i) = self.index(id) else {
            if pointer != PointerType::Touch && self.count() == 0 {
                out.push(Action::Intent(Intent::Hover(Some(at))));
            }
            return false;
        };
        let Some(Some(p)) = self.pointers.get_mut(i) else {
            return false;
        };
        let (prev, start, kind) = (p.pos, p.start, p.kind);
        p.pos = at;
        if self.cancelled {
            return true;
        }
        let slop = if kind == PointerType::Mouse {
            DRAG_SLOP_MOUSE_PX
        } else {
            DRAG_SLOP_TOUCH_PX
        };
        let was_still = self.still;
        if self.still && dist(start, at) > slop {
            self.still = false;
        }
        if self.still {
            return true;
        }
        match self.count() {
            1 => {
                if !self.dragging {
                    self.dragging = true;
                    self.sampled = 0;
                    if was_still {
                        // A flick may cross the threshold in one move.
                        self.sample(self.start_ms, start);
                    }
                }
                self.sample(time_ms, at);
                // The drag grabs the chart where the press began.
                let from = if was_still { start } else { prev };
                if self.mode.drags(kind) {
                    out.push(Action::Pan([at[0] - from[0], at[1] - from[1]]));
                }
            }
            2 if self.mode != GestureHandling::None => {
                let mut pair = self.pointers.iter().flatten();
                let (Some(a), Some(b)) = (pair.next(), pair.next()) else {
                    return true;
                };
                let other = match (a.id == id, b.id == id) {
                    (true, _) => *b,
                    (_, true) => *a,
                    _ => return true,
                };
                // The first pinching move also applies the movement made
                // under the threshold.
                let (from, other_from) = if was_still {
                    (start, other.start)
                } else {
                    (prev, other.pos)
                };
                let (m0, d0) = (mid(from, other_from), dist(from, other_from));
                let (m1, d1) = (mid(at, other.pos), dist(at, other.pos));
                if d0 > 0.0 {
                    out.push(Action::Zoom {
                        factor: d1 / d0,
                        at: m0,
                    });
                }
                out.push(Action::Pan([m1[0] - m0[0], m1[1] - m0[1]]));
            }
            _ => {}
        }
        true
    }

    fn up(&mut self, id: u32, time_ms: f64, out: &mut Vec<Action>) {
        let Some(p) = self
            .index(id)
            .and_then(|i| self.pointers.get_mut(i)?.take())
        else {
            return;
        };
        if self.count() > 0 || self.cancelled {
            return;
        }
        let last_tap = self.last_tap.take();
        let held = time_ms - self.start_ms;
        if self.still && self.most == 1 {
            if !self.long_pressed && p.kind != PointerType::Mouse && held >= LONG_PRESS_MS {
                self.long_pressed = true;
                out.push(Action::Intent(Intent::LongPress { at: p.pos }));
            }
            if self.long_pressed {
                return;
            }
            let double = last_tap.is_some_and(|(up_ms, at, kind)| {
                kind == p.kind
                    && self.start_ms - up_ms <= DOUBLE_TAP_MS
                    && dist(at, p.pos) <= DOUBLE_TAP_DISTANCE_PX
            });
            if double && self.mode != GestureHandling::None {
                out.push(Action::ZoomStep {
                    factor: ZOOM_STEP,
                    at: p.pos,
                });
            } else {
                out.push(Action::Intent(Intent::Tap {
                    at: p.pos,
                    pointer: p.kind,
                }));
                self.last_tap = Some((time_ms, p.pos, p.kind));
            }
        } else if self.still && self.most == 2 {
            if held <= TWO_FINGER_TAP_MS && self.mode != GestureHandling::None {
                out.push(Action::ZoomStep {
                    factor: 1.0 / ZOOM_STEP,
                    at: self.pair_mid,
                });
            }
        } else if self.dragging
            && self.mode.drags(p.kind)
            && let Some(velocity) = self.velocity()
        {
            out.push(Action::Fling { velocity, time_ms });
        }
    }

    fn cancel(&mut self, id: u32, out: &mut Vec<Action>) {
        let Some(slot) = self.index(id).and_then(|i| self.pointers.get_mut(i)) else {
            // A pointer that is not down left the canvas.
            out.push(Action::Intent(Intent::Hover(None)));
            return;
        };
        *slot = None;
        self.cancelled = true;
        self.still = false;
        self.dragging = false;
        self.last_tap = None;
    }

    fn sample(&mut self, time_ms: f64, at: [f64; 2]) {
        if let Some(slot) = self.samples.get_mut(self.sampled % SAMPLES) {
            *slot = (time_ms, at);
        }
        self.sampled += 1;
    }

    /// Velocity over the samples within `VELOCITY_WINDOW_MS` of the last one.
    fn velocity(&self) -> Option<[f64; 2]> {
        let nth_last = |k: usize| {
            self.sampled
                .checked_sub(k + 1)
                .and_then(|i| self.samples.get(i % SAMPLES))
        };
        let last = nth_last(0)?;
        let first = (1..SAMPLES.min(self.sampled))
            .map_while(nth_last)
            .take_while(|s| last.0 - s.0 <= VELOCITY_WINDOW_MS)
            .last()?;
        let dt = last.0 - first.0;
        (dt > 0.0).then(|| [(last.1[0] - first.1[0]) / dt, (last.1[1] - first.1[1]) / dt])
    }
}

fn mid(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    length([a[0] - b[0], a[1] - b[1]])
}

/// Whether `v` is a non-zero whole number; `trunc` is one WASM instruction,
/// where `%` would pull in 2.4 KB of `fmod`.
fn is_whole_nonzero(v: f64) -> bool {
    v != 0.0 && v == v.trunc()
}
