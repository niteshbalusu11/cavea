//! The row block and arc block tools (`docs/07-designer.md` §4).

use std::f64::consts::{PI, TAU};

use cavea_core::limits::MAX_SEATS_PER_ROW;

use super::draw::{Entry, Live, Spot, budget, count, dist, finish};
use super::snap;
use super::{Button, Context, Cursor, Input, Key, Overlay, Pointer, Readout, Tool, tuning};
use crate::draft::{Draft, MIN_RADIUS, arc_max_seats, arc_row_seats};
use crate::index::P;
use crate::selection::{Selection, View};

/// 03 §1: the default seat and row pitch.
pub(crate) const SEAT_PITCH: f64 = 55.0;
pub(crate) const ROW_PITCH: f64 = 95.0;

/// Seats a preview may hold: the live cap, within what the chart may take.
fn cap(budget: u64, typed: bool) -> u64 {
    if typed {
        budget
    } else {
        budget.min(u64::from(tuning::LIVE_SEATS))
    }
}

#[derive(Debug, Default)]
pub(crate) struct BlockTool {
    state: Block,
    entry: Entry,
}

#[derive(Debug, Default)]
enum Block {
    #[default]
    Idle,
    /// Dragging from `a` (in the parent's space): the block is shown from
    /// the press. `budget` is what the chart could take at the press.
    Drag {
        down: Pointer,
        spot: Spot,
        a: P,
        b: P,
        shift: bool,
        dragged: bool,
        budget: u64,
        counts: (u32, u32),
        live: Live,
    },
    /// Done or abandoned before the release: wait for it.
    Swallow,
}

/// The row block for a drag from `a` to `b`: seats per row and rows are the
/// drag's width and height over the pitches, rounded (at least 1, Shift
/// squaring the drag first); each seat takes a pitch-wide cell, the cells
/// starting at `a` and running towards `b`, with the front row on top.
fn block_counts(a: P, b: P, shift: bool, budget: u64) -> (u32, u32) {
    let (mut w, mut h) = ((b[0] - a[0]).abs(), (b[1] - a[1]).abs());
    if shift {
        (w, h) = (w.max(h), w.max(h));
    }
    let seats = count(w / SEAT_PITCH, MAX_SEATS_PER_ROW);
    let rows = count(h / ROW_PITCH, rows_max(budget, seats));
    (rows, seats)
}

fn rows_max(budget: u64, seats: u32) -> u32 {
    u32::try_from(budget / u64::from(seats.max(1))).unwrap_or(u32::MAX)
}

fn block_draft(a: P, b: P, (rows, seats): (u32, u32)) -> Draft {
    let sx = if b[0] >= a[0] { 1.0 } else { -1.0 };
    let x = a[0] + sx * f64::from(seats) * SEAT_PITCH / 2.0;
    let top = if b[1] >= a[1] {
        a[1]
    } else {
        a[1] - f64::from(rows) * ROW_PITCH
    };
    Draft::RowBlock {
        origin: [x, top + ROW_PITCH / 2.0],
        rows,
        seats,
        seat_pitch: SEAT_PITCH,
        row_pitch: ROW_PITCH,
    }
}

impl Tool for BlockTool {
    fn input(&mut self, cx: &mut Context<'_>, input: &Input) {
        match (&mut self.state, *input) {
            (Block::Idle, Input::PointerDown(p)) if p.button == Button::Primary => {
                let spot = Spot::at(cx, &p);
                let at = snap::point(cx, &p, None);
                let a = spot.local(cx, at);
                self.entry = Entry::default();
                self.state = Block::Drag {
                    down: p,
                    spot,
                    a,
                    b: a,
                    shift: p.mods.shift,
                    dragged: false,
                    budget: budget(cx),
                    counts: (0, 0),
                    live: Live::default(),
                };
                self.show(cx, None);
            }
            (Block::Drag { .. }, Input::PointerDown(_)) => {
                self.reset(cx);
                self.state = Block::Swallow;
            }
            (
                Block::Drag {
                    down,
                    spot,
                    b,
                    shift,
                    dragged,
                    live,
                    ..
                },
                Input::PointerMove(p),
            ) => {
                let at = snap::point(cx, &p, live.created());
                (*b, *shift) = (spot.local(cx, at), p.mods.shift);
                *dragged |= p.dragged_from(down);
                self.show(cx, None);
            }
            (Block::Drag { .. }, Input::PointerUp(p)) => {
                self.input(cx, &Input::PointerMove(p));
                if let Block::Drag {
                    dragged: true,
                    spot,
                    live,
                    ..
                } = &mut self.state
                {
                    finish(cx, live, spot);
                }
                self.reset(cx);
            }
            (Block::Swallow, Input::PointerUp(_)) => self.state = Block::Idle,
            (_, Input::PointerCancel) | (_, Input::KeyDown(Key::Escape, _)) => self.reset(cx),
            (Block::Drag { .. }, Input::KeyDown(Key::Enter, _)) => {
                let typed = self.entry.fields();
                self.show(cx, Some(typed));
                if let Block::Drag { spot, live, .. } = &mut self.state {
                    finish(cx, live, spot);
                }
                self.reset(cx);
                self.state = Block::Swallow;
            }
            (Block::Drag { .. }, Input::KeyDown(key, _)) => drop(self.entry.key(key)),
            _ => {}
        }
    }

    fn reset(&mut self, cx: &mut Context<'_>) {
        if let Block::Drag { live, .. } = &mut self.state {
            live.cancel(cx);
        }
        self.state = Block::Idle;
        self.entry = Entry::default();
    }

    fn cursor(&self) -> Cursor {
        Cursor::Crosshair
    }

    fn overlay(&self, _: &View<'_>, _: &Selection, out: &mut Overlay) {
        if let Block::Drag { counts, .. } = self.state {
            out.readout = Some(Readout::Block {
                rows: counts.0,
                seats: counts.1,
                seat_pitch: SEAT_PITCH,
                row_pitch: ROW_PITCH,
            });
            out.entry.clone_from(&self.entry.0);
        }
    }
}

impl BlockTool {
    /// Shows the block for the drag, or for the typed rows and seats.
    fn show(&mut self, cx: &mut Context<'_>, typed: Option<[Option<f64>; 3]>) {
        let Block::Drag {
            spot,
            a,
            b,
            shift,
            budget,
            counts,
            live,
            ..
        } = &mut self.state
        else {
            return;
        };
        let room = cap(*budget, typed.is_some());
        let mut c = block_counts(*a, *b, *shift, room);
        if let Some([rows, seats, _]) = typed {
            c.1 = seats.map_or(c.1, |v| count(v, MAX_SEATS_PER_ROW));
            c.0 = rows.map_or(c.0, |v| count(v, rows_max(room, c.1)));
            c.0 = c.0.min(rows_max(room, c.1)).max(1);
        }
        if let Some(command) = spot.create(cx.category, &block_draft(*a, *b, c))
            && live.show(cx, command)
        {
            *counts = c;
        }
    }
}

/// The arc block tool: click the centre; press on the first row and drag
/// along it; then drag outward for rows.
#[derive(Debug, Default)]
pub(crate) struct ArcTool {
    state: Arc,
    entry: Entry,
    held: bool,
}

#[derive(Debug, Default)]
enum Arc {
    #[default]
    Idle,
    /// The centre is placed (in the parent's space).
    Centre {
        spot: Spot,
        c: P,
        budget: u64,
    },
    /// Dragging the first row: its start angle, the angle swept since (in
    /// radians, unwrapped, clockwise positive) and the pointer's last angle.
    Span {
        spot: Spot,
        c: P,
        budget: u64,
        start: f64,
        sweep: f64,
        last: f64,
        shape: ArcShape,
        live: Live,
    },
    /// Adding rows outward; a press then release finishes.
    Rows {
        spot: Spot,
        c: P,
        budget: u64,
        pressed: bool,
        shape: ArcShape,
        live: Live,
    },
    Swallow,
}

/// What an arc block shows: the front row's radius, the apex direction in
/// degrees, its seats, and rows.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ArcShape {
    radius: f64,
    direction: f64,
    seats: u32,
    rows: u32,
}

impl ArcShape {
    fn draft(&self, c: P) -> Draft {
        Draft::ArcBlock {
            center: c,
            radius: self.radius,
            direction: self.direction,
            rows: self.rows,
            seats: self.seats,
            seat_pitch: SEAT_PITCH,
            row_pitch: ROW_PITCH,
        }
    }

    /// The angle the front row's seats span, in degrees.
    fn span(&self) -> f64 {
        (f64::from(self.seats.saturating_sub(1)) * SEAT_PITCH / self.radius).to_degrees()
    }

    /// Rows cut back until every row fits the limits and `room` seats.
    fn fit(mut self, room: u64) -> Self {
        let mut total = 0u64;
        let mut rows = 0;
        while rows < self.rows {
            let n = arc_row_seats(self.seats, self.radius, ROW_PITCH, rows);
            total += u64::from(n);
            if n > MAX_SEATS_PER_ROW || total > room {
                break;
            }
            rows += 1;
        }
        self.rows = rows.max(1);
        self
    }
}

/// The radius at `q`, kept to what the compiler accepts and finite.
fn radius(c: P, q: P) -> f64 {
    let r = dist(c, q);
    if r.is_finite() {
        r.max(MIN_RADIUS)
    } else {
        MIN_RADIUS
    }
}

/// An angle difference wrapped into (−π, π].
fn wrap(a: f64) -> f64 {
    let r = a % TAU;
    if r > PI {
        r - TAU
    } else if r <= -PI {
        r + TAU
    } else {
        r
    }
}

impl Tool for ArcTool {
    fn input(&mut self, cx: &mut Context<'_>, input: &Input) {
        match *input {
            Input::PointerDown(p) if self.held => {
                self.reset(cx);
                self.state = Arc::Swallow;
                self.held = true;
                let _ = p;
            }
            Input::PointerDown(p) if p.button == Button::Primary => {
                self.held = true;
                self.down(cx, p);
            }
            Input::PointerMove(p) => self.moved(cx, p),
            Input::PointerUp(p) => {
                self.held = false;
                self.moved(cx, p);
                match &mut self.state {
                    Arc::Span {
                        spot,
                        c,
                        budget,
                        shape,
                        live,
                        ..
                    } => {
                        self.state = Arc::Rows {
                            spot: *spot,
                            c: *c,
                            budget: *budget,
                            pressed: false,
                            shape: *shape,
                            live: std::mem::take(live),
                        };
                    }
                    Arc::Rows {
                        pressed: true,
                        spot,
                        live,
                        ..
                    } => {
                        finish(cx, live, spot);
                        self.reset(cx);
                    }
                    Arc::Swallow => self.state = Arc::Idle,
                    _ => {}
                }
            }
            Input::PointerCancel | Input::KeyDown(Key::Escape, _) => self.reset(cx),
            Input::KeyDown(Key::Enter, _) => self.enter(cx),
            Input::KeyDown(key, _) if matches!(self.state, Arc::Span { .. } | Arc::Rows { .. }) => {
                self.entry.key(key);
            }
            _ => {}
        }
    }

    fn reset(&mut self, cx: &mut Context<'_>) {
        if let Arc::Span { live, .. } | Arc::Rows { live, .. } = &mut self.state {
            live.cancel(cx);
        }
        self.state = Arc::Idle;
        self.entry = Entry::default();
        self.held = false;
    }

    fn cursor(&self) -> Cursor {
        Cursor::Crosshair
    }

    fn overlay(&self, _: &View<'_>, _: &Selection, out: &mut Overlay) {
        if let Arc::Span { shape, .. } | Arc::Rows { shape, .. } = &self.state {
            out.readout = Some(Readout::Arc {
                radius: shape.radius,
                span: shape.span(),
                rows: shape.rows,
                seats: shape.seats,
            });
            out.entry.clone_from(&self.entry.0);
        }
    }
}

impl ArcTool {
    fn down(&mut self, cx: &mut Context<'_>, p: Pointer) {
        match &mut self.state {
            Arc::Idle => {
                let spot = Spot::at(cx, &p);
                let at = snap::point(cx, &p, None);
                let c = spot.local(cx, at);
                let budget = budget(cx);
                self.state = Arc::Centre { spot, c, budget };
            }
            Arc::Centre { spot, c, budget } => {
                let at = snap::point(cx, &p, None);
                let q = spot.local(cx, at);
                let start = (q[1] - c[1]).atan2(q[0] - c[0]);
                let shape = ArcShape {
                    radius: radius(*c, q),
                    direction: start.to_degrees(),
                    seats: 1,
                    rows: 1,
                };
                self.entry = Entry::default();
                self.state = Arc::Span {
                    spot: *spot,
                    c: *c,
                    budget: *budget,
                    start,
                    sweep: 0.0,
                    last: start,
                    shape,
                    live: Live::default(),
                };
                self.show(cx, shape);
            }
            Arc::Rows { pressed, .. } => {
                *pressed = true;
                self.moved(cx, p);
            }
            Arc::Span { .. } | Arc::Swallow => {}
        }
    }

    fn moved(&mut self, cx: &mut Context<'_>, p: Pointer) {
        let shape = match &mut self.state {
            Arc::Span {
                spot,
                c,
                budget,
                start,
                sweep,
                last,
                shape,
                ..
            } => {
                let q = spot.local(cx, p.world);
                let r = radius(*c, q);
                let angle = (q[1] - c[1]).atan2(q[0] - c[0]);
                // The span stops short of a full turn (03 §2.4: every seat
                // within half a turn of the apex, the ends a pitch apart).
                let most = (TAU - SEAT_PITCH / r).max(0.0);
                *sweep = (*sweep + wrap(angle - *last)).clamp(-most, most);
                *last = angle;
                let seats = (r * sweep.abs() / SEAT_PITCH + 1e-9).floor() + 1.0;
                let seats = (seats as u32).min(arc_max_seats(r, SEAT_PITCH));
                let next = ArcShape {
                    radius: r,
                    direction: (*start + *sweep / 2.0).to_degrees(),
                    seats,
                    rows: 1,
                };
                Some(next.fit(cap(*budget, false)))
                    .filter(|n| n != shape)
                    .inspect(|n| *shape = *n)
            }
            Arc::Rows {
                spot,
                c,
                budget,
                shape,
                ..
            } => {
                let q = spot.local(cx, p.world);
                let out = (dist(*c, q) - shape.radius) / ROW_PITCH;
                let rows = if out.is_finite() {
                    out.round().max(0.0)
                } else {
                    0.0
                };
                let rows = (rows.min(f64::from(tuning::LIVE_SEATS)) as u32) + 1;
                let next = ArcShape { rows, ..*shape }.fit(cap(*budget, false));
                Some(next).filter(|n| n != shape).inspect(|n| *shape = *n)
            }
            _ => None,
        };
        if let Some(shape) = shape {
            self.show(cx, shape);
        }
    }

    fn show(&mut self, cx: &mut Context<'_>, shape: ArcShape) {
        if let Arc::Span { spot, c, live, .. } | Arc::Rows { spot, c, live, .. } = &mut self.state
            && let Some(command) = spot.create(cx.category, &shape.draft(*c))
        {
            live.show(cx, command);
        }
    }

    /// Takes the typed radius, seats and rows, and finishes.
    fn enter(&mut self, cx: &mut Context<'_>) {
        let [radius, seats, rows] = self.entry.fields();
        let (Arc::Span { shape, budget, .. } | Arc::Rows { shape, budget, .. }) = &mut self.state
        else {
            return;
        };
        let mut next = *shape;
        if let Some(r) = radius {
            next.radius = r.clamp(MIN_RADIUS, 1e9);
        }
        let most = arc_max_seats(next.radius, SEAT_PITCH);
        next.seats = seats.map_or(next.seats.min(most), |v| count(v, most));
        next.rows = rows.map_or(next.rows, |v| count(v, MAX_SEATS_PER_ROW * 256));
        let next = next.fit(cap(*budget, true));
        *shape = next;
        self.show(cx, next);
        if let Arc::Span { spot, live, .. } | Arc::Rows { spot, live, .. } = &mut self.state {
            finish(cx, live, spot);
        }
        let held = self.held;
        self.reset(cx);
        if held {
            (self.state, self.held) = (Arc::Swallow, true);
        }
    }
}
