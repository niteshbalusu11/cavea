//! The single row tool (`docs/07-designer.md` §4): click the start, click
//! the end (or drag from one to the other); a third click bends the row
//! through it, a click on the end or Enter keeps it straight.

use cavea_compiler::document::{Node, Path};

use super::block::SEAT_PITCH;
use super::draw::{Live, Spot, dist, finish, px};
use super::snap;
use super::{Button, Context, Cursor, Input, Key, Overlay, Pointer, Readout, Tool, tuning};
use crate::draft::Draft;
use crate::index::P;
use crate::selection::{Selection, View};

#[derive(Debug, Default)]
pub(crate) struct RowTool {
    state: State,
    held: bool,
}

#[derive(Debug, Default)]
enum State {
    #[default]
    Idle,
    /// The start is placed; the row runs to the pointer. `drag` is the
    /// press while it is held and has not yet moved past the threshold.
    Start {
        spot: Spot,
        a: P,
        down: Option<Pointer>,
        dragged: bool,
        live: Live,
    },
    /// Both ends are placed; the row bends through the pointer.
    End {
        spot: Spot,
        a: P,
        b: P,
        live: Live,
    },
    Swallow,
}

impl Tool for RowTool {
    fn input(&mut self, cx: &mut Context<'_>, input: &Input) {
        match *input {
            Input::PointerDown(_) if self.held => {
                self.reset(cx);
                (self.state, self.held) = (State::Swallow, true);
            }
            Input::PointerDown(p) if p.button == Button::Primary => {
                self.held = true;
                self.down(cx, p);
            }
            Input::PointerMove(p) => self.moved(cx, p),
            Input::PointerUp(p) => {
                self.held = false;
                match &mut self.state {
                    State::Start {
                        spot,
                        a,
                        down: Some(_),
                        dragged,
                        live,
                    } => {
                        if *dragged {
                            let at = snap::point(cx, &p, live.created());
                            let b = spot.local(cx, at);
                            self.state = State::End {
                                spot: *spot,
                                a: *a,
                                b,
                                live: std::mem::take(live),
                            };
                            self.bend(cx, None);
                        } else if let State::Start { down, .. } = &mut self.state {
                            *down = None;
                        }
                    }
                    State::Swallow => self.state = State::Idle,
                    _ => {}
                }
            }
            Input::PointerCancel | Input::KeyDown(Key::Escape, _) => self.reset(cx),
            Input::KeyDown(Key::Enter, _) => {
                if let State::End { .. } = self.state {
                    self.bend(cx, None);
                }
                if let State::Start { spot, live, .. } | State::End { spot, live, .. } =
                    &mut self.state
                {
                    finish(cx, live, spot);
                }
                let held = self.held;
                self.reset(cx);
                if held {
                    (self.state, self.held) = (State::Swallow, true);
                }
            }
            _ => {}
        }
    }

    fn reset(&mut self, cx: &mut Context<'_>) {
        if let State::Start { live, .. } | State::End { live, .. } = &mut self.state {
            live.cancel(cx);
        }
        self.state = State::Idle;
        self.held = false;
    }

    fn cursor(&self) -> Cursor {
        Cursor::Crosshair
    }

    fn overlay(&self, view: &View<'_>, _: &Selection, out: &mut Overlay) {
        let (State::Start { live, .. } | State::End { live, .. }) = &self.state else {
            return;
        };
        if let Some(Node::Row(r)) = live.created().and_then(|id| view.doc.node(id)) {
            let Path::Polyline { points } = &r.path;
            let length = points.windows(2).map(|w| dist(w[0], w[1])).sum();
            out.readout = Some(Readout::Row {
                length,
                seats: r.seats,
            });
        }
    }
}

impl RowTool {
    fn down(&mut self, cx: &mut Context<'_>, p: Pointer) {
        match &mut self.state {
            State::Idle => {
                let spot = Spot::at(cx, &p);
                let at = snap::point(cx, &p, None);
                let a = spot.local(cx, at);
                self.state = State::Start {
                    spot,
                    a,
                    down: Some(p),
                    dragged: false,
                    live: Live::default(),
                };
            }
            // The second click places the end.
            State::Start { spot, a, live, .. } => {
                let at = snap::point(cx, &p, live.created());
                let b = spot.local(cx, at);
                self.state = State::End {
                    spot: *spot,
                    a: *a,
                    b,
                    live: std::mem::take(live),
                };
                self.bend(cx, None);
            }
            // The third: on the end it keeps the row straight; elsewhere
            // it bends the row through it.
            State::End { spot, b, live, .. } => {
                let at = snap::point(cx, &p, live.created());
                let m = spot.local(cx, at);
                let near = dist(m, *b) <= px(&p, tuning::DOUBLE_CLICK_PX);
                self.bend(cx, (!near).then_some(m));
                if let State::End { spot, live, .. } = &mut self.state {
                    finish(cx, live, spot);
                }
                self.reset(cx);
                (self.state, self.held) = (State::Swallow, true);
            }
            State::Swallow => {}
        }
    }

    fn moved(&mut self, cx: &mut Context<'_>, p: Pointer) {
        match &mut self.state {
            State::Start {
                spot,
                a,
                down,
                dragged,
                live,
            } => {
                if let Some(d) = down {
                    *dragged |= p.dragged_from(d);
                }
                let at = snap::point(cx, &p, live.created());
                let draft = Draft::Row {
                    start: *a,
                    end: spot.local(cx, at),
                    bend: None,
                    seat_pitch: SEAT_PITCH,
                };
                match spot.create(cx.category, &draft) {
                    Some(command) => drop(live.show(cx, command)),
                    // Too short to be a row: nothing shows.
                    None => live.clear(cx),
                }
            }
            State::End { spot, b, live, .. } => {
                let at = snap::point(cx, &p, live.created());
                let m = spot.local(cx, at);
                let near = dist(m, *b) <= px(&p, tuning::DOUBLE_CLICK_PX);
                self.bend(cx, (!near).then_some(m));
            }
            State::Idle | State::Swallow => {}
        }
    }

    /// Shows the row between the ends, bent through `m` if given.
    fn bend(&mut self, cx: &mut Context<'_>, m: Option<P>) {
        if let State::End { spot, a, b, live } = &mut self.state {
            let draft = Draft::Row {
                start: *a,
                end: *b,
                bend: m,
                seat_pitch: SEAT_PITCH,
            };
            if let Some(command) = spot.create(cx.category, &draft) {
                live.show(cx, command);
            }
        }
    }
}
