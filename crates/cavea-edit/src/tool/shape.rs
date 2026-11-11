//! The shape tools (`docs/07-designer.md` §4): rectangle and ellipse by
//! dragging, a line by dragging or two clicks, a polygon by clicking its
//! corners; and the section tool, which drags a box or clicks a polygon the
//! same way (§5.5).

use super::draw::{Live, Spot, dist, finish, px, select, snap15};
use super::snap;
use super::{Button, Context, Cursor, Input, Key, Overlay, Pointer, Readout, Tool};
use crate::command::Command;
use crate::draft::{Draft, MAX_POINTS, MIN_SIZE, closes, extends};
use crate::index::{P, Rect, Select};
use crate::section;
use crate::selection::{Level, Selection, View};
use cavea_compiler::document::Node;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShapeKindTool {
    Rect,
    Ellipse,
    Line,
    Polygon,
    Section,
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
        b: P,
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
        let kind = self.kind;
        let (draft, readout) = match &mut self.state {
            State::Drag {
                down,
                spot,
                a,
                b,
                dragged,
                clicks,
                live,
                ..
            } => {
                *dragged |= p.dragged_from(down);
                let at = match kind {
                    ShapeKindTool::Line if !p.mods.shift => {
                        snap::line_end(cx, &p, down.world, live.created())
                    }
                    _ => snap::point(cx, &p, live.created()),
                };
                *b = spot.local(cx, at);
                if (!*dragged && !*clicks) || self.kind == ShapeKindTool::Section {
                    return;
                }
                let (a, b) = (*a, *b);
                self.draft(a, b, p.mods.shift)
            }
            State::Polygon {
                spot,
                points,
                pointer,
            } => {
                let at = snap::point(cx, &p, None);
                let q = spot.local(cx, at);
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
            let spot = self.spot(cx, &p);
            let at = snap::point(cx, &p, None);
            let q = spot.local(cx, at);
            self.state = State::Polygon {
                spot,
                points: vec![q],
                pointer: Some(q),
            };
            return;
        };
        let at = snap::point(cx, &p, None);
        let mut q = spot.local(cx, at);
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
        if self.kind == ShapeKindTool::Section {
            let points = points.clone();
            let (view, _) = cx.split(0.0);
            let inside = |id: &u32| {
                let quad = view.index.quad(*id);
                let section = matches!(view.doc.node(*id), Some(Node::Section(_)));
                !section && quad.is_some_and(|q| q.iter().all(|&c| within(&points, c)))
            };
            let ids: Vec<u32> = view.all(Level::Top).into_iter().filter(inside).collect();
            return self.group(cx, &ids, Some(points));
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

    /// Where the node goes: a section always at the top level.
    fn spot(&self, cx: &Context<'_>, p: &Pointer) -> Spot {
        match self.kind {
            ShapeKindTool::Section => Spot::new(cx.editor.document(), None, cx.floor),
            _ => Spot::at(cx, p),
        }
    }

    /// The section tool's box: a section holding the top-level nodes it
    /// contains (sections aside), or, holding none, an empty section with
    /// the box as its outline.
    fn section_box(&mut self, cx: &mut Context<'_>, a: P, b: P) {
        let r = Rect::new(a, b);
        let (view, _) = cx.split(0.0);
        let ids = view.select(Level::Top, r, Select::Contain);
        let doc = view.doc;
        let ids: Vec<u32> = ids
            .into_iter()
            .filter(|&id| !matches!(doc.node(id), Some(Node::Section(_))))
            .collect();
        let size = [r.max[0] - r.min[0], r.max[1] - r.min[1]];
        if !ids.is_empty() {
            self.group(cx, &ids, None);
        } else if size.iter().all(|&v| v >= MIN_SIZE) {
            self.group(cx, &[], Some(r.corners().to_vec()));
        }
    }

    fn group(&mut self, cx: &mut Context<'_>, ids: &[u32], outline: Option<Vec<P>>) {
        let floor = Spot::new(cx.editor.document(), None, cx.floor).floor;
        let made = section::group(cx.editor, cx.index, ids, outline, (cx.category, floor));
        if let Ok(id) = made {
            select(cx.selection, cx.editor.document(), Level::Top, id);
        }
        self.state = State::Idle;
    }
}

/// Whether `p` is inside the polygon `poly` (even-odd; a point on an edge may
/// fall either way).
fn within(poly: &[P], p: P) -> bool {
    let mut inside = false;
    let mut j = poly.len().wrapping_sub(1);
    for (i, &a) in poly.iter().enumerate() {
        if let Some(&b) = poly.get(j)
            && (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1])
        {
            inside = !inside;
        }
        j = i;
    }
    inside
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
                if self.kind == ShapeKindTool::Polygon
                    || matches!(self.state, State::Polygon { .. })
                {
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
                        let spot = self.spot(cx, &p);
                        let at = snap::point(cx, &p, None);
                        let a = spot.local(cx, at);
                        self.state = State::Drag {
                            down: Pointer { world: at, ..p },
                            spot,
                            a,
                            b: a,
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
                    &mut State::Drag {
                        dragged: true,
                        a,
                        b,
                        ..
                    } if self.kind == ShapeKindTool::Section => self.section_box(cx, a, b),
                    // A section's click without a drag starts its polygon.
                    State::Drag { a, spot, .. } if self.kind == ShapeKindTool::Section => {
                        (self.state, self.held) = (
                            State::Polygon {
                                spot: *spot,
                                points: vec![*a],
                                pointer: Some(*a),
                            },
                            false,
                        );
                    }
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
            State::Drag { a, b, dragged, .. } if self.kind == ShapeKindTool::Section => {
                if *dragged {
                    out.marquee = Some(Rect::new(*a, *b));
                }
            }
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
