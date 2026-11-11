//! Tools (`docs/07-designer.md` §2.3): state machines that turn pointer and
//! key input into selection changes and commands. A tool sees the editor,
//! the spatial index and the selection through a [`Context`]; it changes the
//! document only through commands, inside a gesture of its own while a drag
//! previews, and describes what the canvas should draw over the chart as
//! plain [`Overlay`] data. Tools hold no clock and do no I/O: the same input
//! gives the same document, selection and overlay.

use crate::editor::Editor;
use crate::index::{P, Quad, Rect, SpatialIndex};
use crate::selection::{Selection, View};

mod select;

pub(crate) use select::{SelectTool, select_all, step_in};

#[cfg(test)]
mod tests;

/// Screen-space constants, in CSS pixels and milliseconds, as the buyer
/// renderer's (`04-renderer.md` §14, §15).
pub mod tuning {
    /// Movement after which a mouse press becomes a drag.
    pub const DRAG_MOUSE: f64 = 4.0;
    /// Movement after which a touch or pen press becomes a drag.
    pub const DRAG_TOUCH: f64 = 8.0;
    /// How far from a node a mouse click still hits it.
    pub const HIT_MOUSE: f64 = 4.0;
    /// How far from a node a touch or pen press still hits it.
    pub const HIT_TOUCH: f64 = 12.0;
    /// A second click within this time of the first's release is a double
    /// click…
    pub const DOUBLE_CLICK_MS: f64 = 300.0;
    /// …if it is also within this distance of it.
    pub const DOUBLE_CLICK_PX: f64 = 24.0;
}

/// Held modifier keys. `ctrl` is Cmd on macOS.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Primary,
    Middle,
    Secondary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerKind {
    Mouse,
    Touch,
    Pen,
}

/// One pointer event: where, in chart units and in CSS pixels, at what zoom
/// (CSS pixels per chart unit), with which button and modifiers, and when (in
/// milliseconds, from any fixed start).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pointer {
    pub world: P,
    pub screen: P,
    pub zoom: f64,
    pub button: Button,
    pub kind: PointerKind,
    pub mods: Modifiers,
    pub time: f64,
}

impl Pointer {
    /// Usable input: finite, with a zoom above 0. The session drops others.
    pub(crate) fn valid(&self) -> bool {
        self.world.iter().chain(&self.screen).all(|v| v.is_finite())
            && self.zoom.is_finite()
            && self.zoom > 0.0
            && self.time.is_finite()
    }

    /// The hit tolerance in chart units.
    pub(crate) fn tolerance(&self) -> f64 {
        match self.kind {
            PointerKind::Mouse => tuning::HIT_MOUSE / self.zoom,
            PointerKind::Touch | PointerKind::Pen => tuning::HIT_TOUCH / self.zoom,
        }
    }

    /// Whether the pointer has moved far enough from `down` to drag.
    pub(crate) fn dragged_from(&self, down: &Pointer) -> bool {
        let limit = match down.kind {
            PointerKind::Mouse => tuning::DRAG_MOUSE,
            PointerKind::Touch | PointerKind::Pen => tuning::DRAG_TOUCH,
        };
        let (dx, dy) = (
            self.screen[0] - down.screen[0],
            self.screen[1] - down.screen[1],
        );
        dx.hypot(dy) > limit
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Escape,
    Enter,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    /// A printable key, lower case: `a` for select all with Ctrl.
    Char(char),
}

/// What a tool receives. A pointer up or move without a press, a second
/// press during a drag and a cancel at any time are all expected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Input {
    PointerDown(Pointer),
    PointerMove(Pointer),
    PointerUp(Pointer),
    /// The pointer was lost: the drag ends as if Escape were pressed.
    PointerCancel,
    KeyDown(Key, Modifiers),
    KeyUp(Key, Modifiers),
}

/// The pointer the canvas should show.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Cursor {
    #[default]
    Default,
    /// Over a selected node, or dragging it.
    Move,
    /// Drawing a selection box.
    Crosshair,
}

/// The tools of the tool rail; one is active at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolId {
    Select,
}

/// What the canvas draws over the chart, in chart coordinates; sizes in
/// screen pixels come from [`tuning`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overlay {
    /// One outline per selected item, following its rotation.
    pub outlines: Vec<Quad>,
    /// The box around the whole selection: the selected node's own box for
    /// one node, else the bounds of the outlines.
    pub frame: Option<Quad>,
    /// The selection box being dragged.
    pub marquee: Option<Rect>,
}

/// What a tool works on.
pub(crate) struct Context<'a> {
    pub(crate) editor: &'a mut Editor,
    pub(crate) index: &'a SpatialIndex,
    pub(crate) selection: &'a mut Selection,
    /// The floor shown: only its nodes are picked.
    pub(crate) floor: Option<u32>,
}

impl Context<'_> {
    /// The canvas as a query sees it, with a hit tolerance in chart units,
    /// and the selection to change.
    pub(crate) fn split(&mut self, tol: f64) -> (View<'_>, &mut Selection) {
        let view = View {
            doc: self.editor.document(),
            index: self.index,
            floor: self.floor,
            tol,
        };
        (view, self.selection)
    }
}

/// The interface every tool implements. A new tool is a type implementing
/// it and a [`ToolId`] variant that the session maps to it.
pub(crate) trait Tool: std::fmt::Debug {
    /// Handles one input. Bounded work; never panics, whatever the order.
    fn input(&mut self, cx: &mut Context<'_>, input: &Input);

    /// Abandons what the tool is doing, as Escape would: cancels the gesture
    /// it opened and forgets its pointer. Called before the tool is switched
    /// and before the session applies anything from outside.
    fn reset(&mut self, cx: &mut Context<'_>);

    fn cursor(&self) -> Cursor;

    /// Adds the tool's own overlay (a selection box being dragged).
    fn overlay(&self, out: &mut Overlay);
}
