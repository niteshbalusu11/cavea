//! The inspector (`docs/07-designer.md` §3, §5.3, §5.5): the selected nodes'
//! fields, each the same across them or mixed, with its limits; the chart's
//! settings when nothing is selected; and one edit of one field on every
//! selected node, built from existing commands.
//!
//! The inspector acts on every selected node, locked and hidden ones
//! included: the selection is explicit (the layers panel can select them),
//! and typing a value is deliberate, unlike a drag on the canvas.

use cavea_compiler::document::{
    Align, AlphaOverflow, Curve, ImageRole, Layer, Node, Outline, RowBlock, RowLabelDisplay,
    SeatShape, ShapeKind,
};
use cavea_core::limits::{MAX_LABEL_LEN, MAX_OBJECTS, MAX_SEATS_PER_ROW};
use serde::{Deserialize, Serialize};

use crate::categories::rgb;
use crate::command::{ChartProperty, Command, CommandError, End, NodeTransform, Property};
use crate::document::{Document, each_node};
use crate::draft::normalise;
use crate::numbering::{Shared, shared};
use crate::selection::{Level, Selection};
use crate::tool::handles::arc_ok;

/// Bytes of a node name, a section's entrance, a label template (02 §14
/// sets the text object's 256 for all free text).
const MAX_TEXT: usize = 256;

/// The unit a number is shown in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Unit {
    /// Chart units (03 §1).
    Cu,
    Degrees,
    Count,
    /// 0 to 1 (opacity, stagger as a fraction of the seat pitch).
    Fraction,
    /// CSS pixels per chart unit (a text's `minZoom`).
    Zoom,
    /// Unitless (a section's priority, a text's weight).
    Number,
}

/// A number field: its value across the selection and what it accepts,
/// `min` to `max` inclusive.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Num {
    pub value: Shared<f64>,
    pub min: f64,
    pub max: f64,
    pub unit: Unit,
}

/// What the inspector shows: the chart when nothing is selected at a node
/// level, else the selected nodes' fields. A group is present when every
/// selected node has it (`rowBlock` when all are row blocks); `common`
/// fields any node has, `category` and `label` when all have them.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectorView {
    pub level: Level,
    pub chart: Option<ChartFields>,
    pub common: Option<CommonFields>,
    pub row_block: Option<RowBlockFields>,
    pub row: Option<RowFields>,
    pub section: Option<SectionFields>,
    pub shape: Option<ShapeFields>,
    pub text: Option<TextFields>,
    pub image: Option<ImageFields>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartFields {
    pub name: String,
    pub seat_size: Num,
    pub seat_shape: SeatShape,
    pub label_template: String,
    pub table_label_template: String,
    pub row_label_display: RowLabelDisplay,
    pub alpha_overflow: AlphaOverflow,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommonFields {
    pub name: Shared<Option<String>>,
    /// The origin in the parent's space, and the rotation (03 §2.2).
    pub x: Num,
    pub y: Num,
    pub rotation: Num,
    pub locked: Shared<bool>,
    pub hidden: Shared<bool>,
    /// Top-level nodes only: children are on their section's floor.
    pub floor: Option<Shared<Option<u32>>>,
    pub category: Option<Shared<Option<String>>>,
    pub label: Option<Shared<Option<String>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowBlockFields {
    pub rows: Num,
    /// Mixed also when one block's rows differ.
    pub seats: Num,
    pub seat_pitch: Num,
    pub row_pitch: Num,
    pub curve: Shared<Curve>,
    pub align: Shared<Align>,
    pub stagger: Num,
    pub skew: Num,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowFields {
    pub seat_pitch: Num,
    /// Seat positions; set by the row's path (read only).
    pub seats: Shared<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionFields {
    pub priority: Num,
    pub entrance: Shared<Option<String>>,
    /// Automatic (true) or explicit points; `Session::convert_outline`
    /// makes it explicit.
    pub auto_outline: Shared<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeFields {
    pub kind: Shared<ShapeKind>,
    /// Rectangles and ellipses.
    pub width: Option<Num>,
    pub height: Option<Num>,
    /// Rectangles.
    pub corner_radius: Option<Num>,
    /// Not for lines, which are never filled.
    pub fill: Option<Shared<Option<String>>>,
    pub stroke: Shared<Option<String>>,
    pub stroke_width: Num,
    pub opacity: Num,
    pub layer: Shared<Layer>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextFields {
    pub text: Shared<String>,
    pub size: Num,
    pub weight: Num,
    pub color: Shared<String>,
    pub align: Shared<Align>,
    pub min_zoom: Num,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageFields {
    pub width: Num,
    pub height: Num,
    pub opacity: Num,
    pub role: Shared<ImageRole>,
    /// The picture's content hash (read only: the shell shows whether it
    /// is loaded and uploaded).
    pub asset: Shared<String>,
}

/// One inspector field with its new value, for every selected node (or
/// the chart): `{"field": "rowPitch", "value": 95}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "field", content = "value", rename_all = "camelCase")]
pub enum Field {
    Name(Option<String>),
    X(f64),
    Y(f64),
    Rotation(f64),
    Locked(bool),
    Hidden(bool),
    Floor(Option<u32>),
    Category(Option<String>),
    Label(Option<String>),
    /// Row blocks: rows are added or removed at the back.
    Rows(u32),
    /// Row blocks: every row gets this many positions, added or removed at
    /// its end.
    Seats(u32),
    SeatPitch(f64),
    RowPitch(f64),
    Curve(Curve),
    Align(Align),
    Stagger(f64),
    Skew(f64),
    Priority(f64),
    Entrance(Option<String>),
    /// Rectangles, ellipses and images.
    Width(f64),
    Height(f64),
    CornerRadius(f64),
    Fill(Option<String>),
    Stroke(Option<String>),
    StrokeWidth(f64),
    /// Shapes and images.
    Opacity(f64),
    Layer(Layer),
    Text(String),
    TextSize(f64),
    Weight(u16),
    Color(String),
    TextAlign(Align),
    MinZoom(f64),
    ImageRole(ImageRole),
    ChartName(String),
    SeatSize(f64),
    SeatShape(SeatShape),
    LabelTemplate(String),
    TableLabelTemplate(String),
    RowLabelDisplay(RowLabelDisplay),
    AlphaOverflow(AlphaOverflow),
}

/// Each number field's range and unit.
fn limits(field: &'static str) -> (f64, f64, Unit) {
    match field {
        "x" | "y" => (-1e7, 1e7, Unit::Cu),
        "rotation" => (-180.0, 180.0, Unit::Degrees),
        "rows" => (1.0, f64::from(MAX_OBJECTS), Unit::Count),
        "seats" => (1.0, f64::from(MAX_SEATS_PER_ROW), Unit::Count),
        "seatPitch" | "rowPitch" => (1.0, 10_000.0, Unit::Cu),
        "stagger" => (0.0, 1.0, Unit::Fraction),
        "skew" => (-89.0, 89.0, Unit::Degrees),
        "priority" => (0.001, 1e6, Unit::Number),
        "width" | "height" => (1.0, 1e6, Unit::Cu),
        "cornerRadius" | "strokeWidth" => (0.0, 1e5, Unit::Cu),
        "opacity" => (0.0, 1.0, Unit::Fraction),
        // 03 §2.7: quantised to quarters below 16,384.
        "textSize" | "seatSize" => (0.25, 16_383.75, Unit::Cu),
        "weight" => (1.0, 1_000.0, Unit::Number),
        "minZoom" => (0.0, 1_000.0, Unit::Zoom),
        _ => (f64::NEG_INFINITY, f64::INFINITY, Unit::Number),
    }
}

fn num(field: &'static str, value: Shared<f64>) -> Num {
    let (min, max, unit) = limits(field);
    Num {
        value,
        min,
        max,
        unit,
    }
}

/// A number field over `nodes`, if each has it.
fn over<'a, T>(nodes: &'a [&'a Node], get: impl Fn(&'a Node) -> Option<T>) -> Option<Vec<T>> {
    nodes.iter().map(|n| get(n)).collect()
}

fn same<T: PartialEq>(values: Vec<T>) -> Option<Shared<T>> {
    shared(values.into_iter())
}

fn shared_num(field: &'static str, values: Vec<f64>) -> Option<Num> {
    same(values).map(|v| num(field, v))
}

fn category_of(node: &Node) -> Option<Option<String>> {
    crate::command::category(node).cloned()
}

fn label_of(node: &Node) -> Option<Option<String>> {
    Some(
        match node {
            Node::Section(n) => &n.label,
            Node::Row(n) => &n.label,
            Node::Table(n) => &n.label,
            Node::Booth(n) => &n.label,
            Node::Area(n) => &n.label,
            _ => return None,
        }
        .clone(),
    )
}

/// The nodes the inspector edits: the selection at a node level.
pub(crate) fn targets(sel: &Selection) -> Vec<u32> {
    match sel.level() {
        Level::Top | Level::Section(_) => sel.items().iter().copied().collect(),
        Level::Rows(_) | Level::Seats(_) => Vec::new(),
    }
}

pub(crate) fn view(doc: &Document, sel: &Selection) -> InspectorView {
    let ids = targets(sel);
    let nodes: Vec<&Node> = ids.iter().filter_map(|&id| doc.node(id)).collect();
    let mut out = InspectorView {
        level: sel.level(),
        chart: None,
        common: None,
        row_block: None,
        row: None,
        section: None,
        shape: None,
        text: None,
        image: None,
    };
    if nodes.is_empty() {
        if matches!(sel.level(), Level::Top | Level::Section(_)) {
            out.chart = Some(chart(doc));
        }
        return out;
    }
    let list = |f: &dyn Fn(&Node) -> f64| nodes.iter().map(|n| f(n)).collect::<Vec<_>>();
    out.common = Some(CommonFields {
        name: shared(nodes.iter().map(|n| each_node!(*n, x => x.name.clone())))
            .unwrap_or(Shared::Mixed),
        x: num(
            "x",
            same(list(&|n| each_node!(n, x => x.origin[0]))).unwrap_or(Shared::Mixed),
        ),
        y: num(
            "y",
            same(list(&|n| each_node!(n, x => x.origin[1]))).unwrap_or(Shared::Mixed),
        ),
        rotation: num(
            "rotation",
            same(list(&|n| each_node!(n, x => x.rotation))).unwrap_or(Shared::Mixed),
        ),
        locked: shared(nodes.iter().map(|n| each_node!(*n, x => x.locked)))
            .unwrap_or(Shared::Mixed),
        hidden: shared(nodes.iter().map(|n| each_node!(*n, x => x.hidden)))
            .unwrap_or(Shared::Mixed),
        floor: ids
            .iter()
            .all(|&id| doc.parent(id).is_none())
            .then(|| shared(nodes.iter().map(|n| each_node!(*n, x => x.floor))))
            .flatten(),
        category: over(&nodes, category_of).and_then(same),
        label: over(&nodes, label_of).and_then(same),
    });
    let blocks = over(&nodes, |n| match n {
        Node::RowBlock(b) => Some(b),
        _ => None,
    });
    if let Some(b) = blocks {
        let seats = |b: &RowBlock| {
            let first = b.rows.first().map_or(0, |r| r.seats);
            b.rows.iter().all(|r| r.seats == first).then_some(first)
        };
        let seats = over(&nodes, |n| match n {
            Node::RowBlock(b) => Some(seats(b)),
            _ => None,
        })
        .and_then(same)
        .and_then(|s| match s {
            Shared::Same(Some(n)) => Some(Shared::Same(f64::from(n))),
            _ => None,
        })
        .unwrap_or(Shared::Mixed);
        let n = |f: fn(&RowBlock) -> f64, field| {
            shared_num(field, b.iter().map(|b| f(b)).collect()).unwrap_or(num(field, Shared::Mixed))
        };
        out.row_block = Some(RowBlockFields {
            rows: n(|b| b.rows.len() as f64, "rows"),
            seats: num("seats", seats),
            seat_pitch: n(|b| b.seat_pitch, "seatPitch"),
            row_pitch: n(|b| b.row_pitch, "rowPitch"),
            curve: same(b.iter().map(|b| b.curve).collect()).unwrap_or(Shared::Mixed),
            align: same(b.iter().map(|b| b.align).collect()).unwrap_or(Shared::Mixed),
            stagger: n(|b| b.stagger, "stagger"),
            skew: n(|b| b.skew, "skew"),
        });
    }
    let rows = over(&nodes, |n| match n {
        Node::Row(r) => Some(r),
        _ => None,
    });
    if let Some(r) = rows {
        out.row = Some(RowFields {
            seat_pitch: shared_num("seatPitch", r.iter().map(|r| r.seat_pitch).collect())
                .unwrap_or(num("seatPitch", Shared::Mixed)),
            seats: same(r.iter().map(|r| r.seats).collect()).unwrap_or(Shared::Mixed),
        });
    }
    let sections = over(&nodes, |n| match n {
        Node::Section(s) => Some(s),
        _ => None,
    });
    if let Some(s) = sections {
        out.section = Some(SectionFields {
            priority: shared_num("priority", s.iter().map(|s| s.priority).collect())
                .unwrap_or(num("priority", Shared::Mixed)),
            entrance: same(s.iter().map(|s| s.entrance.clone()).collect()).unwrap_or(Shared::Mixed),
            auto_outline: same(s.iter().map(|s| s.outline == Outline::Auto).collect())
                .unwrap_or(Shared::Mixed),
        });
    }
    let shapes = over(&nodes, |n| match n {
        Node::Shape(s) => Some(s),
        _ => None,
    });
    if let Some(s) = shapes {
        let sized = s
            .iter()
            .all(|s| matches!(s.kind, ShapeKind::Rect | ShapeKind::Ellipse));
        let size = |i: usize, field| {
            sized.then(|| {
                let v = s.iter().map(|s| s.size.map_or(0.0, |z| z[i])).collect();
                shared_num(field, v).unwrap_or(num(field, Shared::Mixed))
            })
        };
        let rects = s.iter().all(|s| s.kind == ShapeKind::Rect);
        let filled = s.iter().all(|s| s.kind != ShapeKind::Line);
        out.shape = Some(ShapeFields {
            kind: same(s.iter().map(|s| s.kind).collect()).unwrap_or(Shared::Mixed),
            width: size(0, "width"),
            height: size(1, "height"),
            corner_radius: rects.then(|| {
                let v = s.iter().map(|s| s.corner_radius.unwrap_or(0.0)).collect();
                shared_num("cornerRadius", v).unwrap_or(num("cornerRadius", Shared::Mixed))
            }),
            fill: filled
                .then(|| same(s.iter().map(|s| s.style.fill.clone()).collect()))
                .flatten(),
            stroke: same(s.iter().map(|s| s.style.stroke.clone()).collect())
                .unwrap_or(Shared::Mixed),
            stroke_width: shared_num(
                "strokeWidth",
                s.iter().map(|s| s.style.stroke_width).collect(),
            )
            .unwrap_or(num("strokeWidth", Shared::Mixed)),
            opacity: shared_num("opacity", s.iter().map(|s| s.style.opacity).collect())
                .unwrap_or(num("opacity", Shared::Mixed)),
            layer: same(s.iter().map(|s| s.layer).collect()).unwrap_or(Shared::Mixed),
        });
    }
    let texts = over(&nodes, |n| match n {
        Node::Text(t) => Some(t),
        _ => None,
    });
    if let Some(t) = texts {
        let n = |v: Vec<f64>, field| shared_num(field, v).unwrap_or(num(field, Shared::Mixed));
        out.text = Some(TextFields {
            text: same(t.iter().map(|t| t.text.clone()).collect()).unwrap_or(Shared::Mixed),
            size: n(t.iter().map(|t| t.size).collect(), "textSize"),
            weight: n(t.iter().map(|t| f64::from(t.weight)).collect(), "weight"),
            color: same(t.iter().map(|t| t.color.clone()).collect()).unwrap_or(Shared::Mixed),
            align: same(t.iter().map(|t| t.align).collect()).unwrap_or(Shared::Mixed),
            min_zoom: n(t.iter().map(|t| t.min_zoom).collect(), "minZoom"),
        });
    }
    let images = over(&nodes, |n| match n {
        Node::Image(i) => Some(i),
        _ => None,
    });
    if let Some(i) = images {
        let n = |v: Vec<f64>, field| shared_num(field, v).unwrap_or(num(field, Shared::Mixed));
        out.image = Some(ImageFields {
            width: n(i.iter().map(|i| i.size[0]).collect(), "width"),
            height: n(i.iter().map(|i| i.size[1]).collect(), "height"),
            opacity: n(i.iter().map(|i| i.opacity).collect(), "opacity"),
            role: same(i.iter().map(|i| i.role).collect()).unwrap_or(Shared::Mixed),
            asset: same(i.iter().map(|i| i.asset.clone()).collect()).unwrap_or(Shared::Mixed),
        });
    }
    out
}

fn chart(doc: &Document) -> ChartFields {
    let s = doc.settings();
    ChartFields {
        name: doc.name().to_owned(),
        seat_size: num("seatSize", Shared::Same(s.seat_size)),
        seat_shape: s.seat_shape,
        label_template: s.label_template.clone(),
        table_label_template: s.table_label_template.clone(),
        row_label_display: s.row_label_display,
        alpha_overflow: s.alpha_overflow,
    }
}

impl Field {
    /// The field's name in JSON, which is also the undo entry's key.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Name(_) => "name",
            Self::X(_) => "x",
            Self::Y(_) => "y",
            Self::Rotation(_) => "rotation",
            Self::Locked(_) => "locked",
            Self::Hidden(_) => "hidden",
            Self::Floor(_) => "floor",
            Self::Category(_) => "category",
            Self::Label(_) => "label",
            Self::Rows(_) => "rows",
            Self::Seats(_) => "seats",
            Self::SeatPitch(_) => "seatPitch",
            Self::RowPitch(_) => "rowPitch",
            Self::Curve(_) => "curve",
            Self::Align(_) => "align",
            Self::Stagger(_) => "stagger",
            Self::Skew(_) => "skew",
            Self::Priority(_) => "priority",
            Self::Entrance(_) => "entrance",
            Self::Width(_) => "width",
            Self::Height(_) => "height",
            Self::CornerRadius(_) => "cornerRadius",
            Self::Fill(_) => "fill",
            Self::Stroke(_) => "stroke",
            Self::StrokeWidth(_) => "strokeWidth",
            Self::Opacity(_) => "opacity",
            Self::Layer(_) => "layer",
            Self::Text(_) => "text",
            Self::TextSize(_) => "textSize",
            Self::Weight(_) => "weight",
            Self::Color(_) => "color",
            Self::TextAlign(_) => "textAlign",
            Self::MinZoom(_) => "minZoom",
            Self::ImageRole(_) => "imageRole",
            Self::ChartName(_) => "chartName",
            Self::SeatSize(_) => "seatSize",
            Self::SeatShape(_) => "seatShape",
            Self::LabelTemplate(_) => "labelTemplate",
            Self::TableLabelTemplate(_) => "tableLabelTemplate",
            Self::RowLabelDisplay(_) => "rowLabelDisplay",
            Self::AlphaOverflow(_) => "alphaOverflow",
        }
    }

    /// Refuses a value outside the field's limits: a number outside its
    /// range or not finite, a text too long, a colour not `#RRGGBB`.
    pub(crate) fn check(&self) -> Result<(), CommandError> {
        let key = self.key();
        let bad = || CommandError::InvalidValue(key);
        let (min, max, _) = limits(key);
        let range = |v: f64| {
            (v.is_finite() && (min..=max).contains(&v))
                .then_some(())
                .ok_or_else(bad)
        };
        let text = |s: &str, n: usize| (s.len() <= n).then_some(()).ok_or_else(bad);
        let colour = |c: &str| rgb(c).map(drop).ok_or_else(bad);
        match self {
            Self::X(v)
            | Self::Y(v)
            | Self::SeatPitch(v)
            | Self::RowPitch(v)
            | Self::Stagger(v)
            | Self::Skew(v)
            | Self::Priority(v)
            | Self::Width(v)
            | Self::Height(v)
            | Self::CornerRadius(v)
            | Self::StrokeWidth(v)
            | Self::Opacity(v)
            | Self::TextSize(v)
            | Self::MinZoom(v)
            | Self::SeatSize(v) => range(*v),
            // Any angle is taken and reduced to (−180°, 180°].
            Self::Rotation(v) => v.is_finite().then_some(()).ok_or_else(bad),
            Self::Rows(n) | Self::Seats(n) => range(f64::from(*n)),
            Self::Weight(n) => range(f64::from(*n)),
            Self::Curve(Curve::Arc { radius: v }) => (v.is_finite() && v.abs() >= 1.0)
                .then_some(())
                .ok_or_else(bad),
            Self::Curve(Curve::Bulge { amount: v }) => (v.is_finite() && v.abs() <= 1e6)
                .then_some(())
                .ok_or_else(bad),
            Self::Name(Some(s)) | Self::Entrance(Some(s)) => text(s, MAX_TEXT),
            Self::Label(Some(s)) => text(s, MAX_LABEL_LEN),
            Self::Text(s) if s.is_empty() => Err(bad()),
            Self::Text(s) | Self::LabelTemplate(s) | Self::TableLabelTemplate(s) => {
                text(s, MAX_TEXT)
            }
            Self::ChartName(s) if s.is_empty() => Err(bad()),
            Self::ChartName(s) => text(s, MAX_TEXT),
            Self::Fill(Some(c)) | Self::Stroke(Some(c)) | Self::Color(c) => colour(c),
            _ => Ok(()),
        }
    }
}

/// The command setting `field` on the nodes `ids` (or the chart), refused
/// with a typed error (the field's limits, a node without it, an unknown
/// category or floor) before anything changes.
pub(crate) fn command(doc: &Document, ids: &[u32], field: &Field) -> Result<Command, CommandError> {
    field.check()?;
    if let Some(c) = chart_command(doc, field) {
        return Ok(c);
    }
    match field {
        Field::Category(Some(key)) if !doc.categories().iter().any(|c| &c.key == key) => {
            return Err(CommandError::MissingCategory(key.clone()));
        }
        Field::Floor(Some(f)) if !doc.floors().iter().any(|l| l.id == *f) => {
            return Err(CommandError::MissingFloor(*f));
        }
        _ => {}
    }
    let mut commands = Vec::new();
    let mut transforms = Vec::new();
    for &id in ids {
        let node = doc.node(id).ok_or(CommandError::MissingNode(id))?;
        let na = || CommandError::NotApplicable {
            id,
            field: field.key(),
        };
        let set = |property| Command::SetProperty { id, property };
        let (origin, rotation) = each_node!(node, n => (n.origin, n.rotation));
        match (field, node) {
            (Field::X(v), _) | (Field::Y(v), _) => {
                let mut origin = origin;
                origin[usize::from(matches!(field, Field::Y(_)))] = *v;
                transforms.push(NodeTransform {
                    id,
                    origin,
                    rotation,
                });
            }
            (Field::Rotation(v), _) => transforms.push(NodeTransform {
                id,
                origin,
                rotation: normalise(*v),
            }),
            (Field::Name(v), _) => commands.push(set(Property::Name(v.clone()))),
            (Field::Locked(v), _) => commands.push(set(Property::Locked(*v))),
            (Field::Hidden(v), _) => commands.push(set(Property::Hidden(*v))),
            (Field::Floor(_), _) if doc.parent(id).is_some() => return Err(na()),
            (Field::Floor(v), _) => commands.push(set(Property::Floor(*v))),
            (Field::Category(v), n) if category_of(n).is_some() => {
                commands.push(set(Property::Category(v.clone())));
            }
            (Field::Label(v), n) if label_of(n).is_some() => {
                commands.push(set(Property::Label(v.clone())));
            }
            (Field::Rows(n), Node::RowBlock(b)) => {
                let have = u32::try_from(b.rows.len()).map_err(|_| CommandError::Limit("rows"))?;
                if *n > have {
                    let seats = b.rows.last().map_or(1, |r| r.seats);
                    let block = arc_after(b, |c| {
                        let back = c.rows.last().cloned();
                        c.rows.extend((have..*n).filter_map(|_| back.clone()));
                    });
                    arc(block, "rows")?;
                    commands.push(Command::AddRows {
                        id,
                        index: have,
                        count: n - have,
                        seats,
                    });
                } else if *n < have {
                    let rids = b.rows.iter().skip(*n as usize).map(|r| r.rid).collect();
                    commands.push(Command::RemoveRows { id, rids });
                }
            }
            (Field::Seats(n), Node::RowBlock(b)) => {
                arc(
                    arc_after(b, |c| c.rows.iter_mut().for_each(|r| r.seats = *n)),
                    "seats",
                )?;
                for r in &b.rows {
                    let (rid, end) = (r.rid, End::End);
                    let count = n.abs_diff(r.seats);
                    if *n > r.seats {
                        commands.push(Command::AddSeats {
                            id,
                            rid,
                            end,
                            count,
                        });
                    } else if *n < r.seats {
                        commands.push(Command::RemoveSeats {
                            id,
                            rid,
                            end,
                            count,
                        });
                    }
                }
            }
            (Field::SeatPitch(v), Node::RowBlock(b)) => {
                arc(arc_after(b, |c| c.seat_pitch = *v), "seatPitch")?;
                commands.push(set(Property::SeatPitch(*v)));
            }
            (Field::SeatPitch(v), Node::Row(_)) => commands.push(set(Property::SeatPitch(*v))),
            (Field::RowPitch(v), Node::RowBlock(b)) => {
                arc(arc_after(b, |c| c.row_pitch = *v), "rowPitch")?;
                commands.push(set(Property::RowPitch(*v)));
            }
            (Field::Curve(v), Node::RowBlock(b)) => {
                arc(arc_after(b, |c| c.curve = *v), "curve")?;
                commands.push(set(Property::Curve(*v)));
            }
            (Field::Align(v), Node::RowBlock(b)) => {
                arc(arc_after(b, |c| c.align = *v), "align")?;
                commands.push(set(Property::Align(*v)));
            }
            (Field::Stagger(v), Node::RowBlock(b)) => {
                arc(arc_after(b, |c| c.stagger = *v), "stagger")?;
                commands.push(set(Property::Stagger(*v)));
            }
            (Field::Skew(v), Node::RowBlock(_)) => commands.push(set(Property::Skew(*v))),
            (Field::Priority(v), Node::Section(_)) => commands.push(set(Property::Priority(*v))),
            (Field::Entrance(v), Node::Section(_)) => {
                commands.push(set(Property::Entrance(v.clone())));
            }
            (_, Node::Shape(_) | Node::Text(_) | Node::Image(_)) => {
                let new = decoration(node, field).ok_or_else(na)?;
                commands.push(set(Property::Decoration(Box::new(new))));
            }
            _ => return Err(na()),
        }
    }
    if !transforms.is_empty() {
        commands.push(Command::TransformNodes { nodes: transforms });
    }
    Ok(Command::Composite { commands })
}

/// The block after `f`, to check its arc (03 §2.4) before any command.
fn arc_after(b: &RowBlock, f: impl FnOnce(&mut RowBlock)) -> RowBlock {
    let mut c = b.clone();
    f(&mut c);
    c
}

fn arc(b: RowBlock, field: &'static str) -> Result<(), CommandError> {
    arc_ok(&b)
        .then_some(())
        .ok_or(CommandError::InvalidValue(field))
}

/// A decoration node with `field` set, or `None` when it has no such field.
fn decoration(node: &Node, field: &Field) -> Option<Node> {
    let mut node = node.clone();
    match (&mut node, field) {
        (Node::Shape(s), Field::Width(v) | Field::Height(v))
            if matches!(s.kind, ShapeKind::Rect | ShapeKind::Ellipse) =>
        {
            let size = s.size.get_or_insert([1.0, 1.0]);
            size[usize::from(matches!(field, Field::Height(_)))] = *v;
        }
        (Node::Shape(s), Field::CornerRadius(v)) if s.kind == ShapeKind::Rect => {
            s.corner_radius = Some(*v);
        }
        (Node::Shape(s), Field::Fill(v)) if s.kind != ShapeKind::Line => s.style.fill.clone_from(v),
        (Node::Shape(s), Field::Stroke(v)) => s.style.stroke.clone_from(v),
        (Node::Shape(s), Field::StrokeWidth(v)) => s.style.stroke_width = *v,
        (Node::Shape(s), Field::Opacity(v)) => s.style.opacity = *v,
        (Node::Shape(s), Field::Layer(v)) => s.layer = *v,
        (Node::Text(t), Field::Text(v)) => t.text.clone_from(v),
        (Node::Text(t), Field::TextSize(v)) => t.size = *v,
        (Node::Text(t), Field::Weight(v)) => t.weight = *v,
        (Node::Text(t), Field::Color(v)) => t.color.clone_from(v),
        (Node::Text(t), Field::TextAlign(v)) => t.align = *v,
        (Node::Text(t), Field::MinZoom(v)) => t.min_zoom = *v,
        (Node::Image(i), Field::Width(v) | Field::Height(v)) => {
            i.size[usize::from(matches!(field, Field::Height(_)))] = *v;
        }
        (Node::Image(i), Field::Opacity(v)) => i.opacity = *v,
        (Node::Image(i), Field::ImageRole(v)) => i.role = *v,
        _ => return None,
    }
    Some(node)
}

fn chart_command(doc: &Document, field: &Field) -> Option<Command> {
    let mut s = doc.settings().clone();
    match field {
        Field::ChartName(name) => {
            let property = ChartProperty::Name(name.clone());
            return Some(Command::SetChart { property });
        }
        Field::SeatSize(v) => s.seat_size = *v,
        Field::SeatShape(v) => s.seat_shape = *v,
        Field::LabelTemplate(v) => s.label_template.clone_from(v),
        Field::TableLabelTemplate(v) => s.table_label_template.clone_from(v),
        Field::RowLabelDisplay(v) => s.row_label_display = *v,
        Field::AlphaOverflow(v) => s.alpha_overflow = *v,
        _ => return None,
    }
    let property = ChartProperty::Settings(s);
    Some(Command::SetChart { property })
}
