//! Validation findings (`docs/03-chart-format.md` §3.2). Each carries the node
//! the designer should select, when there is one.

use std::fmt;

use cavea_core::label::LabelBuf;
use serde::ser::{Serialize, SerializeStruct, Serializer};

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
    /// Area without a capacity. Cannot occur while areas are [`Code::Unsupported`].
    E004,
    /// Invalid geometry or value: non-finite or out-of-range number, zero size.
    E005,
    /// A limit of `docs/02-domain-model.md` §14 exceeded.
    E006,
    /// Reference to an unknown category, floor, icon or asset.
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
    /// Overlapping seats.
    W101,
    /// Floor without a focal point.
    W102,
    /// Row with a single seat.
    W103,
    /// Seat outside its section's explicit outline.
    W104,
    /// Unused category.
    W105,
    /// No seat flagged accessible.
    W106,
    /// Category colours too similar, or low contrast against a background.
    W107,
    /// Text too small to read at fit zoom.
    W108,
    /// Section without a label.
    W109,
    /// Chart extent above 200,000 cu.
    W110,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

impl Code {
    pub fn severity(self) -> Severity {
        if self >= Self::W101 {
            Severity::Warning
        } else {
            Severity::Error
        }
    }

    /// The code as 03 §3.2 writes it (`"E005"`), or `"unsupported"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::E001 => "E001",
            Self::E002 => "E002",
            Self::E003 => "E003",
            Self::E004 => "E004",
            Self::E005 => "E005",
            Self::E006 => "E006",
            Self::E007 => "E007",
            Self::E008 => "E008",
            Self::E009 => "E009",
            Self::E010 => "E010",
            Self::Unsupported => "unsupported",
            Self::W101 => "W101",
            Self::W102 => "W102",
            Self::W103 => "W103",
            Self::W104 => "W104",
            Self::W105 => "W105",
            Self::W106 => "W106",
            Self::W107 => "W107",
            Self::W108 => "W108",
            Self::W109 => "W109",
            Self::W110 => "W110",
        }
    }
}

/// What was wrong, in enough detail to explain it.
#[derive(Clone, Debug, PartialEq)]
pub enum Detail {
    /// The document's `schema` is not one this compiler reads.
    Schema {
        schema: u32,
    },
    /// Migrating from schema `from` to the next failed.
    Migration {
        from: u32,
        problem: &'static str,
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
    /// An icon key that is not one of the six of 03 §2.2.
    UnknownIcon {
        key: String,
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
    /// A category colour that is not `#RRGGBB`.
    CategoryColor {
        key: String,
    },
    /// A floor's focal point outside the quantised range.
    FocalPoint {
        floor: u32,
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
    /// A text node whose text is empty.
    EmptyText,
    /// A world position that does not fit the compiled format (±2²⁹ cu).
    OutOfRange,
    /// Geometry that cannot be drawn: a self-intersecting or flat outline.
    Geometry {
        problem: &'static str,
    },
    /// More than the limit of this kind.
    TooMany {
        what: &'static str,
        limit: u32,
    },
    /// `table`, `booth`, `area`, or a section inside a section.
    UnsupportedNode {
        kind: &'static str,
    },
    /// W101: a seat of this node is within 0.6 seat sizes of a seat of `other`
    /// (which may be the same node).
    Overlap {
        other: u32,
    },
    /// W102: `floor` is `None` for the implicit floor of a chart without floors.
    NoFocalPoint {
        floor: Option<u32>,
    },
    /// W103: the first row of the node with one seat.
    SingleSeat {
        rid: u32,
    },
    /// W104: the node's first seat outside the outline of section `section`.
    OutsideOutline {
        section: u32,
        rid: u32,
        position: u16,
    },
    /// W105.
    UnusedCategory {
        key: String,
    },
    /// W106.
    NoAccessibleSeat,
    /// W107: two categories' colours too similar.
    SimilarColors {
        key: String,
        other: String,
    },
    /// W107: contrast against a built-in theme's background, in hundredths.
    LowContrast {
        key: String,
        background: &'static str,
        ratio: u32,
    },
    /// W108: the text's size at fit zoom, in hundredths of a CSS pixel.
    TextTooSmall {
        px: u32,
    },
    /// W109.
    NoSectionLabel,
    /// W110: the chart's larger extent, in chart units.
    ChartTooLarge {
        extent: u32,
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

/// Document text quoted in a message: at most 64 bytes, so a hostile
/// document cannot make messages long.
struct Quote<'a>(&'a str);

impl fmt::Display for Quote<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.0;
        let mut end = s.len().min(64);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        let cut = if end < s.len() { "…" } else { "" };
        write!(f, "{:?}{cut}", s.get(..end).unwrap_or_default())
    }
}

/// A ratio in hundredths, as `4.5`.
struct Hundredths(u32);

impl fmt::Display for Hundredths {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:02}", self.0 / 100, self.0 % 100)
    }
}

impl Finding {
    pub fn severity(&self) -> Severity {
        self.code.severity()
    }

    /// The other node a finding involves (the first of a duplicate, the
    /// overlapping node, the section whose outline a seat is outside).
    pub fn related(&self) -> Option<u32> {
        match self.detail {
            Detail::DuplicateLabel { first, .. } | Detail::DuplicateId { first, .. } => Some(first),
            Detail::Overlap { other } => Some(other),
            Detail::OutsideOutline { section, .. } => Some(section),
            _ => None,
        }
    }

    /// The message without code or node: short, and quoting document text
    /// only for labels and keys, at most 64 bytes of each.
    pub fn message(&self) -> String {
        Message(&self.detail).to_string()
    }
}

struct Message<'a>(&'a Detail);

impl fmt::Display for Message<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Detail::Schema { schema } => write!(
                f,
                "schema {schema} is not supported; this compiler reads 1 to {}",
                crate::SCHEMA
            ),
            Detail::Migration { from, problem } => {
                write!(f, "migration from schema {from} failed: {problem}")
            }
            Detail::Expand(kind) => kind.fmt(f),
            Detail::DuplicateLabel { label, first } => write!(
                f,
                "label {} is also used in node {first}",
                Quote(label.as_str())
            ),
            Detail::LabelTooLong => f.write_str("full label is longer than 64 bytes"),
            Detail::EmptyLabel { rid, position } => match position {
                Some(p) => write!(f, "row {rid}: seat at position {p} has an empty label"),
                None => write!(f, "row {rid} has an empty label"),
            },
            Detail::NoCategory => f.write_str("seats without a category"),
            Detail::UnknownCategory { key } => write!(f, "unknown category {}", Quote(key)),
            Detail::UnknownFloor { floor } => write!(f, "unknown floor {floor}"),
            Detail::UnknownIcon { key } => write!(f, "unknown icon {}", Quote(key)),
            Detail::FloorConflict {
                floor,
                section_floor,
            } => write!(
                f,
                "floor {floor} differs from its section's {section_floor}"
            ),
            Detail::DuplicateFloor { floor } => write!(f, "floor id {floor} is repeated"),
            Detail::DuplicateCategory { key } => {
                write!(f, "category key {} is repeated", Quote(key))
            }
            Detail::CategoryColor { key } => {
                write!(f, "category {}: colour is not #RRGGBB", Quote(key))
            }
            Detail::FocalPoint { floor } => {
                write!(f, "floor {floor}: focal point is out of range")
            }
            Detail::DuplicateId { id, first } => write!(f, "id {id} is also used in node {first}"),
            Detail::InvalidValue { field } => write!(f, "invalid {field}"),
            Detail::EmptyText => f.write_str("text is empty"),
            Detail::OutOfRange => f.write_str("position outside ±2^29 chart units"),
            Detail::Geometry { problem } => f.write_str(problem),
            Detail::TooMany { what, limit } => write!(f, "more than {limit} {what}"),
            Detail::UnsupportedNode { kind } => write!(f, "{kind} nodes are not compiled yet"),
            Detail::Overlap { other } => {
                write!(
                    f,
                    "seats closer than 0.6 seat sizes to seats of node {other}"
                )
            }
            Detail::NoFocalPoint { floor } => match floor {
                Some(id) => write!(f, "floor {id} has no focal point"),
                None => f.write_str("the chart has no floors, so no focal point"),
            },
            Detail::SingleSeat { rid } => write!(f, "row {rid} has a single seat"),
            Detail::OutsideOutline {
                section,
                rid,
                position,
            } => write!(
                f,
                "row {rid}: seat at position {position} is outside the outline of section {section}"
            ),
            Detail::UnusedCategory { key } => {
                write!(f, "category {} is not used by any seat", Quote(key))
            }
            Detail::NoAccessibleSeat => f.write_str("no seat is flagged accessible"),
            Detail::SimilarColors { key, other } => write!(
                f,
                "categories {} and {} have colours too similar to tell apart",
                Quote(other),
                Quote(key)
            ),
            Detail::LowContrast {
                key,
                background,
                ratio,
            } => write!(
                f,
                "category {}: contrast {}:1 against the {background} background, below 3:1",
                Quote(key),
                Hundredths(*ratio)
            ),
            Detail::TextTooSmall { px } => write!(
                f,
                "text is {} px at fit zoom on a 1280 × 720 viewport, below 10 px",
                Hundredths(*px)
            ),
            Detail::NoSectionLabel => f.write_str("section has no label"),
            Detail::ChartTooLarge { extent } => {
                write!(f, "chart extends {extent} cu, above 200000 cu")
            }
        }
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code.as_str())?;
        if let Some(node) = self.node {
            write!(f, " node {node}")?;
        }
        write!(f, ": {}", Message(&self.detail))
    }
}

/// `{"code", "severity", "node", "related", "message"}` (03 §3.2).
impl Serialize for Finding {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut out = s.serialize_struct("Finding", 5)?;
        out.serialize_field("code", self.code.as_str())?;
        out.serialize_field("severity", self.severity().as_str())?;
        out.serialize_field("node", &self.node)?;
        out.serialize_field("related", &self.related())?;
        out.serialize_field("message", &self.message())?;
        out.end()
    }
}

/// Findings kept per compile. Beyond this the list stops growing, so a
/// hostile document cannot make the report itself unbounded. Warnings are
/// checked only when there is no error, so they never crowd one out.
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
