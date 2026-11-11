//! Compiles a chart source document (JSON) into a compiled chart (CCF):
//! expanding rows and seats, numbering, validation, ranking, chunking and shape
//! tessellation. It runs on the server, in the designer as WASM, and in the CLI.
//! See `docs/03-chart-format.md` §3.
//!
//! The chart document types live here rather than in `cavea-core` so that the
//! buyer's renderer, which links `cavea-core`, never carries serde or JSON code.

mod compile;
mod detmath;
pub mod document;
pub mod expand;
pub mod finding;
mod geometry;
mod migrate;
mod shape;
mod tessellate;

pub use compile::{
    CompileError, Compiled, DEFAULT_PADDING, RANK_ALGO, Stats, compile, write_full_label,
};
pub use document::{ChartDocument, DocumentError};
pub use expand::{ExpandError, ExpandErrorKind, ExpandedRow, Expansion, Seat};
pub use finding::{Code, Detail, Finding, MAX_FINDINGS, Severity};
pub use geometry::auto_outline;
pub use migrate::SCHEMA;
