//! Validation findings (`docs/03-chart-format.md` §3.2). Each carries the node
//! the designer should select, when there is one.

use std::fmt;

use cavea_core::label::LabelBuf;

use crate::expand::ExpandErrorKind;

/// A finding code of 03 §3.2, or [`Code::Unsupported`] for a node this
/// compiler cannot compile yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Code {
    /// Duplicate object label.
    E001,
    /// Object without a category.
    E002,
    /// Row or seat with an empty label.
    E003,
    /// Area without a capacity.
    E004,
    /// Invalid geometry or value: non-finite or out-of-range number, zero size.
    E005,
    /// A limit of `docs/02-domain-model.md` §14 exceeded.
    E006,
    /// Reference to an unknown category, floor or asset.
    E007,
    /// Duplicate category key.
    E008,
    /// Duplicate or reused `oid`.
    E009,
    /// Unsupported schema or failed migration.
    E010,
    /// A node kind this compiler does not compile yet. Blocks publishing, so a
    /// bookable object is never silently left out of a chart.
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Code {
    pub fn severity(self) -> Severity {
        // Warnings (W101–W110) are a later slice (FMT-014).
        Severity::Error
    }
}

/// What was wrong, in enough detail to explain it.
#[derive(Clone, Debug, PartialEq)]
pub enum Detail {
    /// The document's `schema` is not one this compiler reads.
    Schema {
        schema: u32,
    },
    /// The node's expansion failed.
    Expand(ExpandErrorKind),
    /// Two objects with the same full label; `first` is the node of the
    /// earlier one in `idx` order.
    DuplicateLabel {
        label: LabelBuf,
        first: u32,
    },
    /// A full label longer than 64 bytes.
    LabelTooLong,
    /// An empty row label, or an empty seat label at this position in row `rid`.
    EmptyLabel {
        rid: u32,
        position: Option<u16>,
    },
    /// A seat with no category from its override, node or section.
    NoCategory,
    UnknownCategory {
        key: String,
    },
    UnknownFloor {
        floor: u32,
    },
    /// A child of a section names a floor other than the section's.
    FloorConflict {
        floor: u32,
        section_floor: u32,
    },
    DuplicateFloor {
        floor: u32,
    },
    DuplicateCategory {
        key: String,
    },
    /// An `id`, `rid` or `oid` used twice; `first` is the other node using it.
    DuplicateId {
        id: u32,
        first: u32,
    },
    /// A number that is not finite or out of its range, or text that is not a
    /// valid value (a colour that is not `#RRGGBB`).
    InvalidValue {
        field: &'static str,
    },
    /// A world position that does not fit the compiled format (±2²⁹ cu).
    OutOfRange,
    /// Geometry that cannot be drawn: a self-intersecting outline, a shape of
    /// zero size, path data that does not parse.
    Geometry {
        problem: &'static str,
    },
    /// An image `asset` that is not a content hash (32 lowercase hex digits).
    BadAsset,
    /// More than the limit of this kind.
    TooMany {
        what: &'static str,
        limit: u32,
    },
    /// `table`, `booth`, `area`, or a section inside a section.
    UnsupportedNode {
        kind: &'static str,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub code: Code,
    /// The node to select, or `None` for chart-level findings (settings,
    /// categories, floors).
    pub node: Option<u32>,
    pub detail: Detail,
}

impl Finding {
    pub fn severity(&self) -> Severity {
        self.code.severity()
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.code)?;
        if let Some(node) = self.node {
            write!(f, " node {node}")?;
        }
        f.write_str(": ")?;
        match &self.detail {
            Detail::Schema { schema } => write!(f, "schema {schema} is not supported"),
            Detail::Expand(kind) => write!(f, "{kind:?}"),
            Detail::DuplicateLabel { label, first } => {
                write!(f, "label {label:?} is also used in node {first}")
            }
            Detail::LabelTooLong => f.write_str("full label is longer than 64 bytes"),
            Detail::EmptyLabel { rid, position } => match position {
                Some(p) => write!(f, "row {rid}: seat at position {p} has an empty label"),
                None => write!(f, "row {rid} has an empty label"),
            },
            Detail::NoCategory => f.write_str("seats without a category"),
            Detail::UnknownCategory { key } => write!(f, "unknown category {key:?}"),
            Detail::UnknownFloor { floor } => write!(f, "unknown floor {floor}"),
            Detail::FloorConflict {
                floor,
                section_floor,
            } => write!(
                f,
                "floor {floor} differs from its section's {section_floor}"
            ),
            Detail::DuplicateFloor { floor } => write!(f, "floor id {floor} is repeated"),
            Detail::DuplicateCategory { key } => write!(f, "category key {key:?} is repeated"),
            Detail::DuplicateId { id, first } => write!(f, "id {id} is also used in node {first}"),
            Detail::InvalidValue { field } => write!(f, "invalid {field}"),
            Detail::OutOfRange => f.write_str("position outside ±2^29 chart units"),
            Detail::Geometry { problem } => f.write_str(problem),
            Detail::BadAsset => f.write_str("asset is not a 32-digit lowercase hex hash"),
            Detail::TooMany { what, limit } => write!(f, "more than {limit} {what}"),
            Detail::UnsupportedNode { kind } => write!(f, "{kind} nodes are not compiled yet"),
        }
    }
}

/// Findings kept per compile. Beyond this the list stops growing, so a
/// hostile document cannot make the report itself unbounded.
pub const MAX_FINDINGS: usize = 1_000;

#[derive(Debug, Default)]
pub(crate) struct Findings(pub Vec<Finding>);

impl Findings {
    pub fn push(&mut self, code: Code, node: Option<u32>, detail: Detail) {
        if self.0.len() < MAX_FINDINGS {
            self.0.push(Finding { code, node, detail });
        }
    }

    pub fn has_errors(&self) -> bool {
        self.0.iter().any(|f| f.severity() == Severity::Error)
    }
}

/// The code for an expansion failure.
pub(crate) fn expand_code(kind: &ExpandErrorKind) -> Code {
    use cavea_core::label::LabelError;
    match kind {
        ExpandErrorKind::TooManySeatsInRow { .. } | ExpandErrorKind::TooManyObjects => Code::E006,
        ExpandErrorKind::OidRuns { .. } => Code::E009,
        ExpandErrorKind::Label { error, .. } => match error {
            LabelError::TooLong => Code::E006,
            LabelError::CustomExhausted => Code::E003,
            _ => Code::E005,
        },
        _ => Code::E005,
    }
}
