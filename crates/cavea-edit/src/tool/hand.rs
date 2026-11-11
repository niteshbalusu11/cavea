//! The hand tool (`docs/07-designer.md` §4): a drag pans the view. The
//! camera is the shell's and the renderer's, so the tool changes nothing:
//! it reports how far the pointer moved, in CSS pixels, and the session
//! hands that to the shell with the input's result.

use super::{Context, Cursor, Input, Overlay, Tool};
use crate::index::P;
use crate::selection::{Selection, View};

#[derive(Debug, Default)]
pub(crate) struct HandTool {
    /// The pointer's last screen position while pressed.
    last: Option<P>,
    /// Pan not yet handed out, in CSS pixels.
    pan: Option<P>,
}

impl HandTool {
    fn drag(&mut self, to: P) {
        if let Some(from) = self.last.replace(to) {
            let pan = self.pan.get_or_insert([0.0; 2]);
            pan[0] += to[0] - from[0];
            pan[1] += to[1] - from[1];
        }
    }
}

impl Tool for HandTool {
    fn input(&mut self, _cx: &mut Context<'_>, input: &Input) {
        match input {
            Input::PointerDown(p) if self.last.is_none() => self.last = Some(p.screen),
            Input::PointerMove(p) if self.last.is_some() => self.drag(p.screen),
            Input::PointerUp(p) if self.last.is_some() => {
                self.drag(p.screen);
                self.last = None;
            }
            Input::PointerCancel => self.last = None,
            _ => {}
        }
    }

    fn reset(&mut self, _cx: &mut Context<'_>) {
        self.last = None;
    }

    fn cursor(&self) -> Cursor {
        match self.last {
            Some(_) => Cursor::Grabbing,
            None => Cursor::Grab,
        }
    }

    fn overlay(&self, _view: &View<'_>, _selection: &Selection, _out: &mut Overlay) {}

    fn take_pan(&mut self) -> Option<P> {
        self.pan.take()
    }
}
