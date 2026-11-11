//! Drafts (`docs/07-designer.md` §4): the node a drawing tool makes, given
//! as numbers, so that the tools, the chrome's insert forms (§15) and tests
//! all produce it through one function. Positions are in the space of the
//! parent the node goes into. A draft that the compiler would reject (no
//! seats, zero size, a self-intersecting polygon, an arc past half a turn)
//! is refused here, before anything is created.

use std::f64::consts::TAU;

use cavea_compiler::document::{
    Align, BlockRow, Curve, Image, ImageRole, Layer, Node, Path, Row, RowBlock, RowLabels,
    SeatLabels, Shape, ShapeKind, Style, Text,
};
use cavea_core::limits::{MAX_OBJECTS, MAX_SEATS_PER_ROW};

use crate::command::CommandError;
use crate::index::P;

/// The smallest width, height or length a drawn shape or row may have, in
/// chart units.
pub const MIN_SIZE: f64 = 1.0;
/// The smallest radius of an arc block's front row.
pub const MIN_RADIUS: f64 = 55.0;
/// Points a polygon may have (02 §14: points per shape).
pub const MAX_POINTS: usize = 4_096;
/// A bent single row is flattened to within this distance of its curve…
pub const BEND_TOLERANCE: f64 = 0.5;
/// …with at most this many segments.
pub const MAX_BEND_SEGMENTS: usize = 64;
/// Defaults of a new text node.
pub const TEXT_SIZE: f64 = 60.0;
pub const TEXT_WEIGHT: u16 = 400;
pub const TEXT_COLOR: &str = "#111827";
/// A new rectangle, ellipse or polygon: a neutral fill, no stroke.
pub const SHAPE_FILL: &str = "#D1D5DB";
/// A new line: never filled, so it has a stroke.
pub const LINE_STROKE: &str = "#6B7280";
pub const LINE_WIDTH: f64 = 4.0;

/// A node to create, in numbers. In JSON tagged by `kind`:
/// `{"kind": "rowBlock", "origin": [0, 0], "rows": 5, …}`.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Draft {
    /// Straight rows, centre-aligned; `origin` is the centre of the front
    /// row and rows run towards +y.
    RowBlock {
        origin: [f64; 2],
        rows: u32,
        seats: u32,
        seat_pitch: f64,
        row_pitch: f64,
    },
    /// Concentric rows about `center`: the front row has radius `radius` and
    /// `seats` seats centred on `direction` (degrees, clockwise from +x);
    /// each row behind it (radius `radius + i × row_pitch`) fills the same
    /// angle, so it has `⌊(seats − 1) × Rᵢ / radius⌋ + 1` seats.
    ArcBlock {
        center: [f64; 2],
        radius: f64,
        direction: f64,
        rows: u32,
        seats: u32,
        seat_pitch: f64,
        row_pitch: f64,
    },
    /// A free-form row from `start` to `end`, bent through `bend` if given,
    /// with as many seats as fit at the pitch.
    Row {
        start: [f64; 2],
        end: [f64; 2],
        bend: Option<[f64; 2]>,
        seat_pitch: f64,
    },
    Rect {
        center: [f64; 2],
        size: [f64; 2],
    },
    Ellipse {
        center: [f64; 2],
        size: [f64; 2],
    },
    Line {
        start: [f64; 2],
        end: [f64; 2],
    },
    /// A closed polygon: at least three corners, simple, not flat.
    Polygon {
        points: Vec<[f64; 2]>,
    },
    Text {
        at: [f64; 2],
        text: String,
    },
    /// An image `size` wide and high centred on `center`, unturned and
    /// opaque (03 §2.7). `asset` is its content hash as `POST /v1/assets`
    /// returns it, 32 lowercase hex digits.
    Image {
        center: [f64; 2],
        size: [f64; 2],
        asset: String,
        role: ImageRole,
    },
}

/// The form of an asset hash (03 §2.7): 32 lowercase hex digits.
pub fn valid_asset(asset: &str) -> bool {
    asset.len() == 32 && asset.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

type Result<T> = std::result::Result<T, CommandError>;

fn check(ok: bool, field: &'static str) -> Result<()> {
    ok.then_some(()).ok_or(CommandError::InvalidValue(field))
}

fn finite(values: &[f64], field: &'static str) -> Result<()> {
    check(values.iter().all(|v| v.is_finite()), field)
}

fn pitch(v: f64, field: &'static str) -> Result<()> {
    check(v.is_finite() && v > 0.0, field)
}

/// Seats and rows within the limits of 02 §14, checked before any row is
/// built.
fn counts(rows: u32, widest: u32) -> Result<()> {
    check(rows > 0 && widest > 0, "count")?;
    if widest > MAX_SEATS_PER_ROW {
        return Err(CommandError::Limit("seats per row"));
    }
    if u64::from(rows) * u64::from(widest) > u64::from(MAX_OBJECTS) {
        return Err(CommandError::Limit("objects"));
    }
    Ok(())
}

/// Seats of row `i` of an arc block whose front row has `seats`.
/// `u32::MAX` for a row past the per-row limit.
pub(crate) fn arc_row_seats(seats: u32, radius: f64, row_pitch: f64, i: u32) -> u32 {
    if seats <= 1 {
        return 1;
    }
    let ratio = 1.0 + f64::from(i) * row_pitch / radius;
    // The small allowance keeps rounding from losing a seat the angle holds.
    let gaps = (f64::from(seats - 1) * ratio + 1e-9).floor();
    if gaps.is_finite() && gaps < f64::from(MAX_SEATS_PER_ROW) {
        gaps as u32 + 1
    } else {
        u32::MAX
    }
}

/// The most seats a front row of `radius` holds: the ends at least a pitch
/// apart, and every seat within half a turn of the apex (03 §2.4).
pub(crate) fn arc_max_seats(radius: f64, seat_pitch: f64) -> u32 {
    let n = ((TAU * radius - seat_pitch) / seat_pitch).floor() + 1.0;
    n.clamp(1.0, f64::from(MAX_SEATS_PER_ROW)) as u32
}

/// A rotation reduced to (−180, 180].
pub(crate) fn normalise(degrees: f64) -> f64 {
    let r = degrees % 360.0;
    if r > 180.0 {
        r - 360.0
    } else if r <= -180.0 {
        r + 360.0
    } else {
        r
    }
}

fn dist(a: P, b: P) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

fn block_rows(seats: impl Iterator<Item = u32>) -> Vec<BlockRow> {
    seats
        .map(|n| BlockRow {
            rid: 0,
            seats: n,
            offset: 0.0,
            oids: vec![[0, n]],
            label: None,
            removed: Vec::new(),
        })
        .collect()
}

fn row_block(origin: P, rotation: f64, curve: Curve, rows: Vec<BlockRow>) -> RowBlock {
    RowBlock {
        id: 0,
        name: None,
        floor: None,
        origin,
        rotation,
        locked: false,
        hidden: false,
        seat_pitch: 55.0,
        row_pitch: 95.0,
        curve,
        align: Align::Center,
        stagger: 0.0,
        skew: 0.0,
        category: None,
        row_labels: RowLabels::default(),
        seat_labels: SeatLabels::default(),
        rows,
        seat_overrides: Default::default(),
    }
}

fn shape(kind: ShapeKind, origin: P, size: Option<P>, points: Option<Vec<P>>) -> Shape {
    let style = match kind {
        ShapeKind::Line => Style {
            stroke: Some(LINE_STROKE.into()),
            stroke_width: LINE_WIDTH,
            ..Style::default()
        },
        _ => Style {
            fill: Some(SHAPE_FILL.into()),
            ..Style::default()
        },
    };
    Shape {
        id: 0,
        name: None,
        floor: None,
        origin,
        rotation: 0.0,
        locked: false,
        hidden: false,
        kind,
        size,
        corner_radius: None,
        points,
        d: None,
        style,
        layer: Layer::Below,
    }
}

/// Points along the quadratic curve from `a` to `b` through `m` (at its
/// middle), within [`BEND_TOLERANCE`] of it: a chord of `n` equal parameter
/// steps strays from the curve by at most `sag / n²`, `sag` being `m`'s
/// distance from the middle of `ab`.
pub(crate) fn bend(a: P, b: P, m: P) -> Vec<P> {
    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let sag = dist(mid, m);
    if sag < BEND_TOLERANCE {
        return vec![a, b];
    }
    let n = (sag / BEND_TOLERANCE)
        .sqrt()
        .ceil()
        .clamp(2.0, MAX_BEND_SEGMENTS as f64) as usize;
    let c = [2.0 * m[0] - mid[0], 2.0 * m[1] - mid[1]];
    (0..=n)
        .map(|j| {
            let t = j as f64 / n as f64;
            let (u, v, w) = ((1.0 - t) * (1.0 - t), 2.0 * t * (1.0 - t), t * t);
            [
                u * a[0] + v * c[0] + w * b[0],
                u * a[1] + v * c[1] + w * b[1],
            ]
        })
        .collect()
}

fn cross(o: P, a: P, b: P) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// Whether the closed segments `ab` and `cd` meet.
fn meet(a: P, b: P, c: P, d: P) -> bool {
    let on = |p: P, q: P, r: P| {
        r[0] >= p[0].min(q[0])
            && r[0] <= p[0].max(q[0])
            && r[1] >= p[1].min(q[1])
            && r[1] <= p[1].max(q[1])
    };
    let (d1, d2) = (cross(c, d, a), cross(c, d, b));
    let (d3, d4) = (cross(a, b, c), cross(a, b, d));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    (d1 == 0.0 && on(c, d, a))
        || (d2 == 0.0 && on(c, d, b))
        || (d3 == 0.0 && on(a, b, c))
        || (d4 == 0.0 && on(a, b, d))
}

/// Whether the edge from the last point to `q` may extend the open path
/// `points` (itself simple): long enough, not doubling back along the last
/// edge, and meeting no earlier edge. One pass over the edges.
pub(crate) fn extends(points: &[P], q: P) -> bool {
    let Some(&last) = points.last() else {
        return true;
    };
    if dist(last, q) < MIN_SIZE {
        return false;
    }
    let n = points.len();
    if let Some(&prev) = n.checked_sub(2).and_then(|i| points.get(i))
        && cross(prev, last, q) == 0.0
        && (last[0] - prev[0]) * (q[0] - last[0]) + (last[1] - prev[1]) * (q[1] - last[1]) < 0.0
    {
        return false;
    }
    // Every edge but the last, which shares `last`.
    let edges = points.windows(2).take(n.saturating_sub(2));
    !edges.into_iter().any(|e| meet(e[0], e[1], last, q))
}

/// Whether the open, simple path `points` closes into a valid polygon: at
/// least three points, a closing edge that meets no other edge except at
/// its ends and does not double back, and not flat.
pub(crate) fn closes(points: &[P]) -> bool {
    let n = points.len();
    let (Some(&first), Some(&last)) = (points.first(), points.last()) else {
        return false;
    };
    if !(3..=MAX_POINTS).contains(&n) || dist(last, first) < MIN_SIZE {
        return false;
    }
    let area: f64 = (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    if area.abs() / 2.0 < MIN_SIZE * MIN_SIZE {
        return false;
    }
    // The closing edge against the edges not touching it, and both of its
    // neighbours against doubling back.
    let inner = points.windows(2).skip(1).take(n.saturating_sub(3));
    if inner.into_iter().any(|e| meet(e[0], e[1], last, first)) {
        return false;
    }
    let back = |p: P, q: P, r: P| {
        cross(p, q, r) == 0.0 && (q[0] - p[0]) * (r[0] - q[0]) + (q[1] - p[1]) * (r[1] - q[1]) < 0.0
    };
    !back(points[n - 2], last, first) && !back(last, first, points[1])
}

/// Whether `points` is a valid polygon (03 §2.7, §3.1.1): simple, at least
/// three distinct corners, not flat, at most [`MAX_POINTS`]. Quadratic in
/// the points.
pub(crate) fn simple(points: &[P]) -> bool {
    points.len() <= MAX_POINTS
        && (1..points.len()).all(|i| extends(&points[..i], points[i]))
        && closes(points)
}

impl Draft {
    /// The node, with ids of 0 (`createNode` gives it fresh ones), the
    /// category for seats, and the floor (`None` inherits or takes the
    /// first). Fails on anything the compiler would reject.
    pub fn node(&self, category: Option<&str>, floor: Option<u32>) -> Result<Node> {
        let category = category.map(String::from);
        let mut node = match self {
            &Draft::RowBlock {
                origin,
                rows,
                seats,
                seat_pitch,
                row_pitch,
            } => {
                finite(&origin, "origin")?;
                pitch(seat_pitch, "seatPitch")?;
                pitch(row_pitch, "rowPitch")?;
                counts(rows, seats)?;
                let mut b = row_block(
                    origin,
                    0.0,
                    Curve::None {},
                    block_rows((0..rows).map(|_| seats)),
                );
                (b.seat_pitch, b.row_pitch, b.category) = (seat_pitch, row_pitch, category);
                Node::RowBlock(b)
            }
            &Draft::ArcBlock {
                center,
                radius,
                direction,
                rows,
                seats,
                seat_pitch,
                row_pitch,
            } => {
                finite(&[center[0], center[1], direction], "origin")?;
                pitch(seat_pitch, "seatPitch")?;
                pitch(row_pitch, "rowPitch")?;
                check(radius.is_finite() && radius >= MIN_RADIUS, "curve")?;
                check(seats <= arc_max_seats(radius, seat_pitch), "span")?;
                counts(
                    rows,
                    arc_row_seats(seats, radius, row_pitch, rows.saturating_sub(1)),
                )?;
                let (sin, cos) = direction.to_radians().sin_cos();
                let origin = [center[0] + radius * cos, center[1] + radius * sin];
                finite(&origin, "origin")?;
                let list = (0..rows).map(|i| arc_row_seats(seats, radius, row_pitch, i));
                let curve = Curve::Arc { radius };
                let mut b = row_block(origin, normalise(direction - 90.0), curve, block_rows(list));
                (b.seat_pitch, b.row_pitch, b.category) = (seat_pitch, row_pitch, category);
                Node::RowBlock(b)
            }
            &Draft::Row {
                start,
                end,
                bend: through,
                seat_pitch,
            } => {
                finite(&[start[0], start[1], end[0], end[1]], "path")?;
                pitch(seat_pitch, "seatPitch")?;
                check(dist(start, end) >= MIN_SIZE, "path")?;
                let local = |p: P| [p[0] - start[0], p[1] - start[1]];
                let points: Vec<P> = match through {
                    Some(m) => {
                        finite(&m, "path")?;
                        bend(local(start), local(end), local(m))
                    }
                    None => vec![[0.0, 0.0], local(end)],
                };
                let length: f64 = points.windows(2).map(|w| dist(w[0], w[1])).sum();
                let seats = (length / seat_pitch + 1e-9).floor() + 1.0;
                check(seats.is_finite(), "path")?;
                if seats > f64::from(MAX_SEATS_PER_ROW) {
                    return Err(CommandError::Limit("seats per row"));
                }
                let seats = seats as u32;
                Node::Row(Row {
                    id: 0,
                    name: None,
                    floor: None,
                    origin: start,
                    rotation: 0.0,
                    locked: false,
                    hidden: false,
                    path: Path::Polyline { points },
                    seat_pitch,
                    seats,
                    oids: vec![[0, seats]],
                    label: Some("A".into()),
                    seat_labels: SeatLabels::default(),
                    category,
                    removed: Vec::new(),
                    seat_overrides: Default::default(),
                })
            }
            &Draft::Rect { center, size } | &Draft::Ellipse { center, size } => {
                finite(&center, "origin")?;
                check(size.iter().all(|&v| v.is_finite() && v >= MIN_SIZE), "size")?;
                let kind = match self {
                    Draft::Rect { .. } => ShapeKind::Rect,
                    _ => ShapeKind::Ellipse,
                };
                Node::Shape(shape(kind, center, Some(size), None))
            }
            &Draft::Line { start, end } => {
                finite(&[start[0], start[1], end[0], end[1]], "points")?;
                check(dist(start, end) >= MIN_SIZE, "points")?;
                let points = vec![[0.0, 0.0], [end[0] - start[0], end[1] - start[1]]];
                Node::Shape(shape(ShapeKind::Line, start, None, Some(points)))
            }
            Draft::Polygon { points } => {
                finite(points.as_flattened(), "points")?;
                check(simple(points), "points")?;
                let first = points.first().copied().unwrap_or_default();
                let local = points
                    .iter()
                    .map(|p| [p[0] - first[0], p[1] - first[1]])
                    .collect();
                Node::Shape(shape(ShapeKind::Polygon, first, None, Some(local)))
            }
            Draft::Text { at, text } => {
                finite(at, "origin")?;
                check((1..=256).contains(&text.len()), "text")?;
                Node::Text(Text {
                    id: 0,
                    name: None,
                    floor: None,
                    origin: *at,
                    rotation: 0.0,
                    locked: false,
                    hidden: false,
                    text: text.clone(),
                    size: TEXT_SIZE,
                    weight: TEXT_WEIGHT,
                    color: TEXT_COLOR.into(),
                    align: Align::Center,
                    min_zoom: 0.0,
                })
            }
            Draft::Image {
                center,
                size,
                asset,
                role,
            } => {
                finite(center, "origin")?;
                check(size.iter().all(|&v| v.is_finite() && v >= MIN_SIZE), "size")?;
                check(valid_asset(asset), "asset")?;
                Node::Image(Image {
                    id: 0,
                    name: None,
                    floor: None,
                    origin: *center,
                    rotation: 0.0,
                    locked: false,
                    hidden: false,
                    asset: asset.clone(),
                    size: *size,
                    opacity: 1.0,
                    role: *role,
                })
            }
        };
        crate::document::each_node!(&mut node, n => n.floor = floor);
        Ok(node)
    }
}
