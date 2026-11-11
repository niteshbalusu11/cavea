//! Import and export of the chart document JSON (`docs/07-designer.md`
//! §12): export writes what the server stores as the draft; import reads a
//! file as hostile input, with the compiler's own parser and migrations,
//! and refuses what the editor could not hold within the limits of
//! `02-domain-model.md` §14, each refusal a typed error naming what to fix.

use std::fmt;

use cavea_compiler::document::Node;
use cavea_compiler::{ChartDocument, DocumentError};
use cavea_core::limits::{
    MAX_CATEGORIES, MAX_FLOORS, MAX_OBJECTS, MAX_SEATS_PER_ROW, MAX_SECTIONS,
};
use serde::Serialize;

use crate::command::own_objects;
use crate::document::{Document, LoadError, node_id};

/// The largest document read: the server's draft limit (08 §6.2), so what
/// imports can be saved.
pub const MAX_DOCUMENT: usize = 16 << 20;

/// Shapes, texts and icons per chart, each (02 §14).
const MAX_DECORATION: u32 = 65_535;

/// Why a file cannot be opened in the designer. Nothing was changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "camelCase")]
pub enum ImportError {
    /// Over [`MAX_DOCUMENT`] bytes.
    TooLarge { bytes: u64, limit: u64 },
    /// Not JSON, or not a chart document: the parser's message, which names
    /// the line and column (it may quote the file).
    Json {
        message: String,
        line: u64,
        column: u64,
    },
    /// A schema this designer does not read (E010).
    Schema { schema: u32 },
    /// The migration from an older schema failed (E010).
    Migration { from: u32, problem: String },
    /// A `\u0000` escape, which the server cannot store.
    NulEscape,
    /// Two nodes share an id (E009): commands could not name either.
    DuplicateId { id: u32 },
    /// More of something than a chart may hold (02 §14): `what` is
    /// `objects`, `categories`, `floors`, `sections`, `shapes`, `texts` or
    /// `icons`.
    Limit {
        what: String,
        count: u64,
        limit: u64,
    },
    /// A row (`rid`, or the free-form row's own id) with more positions
    /// than a row may hold.
    SeatsPerRow {
        node: u32,
        row: u32,
        seats: u32,
        limit: u32,
    },
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { bytes, limit } => {
                write!(f, "the file is {bytes} bytes, over the limit of {limit}")
            }
            Self::Json {
                message,
                line,
                column,
            } => write!(
                f,
                "not a chart document at line {line}, column {column}: {message}"
            ),
            Self::Schema { schema } => write!(f, "schema {schema} is not supported"),
            Self::Migration { from, problem } => {
                write!(f, "cannot migrate from schema {from}: {problem}")
            }
            Self::NulEscape => f.write_str("the file holds a \\u0000 escape"),
            Self::DuplicateId { id } => write!(f, "two nodes have id {id}"),
            Self::Limit { what, count, limit } => {
                write!(f, "{count} {what}, over the limit of {limit}")
            }
            Self::SeatsPerRow {
                node,
                row,
                seats,
                limit,
            } => write!(
                f,
                "row {row} of node {node} has {seats} positions, over the limit of {limit}"
            ),
        }
    }
}

impl std::error::Error for ImportError {}

/// The chart document as the server stores the draft: [`Document::save`],
/// whose `nextOid` covers every id this session handed out.
pub(crate) fn export(doc: &Document) -> String {
    let saved = doc.save();
    // A document of finite numbers and strings always serialises.
    serde_json::to_string(&saved).unwrap_or_default()
}

/// Reads `text` as a chart document and opens it, or says why not.
pub(crate) fn import(text: &str) -> Result<Document, ImportError> {
    let bytes = text.as_bytes();
    if bytes.len() > MAX_DOCUMENT {
        return Err(ImportError::TooLarge {
            bytes: bytes.len() as u64,
            limit: MAX_DOCUMENT as u64,
        });
    }
    let document = ChartDocument::from_json(bytes).map_err(|e| match e {
        DocumentError::Json(e) => ImportError::Json {
            message: e.to_string(),
            line: e.line() as u64,
            column: e.column() as u64,
        },
        DocumentError::Schema { schema } => ImportError::Schema { schema },
        DocumentError::Migration { from, problem } => ImportError::Migration {
            from,
            problem: problem.to_owned(),
        },
    })?;
    if has_nul_escape(bytes) {
        return Err(ImportError::NulEscape);
    }
    check(&document)?;
    Document::load(document)
        .map_err(|LoadError::DuplicateNodeId(id)| ImportError::DuplicateId { id })
}

fn limit(what: &str, count: u64, max: u32) -> Result<(), ImportError> {
    if count > u64::from(max) {
        return Err(ImportError::Limit {
            what: what.to_owned(),
            count,
            limit: u64::from(max),
        });
    }
    Ok(())
}

/// The counts of 02 §14 the editor relies on: what its commands refuse to
/// exceed, a file may not exceed either.
fn check(doc: &ChartDocument) -> Result<(), ImportError> {
    limit("categories", doc.categories.len() as u64, MAX_CATEGORIES)?;
    limit("floors", doc.floors.len() as u64, MAX_FLOORS)?;
    let mut counts = [0u64; 5];
    let mut stack: Vec<&Node> = doc.nodes.iter().collect();
    while let Some(node) = stack.pop() {
        counts[0] += own_objects(node);
        let row = |row, seats| {
            if seats > MAX_SEATS_PER_ROW {
                let node = node_id(node);
                return Err(ImportError::SeatsPerRow {
                    node,
                    row,
                    seats,
                    limit: MAX_SEATS_PER_ROW,
                });
            }
            Ok(())
        };
        match node {
            Node::Section(s) => {
                counts[1] += 1;
                stack.extend(&s.children);
            }
            Node::RowBlock(b) => b.rows.iter().try_for_each(|r| row(r.rid, r.seats))?,
            Node::Row(r) => row(r.id, r.seats)?,
            Node::Shape(_) => counts[2] += 1,
            Node::Text(_) => counts[3] += 1,
            Node::Icon(_) => counts[4] += 1,
            Node::Table(_) | Node::Booth(_) | Node::Area(_) | Node::Image(_) => {}
        }
    }
    limit("objects", counts[0], MAX_OBJECTS)?;
    limit("sections", counts[1], MAX_SECTIONS)?;
    limit("shapes", counts[2], MAX_DECORATION)?;
    limit("texts", counts[3], MAX_DECORATION)?;
    limit("icons", counts[4], MAX_DECORATION)
}

/// Whether JSON text holds a `\u0000` escape (as the server checks, 08
/// §6.2). In valid JSON every backslash starts an escape, so reading them in
/// pairs never mistakes `\\u0000` for one.
fn has_nul_escape(bytes: &[u8]) -> bool {
    let mut rest = bytes;
    while let Some(at) = rest.iter().position(|&b| b == b'\\') {
        let escape = rest.get(at + 1..).unwrap_or_default();
        if escape.starts_with(b"u0000") {
            return true;
        }
        rest = escape.get(1..).unwrap_or_default();
    }
    false
}
