//! Expansion of parametric rows into seats (`docs/03-chart-format.md` §2.4, §2.6,
//! §3.1 step 3).
//!
//! [`Expansion`] collects rows and seats from `rowBlock` and `row` nodes, in the
//! node's own coordinate space (before its origin and rotation are applied). Each
//! node's rows are pushed in physical order (front row first) and each row's seats
//! left to right, removed positions included and marked. Seats borrow strings from
//! the document, so expansion allocates a few times per node and never per seat.
//!
//! An override keyed by an `oid` that is not in the node is ignored here;
//! reporting it belongs to validation (03 §3.2).
//!
//! Positions use only exactly rounded `f64` arithmetic and [`crate::detmath`], so
//! the same document expands to the same bits on every target.

use std::collections::BTreeMap;
use std::fmt::{self, Write};

use cavea_core::label::{self, LabelBuf, LabelError, LabelRule, Scheme, SeatDirection};
use cavea_core::limits::{MAX_LABEL_LEN, MAX_OBJECTS, MAX_SEATS_PER_ROW};

use crate::detmath;
use crate::document as doc;

/// Rows and seats expanded so far. `rows[r].first_seat..+seat_count` indexes `seats`.
#[derive(Debug, Default)]
pub struct Expansion<'d> {
    pub rows: Vec<ExpandedRow<'d>>,
    pub seats: Vec<Seat<'d>>,
}

/// One row: a block row, or a free-form row. Its seats are
/// `seats[first_seat..first_seat + seat_count]`, left to right.
#[derive(Clone, Copy, Debug)]
pub struct ExpandedRow<'d> {
    /// Id of the `rowBlock` or `row` node.
    pub node: u32,
    /// `rid` in a block; the node id for a free-form row.
    pub rid: u32,
    /// The row's label: its override, or the scheme's label with prefix and suffix.
    pub label: LabelBuf,
    pub first_seat: u32,
    /// Positions, removed ones included.
    pub seat_count: u16,
    pub seat_pitch: f64,
    pub seat_rule: LabelRule<'d>,
    pub seat_direction: SeatDirection,
    /// Place in the block's numbering order (row label order).
    pub order: u32,
    /// Seat positions in rows earlier in numbering order, when numbering is
    /// continuous; otherwise 0.
    pub seat_base: u32,
}

impl ExpandedRow<'_> {
    /// Ordinal of the seat at `index` for [`LabelRule::write`].
    pub fn seat_ordinal(&self, index: u16) -> u32 {
        let within = label::seat_ordinal(
            self.seat_direction,
            self.order,
            self.seat_count.into(),
            index.into(),
        );
        self.seat_base.saturating_add(within)
    }

    /// Writes the seat's label: its override, or the row's rule.
    pub fn write_seat_label<W: Write>(
        &self,
        seat: &Seat<'_>,
        out: &mut W,
    ) -> Result<(), LabelError> {
        match seat.label {
            Some(text) => out.write_str(text).map_err(|_| LabelError::TooLong),
            None => self.seat_rule.write(self.seat_ordinal(seat.index), out),
        }
    }
}

/// One seat position, in the node's own space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seat<'d> {
    /// Node space, nudge included.
    pub pos: [f64; 2],
    pub oid: u32,
    /// Index into [`Expansion::rows`].
    pub row: u32,
    /// Position in the row, 0 at the left end.
    pub index: u16,
    /// A removed position: it keeps its `oid` and number but holds no seat.
    pub removed: bool,
    /// `SFLG` bits from the seat's override.
    pub flags: u8,
    /// The override's category, else the node's; `None` inherits from the section.
    pub category: Option<&'d str>,
    /// Label override.
    pub label: Option<&'d str>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExpandError {
    pub node: u32,
    pub kind: ExpandErrorKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExpandErrorKind {
    TooManySeatsInRow {
        rid: u32,
        seats: u32,
    },
    /// More than [`MAX_OBJECTS`] seat positions (removed ones included) or rows.
    TooManyObjects,
    /// The `oids` runs do not cover the row's positions exactly, or overflow.
    OidRuns {
        rid: u32,
    },
    RemovedOutOfRange {
        rid: u32,
        position: u32,
    },
    /// Non-finite, or outside the field's range.
    InvalidNumber {
        field: &'static str,
    },
    /// The arc's radius reaches zero at this row, or the row runs more than half
    /// a turn either side of the apex.
    ArcRow {
        rid: u32,
    },
    /// A path with fewer than two points or zero length.
    PathTooShort,
    /// `clockwise` and `counterclockwise` number table seats, not rows.
    DirectionNotForRows,
    InvalidLabelStart,
    Label {
        rid: u32,
        error: LabelError,
    },
}

impl fmt::Display for ExpandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "node {}: {}", self.node, self.kind)
    }
}

impl fmt::Display for ExpandErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            ExpandErrorKind::TooManySeatsInRow { rid, seats } => write!(
                f,
                "row {rid} has {seats} seats; the limit is {MAX_SEATS_PER_ROW}"
            ),
            ExpandErrorKind::TooManyObjects => write!(f, "more than {MAX_OBJECTS} seats"),
            ExpandErrorKind::OidRuns { rid } => {
                write!(f, "row {rid}: oid runs do not cover its seats")
            }
            ExpandErrorKind::RemovedOutOfRange { rid, position } => {
                write!(
                    f,
                    "row {rid}: removed position {position} is not in the row"
                )
            }
            ExpandErrorKind::InvalidNumber { field } => write!(f, "invalid {field}"),
            ExpandErrorKind::ArcRow { rid } => write!(f, "row {rid} does not fit its arc"),
            ExpandErrorKind::PathTooShort => f.write_str("path has no length"),
            ExpandErrorKind::DirectionNotForRows => {
                f.write_str("seat direction applies to tables only")
            }
            ExpandErrorKind::InvalidLabelStart => f.write_str("label start is not in its scheme"),
            ExpandErrorKind::Label { rid, error } => write!(f, "row {rid}: {error}"),
        }
    }
}

impl std::error::Error for ExpandError {}

type Kind = ExpandErrorKind;

/// A seat override with its flags folded into `SFLG` bits, sorted by `oid`.
/// Folding once per node keeps a long `flags` list from costing once per seat.
struct Override<'d> {
    oid: u32,
    flags: u8,
    over: &'d doc::SeatOverride,
}

fn resolve(overrides: &BTreeMap<u32, doc::SeatOverride>) -> Vec<Override<'_>> {
    overrides
        .iter()
        .map(|(&oid, over)| Override {
            oid,
            flags: over.flags.iter().fold(0, |acc, f| acc | f.bit()),
            over,
        })
        .collect()
}

/// What [`Expansion::push_seats`] needs to know about one row.
struct RowSpec<'d, 'a> {
    node: u32,
    rid: u32,
    label: LabelBuf,
    seats: u32,
    seat_pitch: f64,
    seat_rule: LabelRule<'d>,
    seat_direction: SeatDirection,
    order: u32,
    seat_base: u32,
    oids: &'a [[u32; 2]],
    removed: &'a [u32],
    overrides: &'a [Override<'d>],
    category: Option<&'d str>,
}

impl<'d> Expansion<'d> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Expands a `rowBlock`. On error nothing is added.
    pub fn push_row_block(
        &mut self,
        block: &'d doc::RowBlock,
        overflow: doc::AlphaOverflow,
    ) -> Result<(), ExpandError> {
        self.atomic(block.id, |e| e.row_block(block, overflow))
    }

    /// Expands a free-form `row`. On error nothing is added.
    pub fn push_row(
        &mut self,
        row: &'d doc::Row,
        overflow: doc::AlphaOverflow,
    ) -> Result<(), ExpandError> {
        self.atomic(row.id, |e| e.free_row(row, overflow))
    }

    fn atomic(
        &mut self,
        node: u32,
        f: impl FnOnce(&mut Self) -> Result<(), Kind>,
    ) -> Result<(), ExpandError> {
        let (rows, seats) = (self.rows.len(), self.seats.len());
        f(self).map_err(|kind| {
            self.rows.truncate(rows);
            self.seats.truncate(seats);
            ExpandError { node, kind }
        })
    }

    /// Checks the per-row and total limits and reserves space; returns the
    /// node's total seat positions.
    fn reserve(&mut self, rows: impl Iterator<Item = (u32, u32)>) -> Result<u32, Kind> {
        let (mut seats, mut count) = (0u64, 0u64);
        for (rid, n) in rows {
            if n > MAX_SEATS_PER_ROW {
                return Err(Kind::TooManySeatsInRow { rid, seats: n });
            }
            seats += u64::from(n);
            count += 1;
        }
        let limit = u64::from(MAX_OBJECTS);
        if self.seats.len() as u64 + seats > limit || self.rows.len() as u64 + count > limit {
            return Err(Kind::TooManyObjects);
        }
        self.seats.reserve(seats as usize);
        self.rows.reserve(count as usize);
        Ok(seats as u32)
    }

    fn row_block(
        &mut self,
        block: &'d doc::RowBlock,
        overflow: doc::AlphaOverflow,
    ) -> Result<(), Kind> {
        let seat_pitch = positive(block.seat_pitch, "seatPitch")?;
        let row_pitch = positive(block.row_pitch, "rowPitch")?;
        finite(block.stagger, "stagger")?;
        if !block.skew.is_finite() || block.skew.abs() >= 90.0 {
            return Err(Kind::InvalidNumber { field: "skew" });
        }
        let (sin, cos) = detmath::sin_cos(detmath::radians(block.skew));
        let shear = sin / cos;
        match block.curve {
            doc::Curve::None {} => {}
            doc::Curve::Arc { radius } if radius.is_finite() && radius != 0.0 => {}
            doc::Curve::Arc { .. } => {
                return Err(Kind::InvalidNumber {
                    field: "curve.radius",
                });
            }
            doc::Curve::Bulge { amount } => {
                finite(amount, "curve.amount")?;
            }
        }

        let total = self.reserve(block.rows.iter().map(|r| (r.rid, r.seats)))?;
        let overrides = resolve(&block.seat_overrides);
        let row_count = block.rows.len() as u32; // ≤ MAX_OBJECTS, checked by reserve
        let rl = &block.row_labels;
        let row_rule = rule(
            rl.scheme, &rl.start, rl.step, &rl.prefix, &rl.suffix, &rl.custom, overflow,
        )?;
        let skipped = row_rule.skipped_ordinals(&rl.skip);
        // Each row's label ordinal, `label::skip_ordinal` for every row in one
        // pass, so a long `skip` list is walked once per block, not once per row.
        let mut ordinals = Vec::with_capacity(block.rows.len());
        let (mut next, mut skips) = (0u32, skipped.iter().peekable());
        for _ in 0..row_count {
            while skips.next_if_eq(&&next).is_some() {
                next = next.saturating_add(1);
            }
            ordinals.push(next);
            next = next.saturating_add(1);
        }
        let sl = &block.seat_labels;
        let seat_rule = rule(
            sl.scheme, &sl.start, sl.step, &sl.prefix, &sl.suffix, &sl.custom, overflow,
        )?;
        let seat_direction = seat_direction(sl.direction)?;
        let direction = match rl.direction {
            doc::RowDirection::FrontToBack => label::RowDirection::FrontToBack,
            doc::RowDirection::BackToFront => {
                ordinals.reverse();
                label::RowDirection::BackToFront
            }
        };

        let mut in_front = 0u32;
        for ((i, row), ordinal) in (0u32..).zip(&block.rows).zip(ordinals) {
            let rid = row.rid;
            let order = label::row_ordinal(direction, row_count, i);
            let label = match &row.label {
                Some(text) => LabelBuf::try_from(text.as_str()),
                None => {
                    let mut buf = LabelBuf::new();
                    row_rule.write(ordinal, &mut buf).map(|()| buf)
                }
            }
            .map_err(|error| Kind::Label { rid, error })?;
            let seat_base = if sl.continuous {
                label::continuous_base(direction, total, in_front, row.seats)
            } else {
                0
            };
            in_front += row.seats;

            // Distance along the row of position 0, and the row's baseline.
            let n = row.seats;
            let length = f64::from(n.saturating_sub(1)) * seat_pitch;
            let align = match block.align {
                doc::Align::Left => 0.0,
                doc::Align::Center => -length / 2.0,
                doc::Align::Right => -length,
            };
            let stagger = if i % 2 == 1 {
                block.stagger * seat_pitch
            } else {
                0.0
            };
            let s0 = align + finite(row.offset, "offset")? + stagger;
            let y0 = f64::from(i) * row_pitch;
            if let doc::Curve::Arc { radius } = block.curve {
                let r = radius + y0;
                let reach = s0.abs().max((s0 + length).abs());
                if r == 0.0
                    || (r > 0.0) != (radius > 0.0)
                    || reach > core::f64::consts::PI * r.abs()
                {
                    return Err(Kind::ArcRow { rid });
                }
            }
            let place = |k: u32| -> [f64; 2] {
                let s = s0 + f64::from(k) * seat_pitch;
                let [x, y] = match block.curve {
                    doc::Curve::None {} => [s, y0],
                    doc::Curve::Bulge { amount } => {
                        let t = if n > 1 {
                            f64::from(k) / f64::from(n - 1)
                        } else {
                            0.5
                        };
                        let d = 2.0 * t - 1.0;
                        [s, y0 + amount * (1.0 - d * d)]
                    }
                    doc::Curve::Arc { radius } => {
                        let r = radius + y0;
                        let (sin, cos) = detmath::sin_cos(s / r);
                        [r * sin, r * cos - radius]
                    }
                };
                [x - y * shear, y]
            };
            self.push_seats(
                RowSpec {
                    node: block.id,
                    rid,
                    label,
                    seats: n,
                    seat_pitch,
                    seat_rule,
                    seat_direction,
                    order,
                    seat_base,
                    oids: &row.oids,
                    removed: &row.removed,
                    overrides: &overrides,
                    category: block.category.as_deref(),
                },
                place,
            )?;
        }
        Ok(())
    }

    fn free_row(&mut self, row: &'d doc::Row, overflow: doc::AlphaOverflow) -> Result<(), Kind> {
        let seat_pitch = positive(row.seat_pitch, "seatPitch")?;
        let doc::Path::Polyline { points } = &row.path;
        if points.iter().flatten().any(|v| !v.is_finite()) {
            return Err(Kind::InvalidNumber { field: "path" });
        }
        let segment = |j: usize| -> Option<([f64; 2], [f64; 2], f64)> {
            let (a, b) = (*points.get(j)?, *points.get(j + 1)?);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            Some((a, b, (dx * dx + dy * dy).sqrt()))
        };
        let segments = 0..points.len().saturating_sub(1);
        if segments
            .clone()
            .any(|j| segment(j).is_some_and(|(_, _, len)| !len.is_finite()))
        {
            return Err(Kind::InvalidNumber { field: "path" });
        }
        // The last segment with length; seats past the path's end continue along it.
        let last = segments
            .rev()
            .find(|&j| segment(j).is_some_and(|(_, _, len)| len > 0.0))
            .ok_or(Kind::PathTooShort)?;

        self.reserve(core::iter::once((row.id, row.seats)))?;
        let overrides = resolve(&row.seat_overrides);
        let sl = &row.seat_labels;
        let seat_rule = rule(
            sl.scheme, &sl.start, sl.step, &sl.prefix, &sl.suffix, &sl.custom, overflow,
        )?;
        let label = LabelBuf::try_from(row.label.as_deref().unwrap_or_default())
            .map_err(|error| Kind::Label { rid: row.id, error })?;

        // Seats are placed in order, so walk the path once.
        let (mut j, mut walked) = (0usize, 0.0f64);
        let place = move |k: u32| -> [f64; 2] {
            let s = f64::from(k) * seat_pitch;
            while let Some((a, b, len)) = segment(j) {
                if j == last || (len > 0.0 && s <= walked + len) {
                    let t = (s - walked) / len;
                    return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
                }
                walked += len;
                j += 1;
            }
            [f64::NAN; 2]
        };
        self.push_seats(
            RowSpec {
                node: row.id,
                rid: row.id,
                label,
                seats: row.seats,
                seat_pitch,
                seat_rule,
                seat_direction: seat_direction(sl.direction)?,
                order: 0,
                seat_base: 0,
                oids: &row.oids,
                removed: &row.removed,
                overrides: &overrides,
                category: row.category.as_deref(),
            },
            place,
        )
    }

    /// Pushes one row whose position `k` is at `place(k)`, before its nudge.
    /// `spec.seats` must already be within [`MAX_SEATS_PER_ROW`].
    fn push_seats(
        &mut self,
        spec: RowSpec<'d, '_>,
        mut place: impl FnMut(u32) -> [f64; 2],
    ) -> Result<(), Kind> {
        let rid = spec.rid;
        let mut covered = 0u32;
        for &[first, count] in spec.oids {
            first
                .checked_add(count.saturating_sub(1))
                .ok_or(Kind::OidRuns { rid })?;
            covered = covered.checked_add(count).ok_or(Kind::OidRuns { rid })?;
        }
        if covered != spec.seats {
            return Err(Kind::OidRuns { rid });
        }
        let mut gone = [0u64; 64]; // one bit per position; 64 × 64 > MAX_SEATS_PER_ROW
        for &position in spec.removed {
            let word = gone.get_mut((position / 64) as usize);
            match word {
                Some(word) if position < spec.seats => *word |= 1 << (position % 64),
                _ => return Err(Kind::RemovedOutOfRange { rid, position }),
            }
        }
        let row = self.rows.len() as u32; // ≤ MAX_OBJECTS, checked by reserve
        let expanded = ExpandedRow {
            node: spec.node,
            rid,
            label: spec.label,
            first_seat: self.seats.len() as u32, // ≤ MAX_OBJECTS, checked by reserve
            seat_count: spec.seats as u16,       // ≤ MAX_SEATS_PER_ROW
            seat_pitch: spec.seat_pitch,
            seat_rule: spec.seat_rule,
            seat_direction: spec.seat_direction,
            order: spec.order,
            seat_base: spec.seat_base,
        };
        let mut oids = spec
            .oids
            .iter()
            .flat_map(|&[first, count]| (0..count).map(move |j| first + j));
        for k in 0..spec.seats {
            // Every position's scheme label must be writable, overridden or not
            // (a custom list shorter than the row is an error). Roman and custom
            // label lengths do not grow with the ordinal, so check each one.
            let index = k as u16; // < MAX_SEATS_PER_ROW
            expanded
                .seat_rule
                .write(expanded.seat_ordinal(index), &mut LabelBuf::new())
                .map_err(|error| Kind::Label { rid, error })?;
            let oid = oids.next().ok_or(Kind::OidRuns { rid })?;
            let resolved = spec
                .overrides
                .binary_search_by_key(&oid, |o| o.oid)
                .ok()
                .and_then(|i| spec.overrides.get(i));
            let over = resolved.map(|o| o.over);
            let mut pos = place(k);
            if let Some([dx, dy]) = over.and_then(|o| o.nudge) {
                pos = [pos[0] + dx, pos[1] + dy];
            }
            if !(pos[0].is_finite() && pos[1].is_finite()) {
                return Err(Kind::InvalidNumber { field: "position" });
            }
            let label = over.and_then(|o| o.label.as_deref());
            if label.is_some_and(|l| l.len() > MAX_LABEL_LEN) {
                return Err(Kind::Label {
                    rid,
                    error: LabelError::TooLong,
                });
            }
            let removed = gone
                .get((k / 64) as usize)
                .is_some_and(|w| w & (1 << (k % 64)) != 0);
            self.seats.push(Seat {
                pos,
                oid,
                row,
                index,
                removed,
                flags: resolved.map_or(0, |o| o.flags),
                category: over.and_then(|o| o.category.as_deref()).or(spec.category),
                label,
            });
        }
        self.rows.push(expanded);
        Ok(())
    }
}

fn finite(v: f64, field: &'static str) -> Result<f64, Kind> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(Kind::InvalidNumber { field })
    }
}

fn positive(v: f64, field: &'static str) -> Result<f64, Kind> {
    if v.is_finite() && v > 0.0 {
        Ok(v)
    } else {
        Err(Kind::InvalidNumber { field })
    }
}

fn seat_direction(direction: doc::SeatDirection) -> Result<SeatDirection, Kind> {
    match direction {
        doc::SeatDirection::Ltr => Ok(SeatDirection::Ltr),
        doc::SeatDirection::Rtl => Ok(SeatDirection::Rtl),
        doc::SeatDirection::CenterOut => Ok(SeatDirection::CenterOut),
        doc::SeatDirection::Serpentine => Ok(SeatDirection::Serpentine),
        doc::SeatDirection::Clockwise | doc::SeatDirection::Counterclockwise => {
            Err(Kind::DirectionNotForRows)
        }
    }
}

/// Builds the core rule for a document label scheme.
fn rule<'d>(
    scheme: doc::LabelScheme,
    start: &Option<doc::LabelStart>,
    step: u32,
    prefix: &'d str,
    suffix: &'d str,
    custom: &'d [String],
    overflow: doc::AlphaOverflow,
) -> Result<LabelRule<'d>, Kind> {
    let scheme = match scheme {
        doc::LabelScheme::Numeric => Scheme::Numeric,
        doc::LabelScheme::Odd => Scheme::Odd,
        doc::LabelScheme::Even => Scheme::Even,
        doc::LabelScheme::Alpha => Scheme::Alpha,
        doc::LabelScheme::AlphaLower => Scheme::AlphaLower,
        doc::LabelScheme::Roman => Scheme::Roman,
        doc::LabelScheme::Custom => Scheme::Custom,
    };
    let overflow = match overflow {
        doc::AlphaOverflow::Double => label::AlphaOverflow::Double,
        doc::AlphaOverflow::Spreadsheet => label::AlphaOverflow::Spreadsheet,
    };
    let start = match start {
        None if scheme == Scheme::Even => 2,
        None => 1,
        Some(doc::LabelStart::Number(n)) => *n,
        Some(doc::LabelStart::Text(_)) if scheme == Scheme::Custom => 0,
        Some(doc::LabelStart::Text(text)) => {
            label::parse_value(scheme, overflow, text).ok_or(Kind::InvalidLabelStart)?
        }
    };
    Ok(LabelRule {
        scheme,
        start,
        step,
        overflow,
        prefix,
        suffix,
        custom,
    })
}

#[cfg(test)]
mod tests;
