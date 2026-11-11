//! Drafts (`docs/07-designer.md` §4): the node a drawing tool makes, given
//! as numbers, so that the tools, the chrome's insert forms (§15) and tests
//! all produce it through one function. Positions are in the space of the
//! parent the node goes into. A draft that the compiler would reject (no
//! seats, an arc past half a turn) is refused here, before anything is
//! created.

use std::f64::consts::TAU;

use cavea_compiler::document::{Align, BlockRow, Curve, Node, RowBlock, RowLabels, SeatLabels};
use cavea_core::limits::{MAX_OBJECTS, MAX_SEATS_PER_ROW};

use crate::command::CommandError;
use crate::index::P;

/// The smallest radius of an arc block's front row.
pub const MIN_RADIUS: f64 = 55.0;
/// The smallest width or height a shape is scaled to, in chart units.
pub const MIN_SIZE: f64 = 1.0;

/// A node to create, in numbers.
#[derive(Debug, Clone, PartialEq)]
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

impl Draft {
    /// The node, with ids of 0 (`createNode` gives it fresh ones), the
    /// category for seats, and the floor (`None` inherits or takes the
    /// first). Fails on anything the compiler would reject.
    pub fn node(&self, category: Option<&str>, floor: Option<u32>) -> Result<Node> {
        let category = category.map(String::from);
        let mut node = match *self {
            Draft::RowBlock {
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
            Draft::ArcBlock {
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
        };
        crate::document::each_node!(&mut node, n => n.floor = floor);
        Ok(node)
    }
}
