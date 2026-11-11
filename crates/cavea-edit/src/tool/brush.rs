//! The category brush (`docs/07-designer.md` §4): a press and drag assigns
//! the active category to what the pointer passes over, at the selection's
//! level: a node's `category` at a node level (the top level reaches a
//! section, as a click does); every seat of a row at the rows level; each
//! seat at the seat level. Seats take an override, or lose it when the
//! category is the one they inherit. A node painted keeps its seats'
//! overrides. One stroke is one gesture: one undo entry, Escape restores.

use std::collections::BTreeSet;

use super::{Button, Context, Cursor, Input, Key, Overlay, Tool};
use crate::command::{Command, Property, SeatEdit, category};
use crate::index::P;
use crate::selection::{Level, Selection, View};

#[derive(Debug, Default)]
pub(crate) struct BrushTool {
    /// The stroke in progress: the last pointer position and the items
    /// painted so far (node ids, rids or oids, by level).
    stroke: Option<(P, BTreeSet<u32>)>,
}

/// Whether `p` is within `r` of the segment from `a` to `b`.
fn near(p: P, a: P, b: P, r: f64) -> bool {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (x, y) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    x * x + y * y <= r * r
}

/// The command painting what the segment `a`–`b` (world) passes over that
/// `done` does not hold yet; adds it to `done`.
fn paint(
    view: &View<'_>,
    sel: &Selection,
    key: Option<&str>,
    (a, b): (P, P),
    done: &mut BTreeSet<u32>,
) -> Option<Command> {
    let value = key.map(String::from);
    let level = sel.level();
    let (Level::Rows(id) | Level::Seats(id)) = level else {
        let id = view.hit(level, b).filter(|&id| done.insert(id))?;
        let node = view.doc.node(id)?;
        category(node).filter(|c| c.as_deref() != key)?;
        let property = Property::Category(value);
        return Some(Command::SetProperty { id, property });
    };
    if !view.pickable(level, id) {
        return None;
    }
    let ix = view.index;
    let place = ix.place(id)?;
    let (a, b) = (place.invert(a), place.invert(b));
    let seats = ix.seats(id);
    let mut oids = Vec::new();
    if let Level::Rows(_) = level {
        let reach = ix.reach(id) + view.tol;
        for row in seats.chunk_by(|x, y| x.row == y.row) {
            let hit = row.iter().any(|s| !s.removed && near(s.pos, a, b, reach));
            if hit && row.first().is_some_and(|s| done.insert(s.row)) {
                oids.extend(row.iter().map(|s| s.oid));
            }
        }
    } else {
        let reach = ix.seat_pad(id) + view.tol;
        let hit = |s: &&crate::index::CachedSeat| !s.removed && near(s.pos, a, b, reach);
        oids.extend(seats.iter().filter(hit).map(|s| s.oid));
        oids.retain(|&o| done.insert(o));
    }
    let edit = SeatEdit::Category { category: value };
    (!oids.is_empty()).then_some(Command::EditSeats { id, oids, edit })
}

impl BrushTool {
    fn to(&mut self, cx: &mut Context<'_>, at: P, tol: f64) {
        let Some((last, done)) = &mut self.stroke else {
            return;
        };
        let key = cx.category;
        let (view, sel) = cx.split(tol);
        let command = paint(&view, sel, key, (*last, at), done);
        *last = at;
        if let Some(c) = command {
            let _ = cx.editor.apply(c);
        }
    }
}

impl Tool for BrushTool {
    fn input(&mut self, cx: &mut Context<'_>, input: &Input) {
        match *input {
            Input::PointerDown(_) if self.stroke.is_some() => self.reset(cx),
            Input::PointerDown(p) if p.button == Button::Primary => {
                if cx.editor.begin_gesture().is_ok() {
                    self.stroke = Some((p.world, BTreeSet::new()));
                    self.to(cx, p.world, p.tolerance());
                }
            }
            Input::PointerMove(p) => self.to(cx, p.world, p.tolerance()),
            Input::PointerUp(p) if self.stroke.is_some() => {
                self.to(cx, p.world, p.tolerance());
                self.stroke = None;
                let _ = cx.editor.end_gesture();
            }
            Input::PointerCancel | Input::KeyDown(Key::Escape, _) => self.reset(cx),
            _ => {}
        }
    }

    fn reset(&mut self, cx: &mut Context<'_>) {
        if self.stroke.take().is_some() {
            let _ = cx.editor.cancel_gesture();
        }
    }

    fn cursor(&self) -> Cursor {
        Cursor::Crosshair
    }

    fn overlay(&self, _: &View<'_>, _: &Selection, _: &mut Overlay) {}
}
