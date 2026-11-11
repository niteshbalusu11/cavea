//! The designer edit core, compiled to WASM: the editor document, commands,
//! undo and redo, and the geometry tools behind the chart designer.
//! See `docs/07-designer.md` §2, §5.2 and §9.
//!
//! - [`Document`]: the chart as an arena of nodes with change tracking.
//! - [`Command`]: the edit vocabulary, applied through an [`Editor`], which
//!   keeps the undo and redo stacks and coalesces gestures.
//! - [`SpatialIndex`]: node bounds for selection and snapping, kept up to date
//!   from the document's change tracking.
//! - [`Session`]: the editor with the index, the [`Selection`] and the active
//!   tool, driven by [`Input`] and drawing an [`Overlay`].

mod arrange;
mod categories;
mod clipboard;
mod command;
mod document;
mod draft;
mod editor;
mod flip;
mod guides;
mod index;
mod numbering;
mod op;
mod rows;
mod section;
mod selection;
mod session;
mod tool;
mod validation;

pub use arrange::{AlignTo, ArrayStep, DUPLICATE_OFFSET, MAX_COPIES, MAX_NEW_NODES, Order};
pub use categories::{CategoriesView, CategoryView, Contrast, contrast, too_similar};
pub use clipboard::MAX_CLIPBOARD;
pub use command::{
    Axis, ChartProperty, Command, CommandError, End, NodeTransform, Property, SeatEdit,
};
pub use document::{Changes, Document, LoadError};
pub use draft::Draft;
pub use editor::{Applied, Editor, MAX_UNDO};
pub use guides::{FloorGuides, GuideAxis, MAX_GUIDES, UserGuide};
pub use index::{Anchor, AnchorKind, CachedSeat, Quad, Rect, Select, SpatialIndex};
pub use numbering::{
    Facing, MAX_LIST, NumberedNode, Numbering, NumberingView, Preview, PreviewRow, PreviewSeat,
    RowLabelsView, SeatLabelsView, Shared,
};
pub use selection::{Level, Selection};
pub use session::Session;
pub use tool::{
    Button, Corner, Cursor, Guide, Handle, HandleKind, Input, Key, Modifiers, Overlay, Pointer,
    PointerKind, Readout, Side, SnapSettings, TextField, ToolId, tuning,
};
pub use validation::{FindingView, Marker, Progress, ValidationView};

#[cfg(test)]
mod edit_tests;
#[cfg(test)]
mod panel_tests;
#[cfg(test)]
mod tests;
