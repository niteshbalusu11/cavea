//! The designer edit core, compiled to WASM: the editor document, commands,
//! undo and redo, and the geometry tools behind the chart designer.
//! See `docs/07-designer.md` §2, §5.2 and §9.
//!
//! - [`Document`]: the chart as an arena of nodes with change tracking.
//! - [`Command`]: the edit vocabulary, applied through an [`Editor`], which
//!   keeps the undo and redo stacks and coalesces gestures.
//! - [`SpatialIndex`]: node bounds for selection and snapping, kept up to date
//!   from the document's change tracking.

mod command;
mod document;
mod editor;
mod index;
mod op;

pub use command::{ChartProperty, Command, CommandError, End, NodeTransform, Property};
pub use document::{Changes, Document, LoadError};
pub use editor::{Applied, Editor, MAX_UNDO};
pub use index::{Anchor, AnchorKind, CachedSeat, Rect, Select, SpatialIndex};

#[cfg(test)]
mod tests;
