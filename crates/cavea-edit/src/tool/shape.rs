//! The shape tools (`docs/07-designer.md` §4): rectangle and ellipse by
//! dragging, a line by dragging or two clicks, a polygon by clicking its
//! corners.

use super::draw::{Live, Spot, dist, finish, px, select, snap15};
use super::{Button, Context, Cursor, Input, Key, Overlay, Pointer, Readout, Tool};
use crate::command::Command;
use crate::draft::{Draft, MAX_POINTS, MIN_SIZE, closes, extends};
use crate::index::P;
use crate::selection::{Selection, View};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShapeKindTool {
    Rect,
    Ellipse,
    Line,
    Polygon,
}

#[derive(Debug)]
pub(crate) struct ShapeTool {
    kind: ShapeKindTool,
    state: State,
    held: bool,
}

#[derive(Debug, Default)]
enum State {
    #[default]
    Idle,
    /// Pressed at `a` (in the parent's space): the shape shows once the
    /// press becomes a drag; for a line released without a drag, the end
    /// follows the pointer until the next click.
    Drag {
        down: Pointer,
        spot: Spot,
        a: P,
        dragged: bool,
        clicks: bool,
        readout: Option<Readout>,
        live: Live,
    },
    /// Corners clicked so far, and the pointer.
    Polygon {
        spot: Spot,
        points: Vec<P>,
        pointer: Option<P>,
    },
    Swallow,
}

impl ShapeTool {
    pub(crate) fn new(kind: ShapeKindTool) -> Self {
        Self {
            kind,
            state: State::Idle,
            held: false,
        }
    }

    fn draft(&self, a: P, mut b: P, shift: bool) -> (Draft, Readout) {
        if self.kind == ShapeKindTool::Line {
            if shift {
                b = snap15(a, b);
            }
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let readout = Readout::Line {
                length: dx.hypot(dy),
                angle: dy.atan2(dx).to_degrees(),
            };
            return (Draft::Line { start: a, end: b }, readout);
        }
        let (mut w, mut h) = ((b[0] - a[0]).abs(), (b[1] - a[1]).abs());
        if shift {
            (w, h) = (w.max(h), w.max(h));
        }
        let size = [w.max(MIN_SIZE), h.max(MIN_SIZE)];
        let sx = if b[0] >= a[0] { 1.0 } else { -1.0 };
        let sy = if b[1] >= a[1] { 1.0 } else { -1.0 };
        let center = [a[0] + sx * size[0] / 2.0, a[1] + sy * size[1] / 2.0];
        let readout = Readout::Size {
            width: size[0],
            height: size[1],
        };
        let draft = match self.kind {
            ShapeKindTool::Ellipse => Draft::Ellipse { center, size },
            _ => Draft::Rect { center, size },
        };
        (draft, readout)
    }

    fn moved(&mut self, cx: &mut Context<'_>, p: Pointer) {
        let (draft, readout) = match &mut self.state {
            State::Drag {
                down,
                spot,
                a,
                dragged,
                clicks,
                ..
            } => {
                *dragged |= p.dragged_from(down);
                if !*dragged && !*clicks {
                    return;
                }
                let b = spot.local(cx, p.world);
                let a = *a;
                self.draft(a, b, p.mods.shift)
            }
            State::Polygon {
                spot,
                points,
                pointer,
            } => {
                let q = spot.local(cx, p.world);
                *pointer = Some(match points.last() {
                    Some(&last) if p.mods.shift => snap15(last, q),
                    _ => q,
                });
                return;
            }
            State::Idle | State::Swallow => return,
        };
        if let State::Drag {
            spot,
            live,
            readout: shown,
            ..
        } = &mut self.state
            && let Some(command) = spot.create(cx.category, &draft)
            && live.show(cx, command)
        {
            *shown = Some(readout);
        }
    }

    /// A polygon click: on the first corner it closes the polygon, if that
    /// makes a valid one; elsewhere it adds a corner, unless the new edge
    /// would cross the outline or the polygon is full.
    fn click_polygon(&mut self, cx: &mut Context<'_>, p: Pointer) {
        let State::Polygon {
            spot,
            points,
            pointer,
        } = &mut self.state
        else {
            let spot = Spot::at(cx, &p);
            let q = spot.local(cx, p.world);
            self.state = State::Polygon {
                spot,
                points: vec![q],
                pointer: Some(q),
            };
            return;
        };
        let mut q = spot.local(cx, p.world);
        if let (Some(&last), true) = (points.last(), p.mods.shift) {
            q = snap15(last, q);
        }
        let on_first = points
            .first()
            .is_some_and(|&f| dist(f, q) <= p.tolerance().max(px(&p, 1.0)));
        if on_first && points.len() >= 3 {
            self.close(cx);
        } else if points.len() < MAX_POINTS && extends(points, q) {
            points.push(q);
            *pointer = Some(q);
        }
    }

    /// Creates the polygon, as one undo entry, if it closes validly.
    fn close(&mut self, cx: &mut Context<'_>) {
        let State::Polygon { spot, points, .. } = &self.state else {
            return;
        };
        if !closes(points) {
            return;
        }
        let draft = Draft::Polygon {
            points: points.clone(),
        };
        let spot = *spot;
        let Some(command @ Command::CreateNode { .. }) = spot.create(cx.category, &draft) else {
            return;
        };
        if let Ok(applied) = cx.editor.apply(command)
            && let Some(&id) = applied.created.first()
        {
            select(cx.selection, cx.editor.document(), spot.level(), id);
        }
        self.state = State::Idle;
    }
}

impl Tool for ShapeTool {
    fn input(&mut self, cx: &mut Context<'_>, input: &Input) {
        match *input {
            Input::PointerDown(_) if self.held => {
                self.reset(cx);
                (self.state, self.held) = (State::Swallow, true);
            }
            Input::PointerDown(p) if p.button == Button::Primary => {
                self.held = true;
                if self.kind == ShapeKindTool::Polygon {
                    return self.click_polygon(cx, p);
                }
                match &mut self.state {
                    // A line's second click places its end.
                    State::Drag { clicks: true, .. } => {
                        self.moved(cx, p);
                        if let State::Drag { spot, live, .. } = &mut self.state {
                            let spot = *spot;
                            finish(cx, live, &spot);
                        }
                        self.reset(cx);
                        (self.state, self.held) = (State::Swallow, true);
                    }
                    _ => {
                        let spot = Spot::at(cx, &p);
                        let a = spot.local(cx, p.world);
                        self.state = State::Drag {
                            down: p,
                            spot,
                            a,
                            dragged: false,
                            clicks: false,
                            readout: None,
                            live: Live::default(),
                        };
                    }
                }
            }
            Input::PointerMove(p) => self.moved(cx, p),
            Input::PointerUp(p) => {
                self.held = false;
                self.moved(cx, p);
                match &mut self.state {
                    State::Drag {
                        dragged: true,
                        spot,
                        live,
                        ..
                    } => {
                        let spot = *spot;
                        finish(cx, live, &spot);
                        self.reset(cx);
                    }
                    // A line's press without a drag: its end comes with the
                    // next click.
                    State::Drag { clicks, .. } if self.kind == ShapeKindTool::Line => {
                        *clicks = true;
                    }
                    State::Drag { .. } | State::Swallow => self.reset(cx),
                    State::Idle | State::Polygon { .. } => {}
                }
            }
            Input::PointerCancel | Input::KeyDown(Key::Escape, _) => self.reset(cx),
            Input::KeyDown(Key::Enter, _) => match &mut self.state {
                State::Polygon { .. } => self.close(cx),
                State::Drag { spot, live, .. } if live.shown() && !self.held => {
                    let spot = *spot;
                    finish(cx, live, &spot);
                    self.reset(cx);
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn reset(&mut self, cx: &mut Context<'_>) {
        if let State::Drag { live, .. } = &mut self.state {
            live.cancel(cx);
        }
        self.state = State::Idle;
        self.held = false;
    }

    fn cursor(&self) -> Cursor {
        Cursor::Crosshair
    }

    fn overlay(&self, view: &View<'_>, _: &Selection, out: &mut Overlay) {
        match &self.state {
            State::Drag { readout, .. } => out.readout = *readout,
            State::Polygon {
                spot,
                points,
                pointer,
            } => {
                let world = |p: P| match spot.parent {
                    Some(s) => view.index.to_world(s, p).unwrap_or(p),
                    None => p,
                };
                out.preview = points.iter().chain(pointer).map(|&p| world(p)).collect();
                out.readout = Some(Readout::Points(
                    u32::try_from(points.len()).unwrap_or(u32::MAX),
                ));
            }
            State::Idle | State::Swallow => {}
        }
    }
}
