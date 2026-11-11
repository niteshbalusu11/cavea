//! The chart document, schema 1 (`docs/03-chart-format.md` §2), as serde types.
//!
//! Field names and values match the JSON exactly. Unknown fields are rejected, so
//! a document that parses is fully represented and saving it loses nothing.
//! Optional fields with a spec default are filled in on parse and always written;
//! other optional fields are written only when present.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// A parse or serialisation failure, with the JSON line and column.
#[derive(Debug)]
pub struct DocumentError(serde_json::Error);

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "chart document: {}", self.0)
    }
}

impl std::error::Error for DocumentError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChartDocument {
    pub schema: u32,
    pub name: String,
    #[serde(default)]
    pub settings: Settings,
    /// Allocation counter for `id`, `rid` and `oid`; never decreases.
    pub next_oid: u32,
    #[serde(default)]
    pub categories: Vec<Category>,
    #[serde(default)]
    pub floors: Vec<Floor>,
    #[serde(default)]
    pub nodes: Vec<Node>,
}

impl ChartDocument {
    /// Parses a document. Nesting deeper than 128 levels is rejected.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DocumentError> {
        serde_json::from_slice(bytes).map_err(DocumentError)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, DocumentError> {
        serde_json::to_vec(self).map_err(DocumentError)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct Settings {
    pub seat_size: f64,
    pub seat_shape: SeatShape,
    pub label_template: String,
    pub table_label_template: String,
    pub row_label_display: RowLabelDisplay,
    pub alpha_overflow: AlphaOverflow,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            seat_size: 44.0,
            seat_shape: SeatShape::Circle,
            label_template: "{section}-{row}-{seat}".into(),
            table_label_template: "{section}-{table}-{seat}".into(),
            row_label_display: RowLabelDisplay::Both,
            alpha_overflow: AlphaOverflow::Double,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SeatShape {
    Circle,
    Rounded,
    Chair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RowLabelDisplay {
    None,
    Start,
    End,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AlphaOverflow {
    Double,
    Spreadsheet,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Category {
    pub key: String,
    pub label: String,
    /// `#RRGGBB`
    pub color: String,
    #[serde(default)]
    pub accessible: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Floor {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub level: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focal_point: Option<[f64; 2]>,
}

/// A node of the scene tree, tagged by `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Node {
    Section(Section),
    RowBlock(RowBlock),
    Row(Row),
    Table(Table),
    Booth(Booth),
    Area(Area),
    Shape(Shape),
    Text(Text),
    Icon(Icon),
    Image(Image),
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn one() -> f64 {
    1.0
}

fn seat_pitch() -> f64 {
    55.0
}

fn row_pitch() -> f64 {
    95.0
}

/// Declares a node struct with the fields every node has (03 §2.2) followed by
/// its own fields.
macro_rules! node {
    ($(#[$meta:meta])* $name:ident { $($body:tt)* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        pub struct $name {
            pub id: u32,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub name: Option<String>,
            /// Floor id. Children of a section inherit the section's floor.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub floor: Option<u32>,
            #[serde(default)]
            pub origin: [f64; 2],
            /// Degrees, clockwise.
            #[serde(default)]
            pub rotation: f64,
            #[serde(default, skip_serializing_if = "is_false")]
            pub locked: bool,
            /// Hidden nodes are not compiled.
            #[serde(default, skip_serializing_if = "is_false")]
            pub hidden: bool,
            $($body)*
        }
    };
}

node!(Section {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub outline: Outline,
    /// Default category for children.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Best-available bias; lower ranks first.
    #[serde(default = "one")]
    pub priority: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrance: Option<String>,
    #[serde(default)]
    pub children: Vec<Node>,
});

/// `"auto"` or `{ "points": [[x, y], ...] }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(untagged)]
pub enum Outline {
    #[default]
    #[serde(with = "auto")]
    Auto,
    Points(Points),
}

/// Serialises the unit variant [`Outline::Auto`] as the string `"auto"`.
mod auto {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str("auto")
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<(), D::Error> {
        let s = String::deserialize(d)?;
        if s == "auto" {
            Ok(())
        } else {
            Err(D::Error::custom("expected \"auto\""))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Points {
    pub points: Vec<[f64; 2]>,
}

node!(RowBlock {
    #[serde(default = "seat_pitch")]
    pub seat_pitch: f64,
    #[serde(default = "row_pitch")]
    pub row_pitch: f64,
    #[serde(default)]
    pub curve: Curve,
    #[serde(default)]
    pub align: Align,
    /// Fraction of a seat pitch by which odd rows are shifted along the row.
    #[serde(default)]
    pub stagger: f64,
    /// Degrees of horizontal shear.
    #[serde(default)]
    pub skew: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default)]
    pub row_labels: RowLabels,
    #[serde(default)]
    pub seat_labels: SeatLabels,
    pub rows: Vec<BlockRow>,
    /// Keyed by `oid`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty", deserialize_with = "oid_keys")]
    pub seat_overrides: BTreeMap<u32, SeatOverride>,
});

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Curve {
    /// An empty struct, not a unit variant, so that extra fields are rejected.
    None {},
    Arc {
        radius: f64,
    },
    Bulge {
        amount: f64,
    },
}

impl Default for Curve {
    fn default() -> Self {
        Self::None {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Align {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlockRow {
    pub rid: u32,
    /// Seat positions, removed ones included.
    pub seats: u32,
    /// Chart units along the row.
    #[serde(default)]
    pub offset: f64,
    /// `[first, count]` runs covering the positions in physical order.
    pub oids: Vec<[u32; 2]>,
    /// Replaces the scheme's label; the row still uses its place in the sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// 0-based seat positions that leave a gap.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<u32>,
}

/// Reads `seatOverrides`, whose keys are oids written as canonical decimal
/// strings. (Inside a tagged node serde buffers the map, which loses
/// serde_json's own key parsing.) A key in another form, or a repeated oid, is
/// an error rather than a silently dropped override.
fn oid_keys<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<u32, SeatOverride>, D::Error> {
    struct Keys;

    impl<'de> serde::de::Visitor<'de> for Keys {
        type Value = BTreeMap<u32, SeatOverride>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a map from oids to seat overrides")
        }

        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> Result<Self::Value, A::Error> {
            use serde::de::Error;
            let mut out = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, SeatOverride>()? {
                let canonical = key.bytes().all(|b| b.is_ascii_digit())
                    && (key == "0" || !key.starts_with('0'));
                let oid: u32 = canonical
                    .then(|| key.parse().ok())
                    .flatten()
                    .ok_or_else(|| {
                        A::Error::custom(format_args!("seat override key {key:?} is not an oid"))
                    })?;
                if out.insert(oid, value).is_some() {
                    return Err(A::Error::custom(format_args!(
                        "seat override for oid {oid} is repeated"
                    )));
                }
            }
            Ok(out)
        }
    }

    d.deserialize_map(Keys)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SeatOverride {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<SeatFlag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nudge: Option<[f64; 2]>,
}

/// Seat flags; the bit numbers are those of the compiled `SFLG` chunk (03 §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SeatFlag {
    Accessible,
    Companion,
    RestrictedView,
    Aisle,
    Removable,
}

impl SeatFlag {
    pub fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// A label start: a number, or a label in the scheme (`"A"`, `"XII"`, `"7"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LabelStart {
    Number(u32),
    Text(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LabelScheme {
    Numeric,
    Odd,
    Even,
    Alpha,
    AlphaLower,
    Roman,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RowDirection {
    FrontToBack,
    BackToFront,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SeatDirection {
    Ltr,
    Rtl,
    CenterOut,
    Serpentine,
    /// Tables only.
    Clockwise,
    /// Tables only.
    Counterclockwise,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct RowLabels {
    pub scheme: LabelScheme,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<LabelStart>,
    pub step: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skip: Vec<String>,
    pub direction: RowDirection,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub suffix: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub custom: Vec<String>,
}

impl Default for RowLabels {
    fn default() -> Self {
        Self {
            scheme: LabelScheme::Alpha,
            start: None,
            step: 1,
            skip: Vec::new(),
            direction: RowDirection::FrontToBack,
            prefix: String::new(),
            suffix: String::new(),
            custom: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct SeatLabels {
    pub scheme: LabelScheme,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<LabelStart>,
    pub step: u32,
    pub direction: SeatDirection,
    #[serde(skip_serializing_if = "is_false")]
    pub continuous: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub suffix: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub custom: Vec<String>,
}

impl Default for SeatLabels {
    fn default() -> Self {
        Self {
            scheme: LabelScheme::Numeric,
            start: None,
            step: 1,
            direction: SeatDirection::Ltr,
            continuous: false,
            prefix: String::new(),
            suffix: String::new(),
            custom: Vec::new(),
        }
    }
}

node!(
    /// One free-form row along a path. Its node `id` is also its row id.
    Row {
    pub path: Path,
    #[serde(default = "seat_pitch")]
    pub seat_pitch: f64,
    pub seats: u32,
    pub oids: Vec<[u32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub seat_labels: SeatLabels,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<u32>,
    /// Keyed by `oid`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty", deserialize_with = "oid_keys")]
    pub seat_overrides: BTreeMap<u32, SeatOverride>,
});

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Path {
    Polyline { points: Vec<[f64; 2]> },
}

node!(Table {
    pub shape: TableShape,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[f64; 2]>,
    pub seats: u32,
    #[serde(default)]
    pub book_as: BookAs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub oids: Vec<[u32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_oid: Option<u32>,
    #[serde(default)]
    pub seat_labels: SeatLabels,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TableShape {
    Round,
    Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum BookAs {
    #[default]
    Seats,
    Table,
}

node!(Booth {
    pub size: [f64; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub oid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
});

node!(Area {
    pub shape: Points,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity: Option<u32>,
    pub oid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
});

node!(Shape {
    pub kind: ShapeKind,
    /// `rect` and `ellipse`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corner_radius: Option<f64>,
    /// `polygon` (closed) and `line` (open).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<Vec<[f64; 2]>>,
    /// `path`: SVG path data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub d: Option<String>,
    #[serde(default)]
    pub style: Style,
    #[serde(default)]
    pub layer: Layer,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShapeKind {
    Rect,
    Ellipse,
    Polygon,
    Line,
    Path,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct Style {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stroke: Option<String>,
    pub stroke_width: f64,
    pub opacity: f64,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            fill: None,
            stroke: None,
            stroke_width: 0.0,
            opacity: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Layer {
    #[default]
    Below,
    Above,
}

node!(Text {
    pub text: String,
    pub size: f64,
    #[serde(default = "text_weight")]
    pub weight: u16,
    pub color: String,
    #[serde(default)]
    pub align: Align,
    #[serde(default)]
    pub min_zoom: f64,
});

fn text_weight() -> u16 {
    400
}

node!(Icon {
    pub icon: IconKind,
    pub size: f64,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IconKind {
    Entrance,
    Exit,
    Stairs,
    Toilets,
    Bar,
    Lift,
}

node!(Image {
    /// Asset hash.
    pub asset: String,
    pub size: [f64; 2],
    #[serde(default = "one")]
    pub opacity: f64,
    pub role: ImageRole,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageRole {
    Reference,
    Background,
}

#[cfg(test)]
mod tests;
