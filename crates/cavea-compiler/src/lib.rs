//! Compiles a chart source document (JSON) into a compiled chart (CCF):
//! expanding rows and seats, numbering, validation, ranking, chunking and shape
//! tessellation. It runs on the server, in the designer as WASM, and in the CLI.
//! See `docs/03-chart-format.md` §3.
//!
//! The chart document types live here rather than in `cavea-core` so that the
//! buyer's renderer, which links `cavea-core`, never carries serde or JSON code.

pub mod document;

pub use document::ChartDocument;
