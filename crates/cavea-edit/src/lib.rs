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

mod command;
mod document;
mod draft;
mod editor;
mod index;
mod op;
mod selection;
mod session;
mod tool;

pub use command::{ChartProperty, Command, CommandError, End, NodeTransform, Property};
pub use document::{Changes, Document, LoadError};
pub use draft::Draft;
pub use editor::{Applied, Editor, MAX_UNDO};
pub use index::{Anchor, AnchorKind, CachedSeat, Quad, Rect, Select, SpatialIndex};
pub use selection::{Level, Selection};
pub use session::Session;
pub use tool::{
    Button, Corner, Cursor, Handle, HandleKind, Input, Key, Modifiers, Overlay, Pointer,
    PointerKind, Readout, Side, TextField, ToolId, tuning,
};

#[cfg(test)]
mod tests;
