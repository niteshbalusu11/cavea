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

mod block;
mod brush;
mod draw;
pub(crate) mod focal;
mod hand;
pub(crate) mod handles;
mod row;
mod select;
mod shape;
pub(crate) mod snap;
mod text;

pub(crate) use block::{ArcTool, BlockTool};
pub(crate) use brush::BrushTool;
pub(crate) use draw::Spot;
pub(crate) use focal::FocalTool;
pub(crate) use hand::HandTool;
pub use handles::{Corner, Handle, HandleKind, Side};
pub(crate) use row::RowTool;
pub(crate) use select::{SelectTool, select_all, step_in};
pub(crate) use shape::{ShapeKindTool, ShapeTool};
pub use snap::{Guide, SnapSettings};
pub(crate) use text::TextTool;

#[cfg(test)]
pub(crate) mod draw_tests;
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
    /// How far from a handle's centre a mouse press still takes it…
    pub const HANDLE_MOUSE: f64 = 6.0;
    /// …and a touch or pen press.
    pub const HANDLE_TOUCH: f64 = 14.0;
    /// How far the rotate handle sits outside the selection frame.
    pub const ROTATE_OFFSET: f64 = 24.0;
    /// How far the curve handle sits behind the middle of a row block's
    /// front row, clear of the top side's handle.
    pub const CURVE_OFFSET: f64 = 16.0;
    /// How near a snap target must be to pull a position onto it.
    pub const SNAP: f64 = 6.0;
    /// How far around the pointer or the dragged selection snapping looks
    /// for other nodes: about a viewport.
    pub const SNAP_REACH: f64 = 1_000.0;
    /// The most seats a drag may preview in one block (a count, not a
    /// size): it bounds the work of one pointer move. Typed numbers are
    /// limited only by `02-domain-model.md` §14.
    pub const LIVE_SEATS: u32 = 20_000;
}

/// Held modifier keys. `ctrl` is Cmd on macOS.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Button {
    Primary,
    Middle,
    Secondary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PointerKind {
    Mouse,
    Touch,
    Pen,
}

/// One pointer event: where, in chart units and in CSS pixels, at what zoom
/// (CSS pixels per chart unit), with which button and modifiers, and when (in
/// milliseconds, from any fixed start).
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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

/// In JSON `"escape"`, `"arrowLeft"`, …, or `{"char": "a"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Key {
    Escape,
    Enter,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    /// Deletes the last character of a numeric entry; as Delete otherwise.
    Backspace,
    Delete,
    /// A printable key, lower case: `a` for select all with Ctrl.
    Char(char),
}

/// What a tool receives. A pointer up or move without a press, a second
/// press during a drag and a cancel at any time are all expected. In JSON
/// `{"kind": "pointerDown", "data": {…}}`, `{"kind": "pointerCancel"}`,
/// `{"kind": "keyDown", "data": ["escape", {…}]}`.
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "camelCase")]
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Cursor {
    #[default]
    Default,
    /// Over a selected node, or dragging it.
    Move,
    /// Drawing a selection box.
    Crosshair,
    /// The hand tool, ready to pan.
    Grab,
    /// Panning.
    Grabbing,
}

/// The tools of the tool rail; one is active at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolId {
    Select,
    /// Pans the view (07 §4); also while Space is held.
    Hand,
    RowBlock,
    ArcBlock,
    Row,
    Rect,
    Ellipse,
    Polygon,
    Line,
    Text,
    Section,
    /// Category brush (07 §4).
    Brush,
    /// Focal point (07 §4).
    Focal,
}

/// What the canvas draws over the chart, in chart coordinates; sizes in
/// screen pixels come from [`tuning`].
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overlay {
    /// One outline per selected item, following its rotation.
    pub outlines: Vec<Quad>,
    /// The box around the whole selection: the selected node's own box for
    /// one node, else the bounds of the outlines.
    pub frame: Option<Quad>,
    /// The selection box being dragged.
    pub marquee: Option<Rect>,
    /// The select tool's handles on the frame.
    pub handles: Vec<Handle>,
    /// A line through these points: a polygon being clicked, to the pointer.
    pub preview: Vec<P>,
    /// The live readout of the gesture in progress, for the status bar.
    pub readout: Option<Readout>,
    /// The numbers typed so far during a drag (07 §4, numeric entry).
    pub entry: String,
    /// A text field the chrome should show while text is typed.
    pub text: Option<TextField>,
    /// What the last pointer event snapped to (§5.2).
    pub guides: Vec<Guide>,
    /// The outlines of the selected sections, and of the section whose
    /// children are selected, automatic or explicit.
    pub sections: Vec<Vec<P>>,
    /// The focal point of the floor shown.
    pub focal: Option<P>,
    /// The user guides of the floor shown, drawn cyan (§5.2).
    pub user_guides: Vec<crate::guides::UserGuide>,
    /// Nodes on the floor shown with validation findings (§10).
    pub markers: Vec<crate::validation::Marker>,
}

/// The numbers a gesture shows while it runs (07 §4). Lengths in chart
/// units, angles in degrees, clockwise. In JSON `{"kind": "block", "data":
/// {…}}`.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Readout {
    /// A row block: rows, seats in its widest row, and pitches.
    Block {
        rows: u32,
        seats: u32,
        seat_pitch: f64,
        row_pitch: f64,
    },
    /// An arc block: the front row's radius, the angle its seats span, rows,
    /// and seats in the front row.
    Arc {
        radius: f64,
        span: f64,
        rows: u32,
        seats: u32,
    },
    /// A single row: its length along the path and its seats.
    Row {
        length: f64,
        seats: u32,
    },
    Size {
        width: f64,
        height: f64,
    },
    Line {
        length: f64,
        angle: f64,
    },
    /// Corners of a polygon so far.
    Points(u32),
    /// The angle a rotation has turned the selection by.
    Rotate {
        angle: f64,
    },
    /// The curve handle: how far the front row's middle lies behind the
    /// line through its ends, and the arc's radius (none for a bulge or a
    /// straight block).
    Curve {
        depth: f64,
        radius: Option<f64>,
    },
}

/// Text being typed (07 §2.3): the chrome shows a field at `at` (world)
/// holding `text`, and sends each change with `Session::set_text`. `node`
/// is the text node being edited, once it exists. The rest is how the text
/// is drawn, so the field can sit over it: its em size in chart units, its
/// angle in the world (degrees, clockwise), where it lies along its x axis
/// from `at` (03 §2.7: vertically centred), its weight and colour.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextField {
    pub node: Option<u32>,
    pub at: [f64; 2],
    pub text: String,
    pub size: f64,
    pub angle: f64,
    pub align: cavea_compiler::document::Align,
    pub weight: u16,
    pub color: String,
}

/// What a tool works on.
pub(crate) struct Context<'a> {
    pub(crate) editor: &'a mut Editor,
    pub(crate) index: &'a SpatialIndex,
    pub(crate) selection: &'a mut Selection,
    /// The floor shown: only its nodes are picked.
    pub(crate) floor: Option<u32>,
    /// The active category, which new seats take (07 §7).
    pub(crate) category: Option<&'a str>,
    pub(crate) snap: &'a mut snap::Snap,
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

    /// Adds the tool's own overlay (a selection box being dragged, handles,
    /// a readout) to the selection's.
    fn overlay(&self, view: &View<'_>, selection: &Selection, out: &mut Overlay);

    /// Takes the whole string of a text field the tool asked for.
    fn text(&mut self, _cx: &mut Context<'_>, _text: &str) {}

    /// How far the view should pan, in CSS pixels, since last asked.
    fn take_pan(&mut self) -> Option<P> {
        None
    }
}
