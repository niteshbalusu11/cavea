//! The focal point tool (`docs/07-designer.md` §4): a press sets the focal
//! point of the floor shown, dragging moves it live, and the release ends
//! the gesture as one undo entry; Escape restores it. The point is snapped
//! as a point (§5.2). Only the floor's `focalPoint` changes: left and right,
//! row order and seat labels are stored, not derived from it.
//!
//! A chart that lists no floors has one implicit floor (03 §2.2), which has
//! no focal point to set: the press first lists that floor (empty name,
//! level 0, as the implicit one is), in the same gesture, so every node
//! stays where it was and undo returns the chart to no floors.

use cavea_compiler::document::Floor;

use super::{Button, Context, Cursor, Input, Key, Overlay, Pointer, Tool, snap};
use crate::command::Command;
use crate::document::Document;
use crate::index::P;
use crate::selection::{Selection, View};

#[derive(Debug, Default)]
pub(crate) struct FocalTool {
    open: bool,
    /// The floor the open gesture sets: the floor shown, or the one it
    /// listed.
    floor: Option<u32>,
}

/// The `updateFloor` setting the focal point of `floor`, if the chart lists
/// it (a chart without floors has none to set) and it changes.
pub(crate) fn command(doc: &Document, floor: Option<u32>, point: Option<P>) -> Option<Command> {
    let mut f = doc.floors().iter().find(|f| Some(f.id) == floor)?.clone();
    if f.focal_point == point {
        return None;
    }
    f.focal_point = point;
    Some(Command::UpdateFloor { floor: f })
}

/// The implicit floor of a chart without floors, listed: what the compiler
/// assumes for it (03 §2.2), so the compiled chart is the same but for the
/// focal point.
fn implicit_floor() -> Floor {
    Floor {
        id: 0,
        name: String::new(),
        level: 0,
        focal_point: None,
    }
}

impl FocalTool {
    fn set(&self, cx: &mut Context<'_>, p: &Pointer) {
        let at = snap::point(cx, p, None, None);
        if let Some(c) = command(cx.editor.document(), self.floor, Some(at)) {
            let _ = cx.editor.apply(c);
        }
    }

    /// The floor to set: the floor shown, or, on a chart that lists none,
    /// its implicit floor listed now, inside the open gesture.
    fn floor(cx: &mut Context<'_>) -> Option<u32> {
        if cx.floor.is_some() || !cx.editor.document().floors().is_empty() {
            return cx.floor;
        }
        let add = Command::AddFloor {
            index: 0,
            floor: implicit_floor(),
        };
        // Named as the focal point's own entry: listing the floor is part
        // of setting it.
        cx.editor.apply_as(add, Some("updateFloor")).ok()?;
        cx.editor.document().floors().first().map(|f| f.id)
    }
}

impl Tool for FocalTool {
    fn input(&mut self, cx: &mut Context<'_>, input: &Input) {
        match *input {
            Input::PointerDown(_) if self.open => self.reset(cx),
            Input::PointerDown(p) if p.button == Button::Primary => {
                self.open = cx.editor.begin_gesture().is_ok();
                if self.open {
                    self.floor = Self::floor(cx);
                    self.set(cx, &p);
                }
            }
            Input::PointerMove(p) if self.open => self.set(cx, &p),
            Input::PointerUp(p) if self.open => {
                self.set(cx, &p);
                let _ = cx.editor.end_gesture();
                self.open = false;
            }
            Input::PointerCancel | Input::KeyDown(Key::Escape, _) => self.reset(cx),
            _ => {}
        }
    }

    fn reset(&mut self, cx: &mut Context<'_>) {
        if std::mem::take(&mut self.open) {
            let _ = cx.editor.cancel_gesture();
        }
    }

    fn cursor(&self) -> Cursor {
        Cursor::Crosshair
    }

    fn overlay(&self, _: &View<'_>, _: &Selection, _: &mut Overlay) {}
}
