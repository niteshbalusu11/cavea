//! The focal point tool (`docs/07-designer.md` §4): a press sets the focal
//! point of the floor shown, dragging moves it live, and the release ends
//! the gesture as one undo entry; Escape restores it. The point is snapped
//! as a point (§5.2). Only the floor's `focalPoint` changes: left and right,
//! row order and seat labels are stored, not derived from it.

use super::{Button, Context, Cursor, Input, Key, Overlay, Pointer, Tool, snap};
use crate::command::Command;
use crate::document::Document;
use crate::index::P;
use crate::selection::{Selection, View};

#[derive(Debug, Default)]
pub(crate) struct FocalTool {
    open: bool,
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

impl FocalTool {
    fn set(&self, cx: &mut Context<'_>, p: &Pointer) {
        let at = snap::point(cx, p, None, None);
        if let Some(c) = command(cx.editor.document(), cx.floor, Some(at)) {
            let _ = cx.editor.apply(c);
        }
    }
}

impl Tool for FocalTool {
    fn input(&mut self, cx: &mut Context<'_>, input: &Input) {
        match *input {
            Input::PointerDown(_) if self.open => self.reset(cx),
            Input::PointerDown(p) if p.button == Button::Primary => {
                self.open = cx.editor.begin_gesture().is_ok();
                if self.open {
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
