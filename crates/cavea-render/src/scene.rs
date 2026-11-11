//! The scene on the CPU (`docs/04-renderer.md` §5): a decoded compiled chart
//! plus the per-seat arrays the renderer keeps, built in one linear pass with
//! one allocation per array.

use std::ops::Range;

use cavea_format::{Artefact, Chart, NONE, SeatShape, Segment};

use crate::{
    protocol::{Bounds, CategoryInfo, ChartInfo, FloorInfo, SectionInfo},
    seat::{self, SeatStatic},
};

/// A loaded chart. Per-seat arrays are indexed by `idx`.
pub struct Scene {
    /// The decoded chart, for its tables: categories, floors, sections, render
    /// chunks, rows, segments, label rules and exceptions. Every index in it is
    /// in range (`cavea_format::decode`). Its `seat_pos` is empty: the
    /// positions were converted in place into `pos`. Categories and flags per
    /// seat are its `seat_category` and `seat_flags`.
    pub chart: Chart,
    /// Seat centres in chart units.
    pub pos: Vec<[f32; 2]>,
    /// Each seat's row, an index into `chart.rows`.
    pub row: Vec<u32>,
    /// Public status (`02-domain-model.md` §4.2); 0 until state arrives.
    pub status: Vec<u8>,
    /// Visual state (04 §9), derived from `status` and the selection.
    pub visual: Vec<u8>,
    /// Effect bits: 0 dim, 1 highlight, 2 pulse.
    pub fx: Vec<u8>,
    /// The seats drawn: the active floor's `idx` range. The first floor until
    /// `SetFloor` exists; seats of other floors stay loaded but are not drawn.
    pub active: Range<u32>,
    /// The active floor, an index into `chart.floors`.
    pub floor: u32,
    /// The active floor's rows outside any section, for hit testing (04 §15).
    pub loose_rows: Vec<u32>,
    /// What the camera fits and clamps to: the active floor's seat centres
    /// grown by `max_half`.
    pub bounds: Bounds,
    /// The largest half seat size in chart units, as the static buffer
    /// encodes it: how far a seat reaches past its centre along its own axes.
    /// Culling grows render chunk boxes (of centres) by this, rotated (04 §11).
    pub max_half: f32,
    /// The chart's default seat size in chart units.
    pub seat_size: f32,
}

impl Scene {
    pub fn decode(ccf: &[u8]) -> Result<Self, cavea_format::Error> {
        let mut chart = cavea_format::decode(ccf, Artefact::Public)?;
        // The element types have the same size and alignment, so the standard
        // library collects in place into the decoded array's buffer.
        let pos: Vec<[f32; 2]> = std::mem::take(&mut chart.seat_pos)
            .into_iter()
            .map(|[x, y]| [x as f32 * 0.25, y as f32 * 0.25])
            .collect();
        Ok(Self::assemble(chart, pos))
    }

    /// A scene of a chart whose positions are already in `pos` (its own
    /// `seat_pos` empty): the designer's live scene (04 §25). The chart must
    /// pass [`Chart::check`] with `pos` as its positions.
    #[cfg(feature = "designer")]
    pub fn from_chart(chart: Chart, pos: Vec<[f32; 2]>) -> Self {
        Self::assemble(chart, pos)
    }

    fn assemble(chart: Chart, pos: Vec<[f32; 2]>) -> Self {
        let seats = pos.len();
        let mut row = Vec::with_capacity(seats);
        for (r, record) in (0u32..).zip(&chart.rows) {
            row.extend(std::iter::repeat_n(r, usize::from(record.seat_count)));
        }
        let seat_size = (f32::from(chart.meta.seat_size) * 0.25).max(0.25);
        let max_half = chart
            .rows
            .iter()
            .map(|r| f32::from(r.seat_size.div_ceil(2)) * 0.25)
            .fold(seat_size / 2.0, f32::max);
        let (active, floor_box) = chart.floors.first().map_or((0..0, [0; 4]), |f| {
            (f.seat_first..f.seat_first + f.seat_count, f.bounds)
        });
        let loose_rows = (0u32..)
            .zip(&chart.rows)
            .filter(|(_, r)| r.section == NONE && active.contains(&r.seat_first))
            .map(|(i, _)| i)
            .collect();
        let [x0, y0, x1, y1] = floor_box.map(|v| v as f32 * 0.25);
        let bounds = Bounds {
            x: x0 - max_half,
            y: y0 - max_half,
            width: x1 - x0 + 2.0 * max_half,
            height: y1 - y0 + 2.0 * max_half,
        };
        Self {
            chart,
            pos,
            row,
            status: vec![0; seats],
            visual: vec![0; seats],
            fx: vec![0; seats],
            active,
            floor: 0,
            loose_rows,
            bounds,
            max_half,
            seat_size,
        }
    }

    /// The static instance buffer (04 §6) for every seat: position, half size
    /// and shape from the seat's row, and for oriented shapes (rounded, chair)
    /// the angle of the row at the seat, from its neighbours in its segment.
    /// A seat alone in its segment takes the direction of its whole row.
    pub fn encode_static(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pos.len() * seat::STATIC_STRIDE);
        for segment in &self.chart.segments {
            let first = segment.seat_first;
            for i in first..first + u32::from(segment.seat_count) {
                if let Some(seat) = self.in_segment(segment, i) {
                    out.extend_from_slice(&seat.encode());
                }
            }
        }
        out
    }

    /// The static instance bytes of seats `range`, as [`Self::encode_static`]
    /// writes them: a live scene's piece uploaded alone (04 §25).
    #[cfg(feature = "designer")]
    pub fn encode_static_range(&self, range: Range<u32>) -> Vec<u8> {
        let mut out = Vec::with_capacity(range.len() * seat::STATIC_STRIDE);
        let segments = &self.chart.segments;
        let first =
            segments.partition_point(|s| s.seat_first + u32::from(s.seat_count) <= range.start);
        for segment in segments.get(first..).unwrap_or_default() {
            if segment.seat_first >= range.end {
                break;
            }
            let end = segment.seat_first + u32::from(segment.seat_count);
            for i in segment.seat_first.max(range.start)..end.min(range.end) {
                if let Some(seat) = self.in_segment(segment, i) {
                    out.extend_from_slice(&seat.encode());
                }
            }
        }
        out
    }

    /// Seat `i`'s section through its row, or `NONE`.
    #[inline]
    pub fn section_of(&self, i: usize) -> u32 {
        let row = self
            .row
            .get(i)
            .and_then(|&r| self.chart.rows.get(r as usize));
        row.map_or(NONE, |r| r.section)
    }

    /// Seat `idx`'s static instance values, as the static buffer holds them.
    pub fn seat_static(&self, idx: u32) -> Option<SeatStatic> {
        let segments = &self.chart.segments;
        let i = segments.partition_point(|s| s.seat_first <= idx);
        self.in_segment(segments.get(i.checked_sub(1)?)?, idx)
    }

    fn in_segment(&self, segment: &Segment, i: u32) -> Option<SeatStatic> {
        let at = |i: u32| self.pos.get(i as usize).copied().unwrap_or_default();
        let row = self.chart.rows.get(segment.row as usize)?;
        let shape = match row.shape {
            SeatShape::Circle => seat::shape::CIRCLE,
            SeatShape::Rounded => seat::shape::ROUNDED_SQUARE,
            SeatShape::Chair => seat::shape::CHAIR,
        };
        let first = segment.seat_first;
        let last = first + u32::from(segment.seat_count).saturating_sub(1);
        let row_last = row.seat_first + u32::from(row.seat_count).saturating_sub(1);
        let (a, b) = if first == last {
            (at(row.seat_first), at(row_last))
        } else {
            (at(i.saturating_sub(1).max(first)), at((i + 1).min(last)))
        };
        let angle = if row.shape == SeatShape::Circle {
            0
        } else {
            binary_angle(b[0] - a[0], b[1] - a[1])
        };
        let accessible = self
            .chart
            .seat_flags
            .get(i as usize)
            .is_some_and(|f| f & 1 != 0);
        Some(SeatStatic {
            pos: at(i),
            half: row.seat_size.div_ceil(2),
            angle,
            shape: if accessible {
                shape | seat::shape::ACCESSIBLE
            } else {
                shape
            },
        })
    }

    /// Metadata for `Ready`. Allocates per category, floor and section, not
    /// per seat.
    pub fn info(&self) -> ChartInfo {
        let c = &self.chart;
        let text = |i: u32| c.strings.get(i).unwrap_or_default().to_owned();
        let mut categories: Vec<CategoryInfo> = c
            .categories
            .iter()
            .map(|cat| CategoryInfo {
                key: text(cat.key),
                label: text(cat.label),
                color: cat.color,
                objects: 0,
            })
            .collect();
        for &cat in &c.seat_category {
            if let Some(info) = categories.get_mut(usize::from(cat)) {
                info.objects += 1;
            }
        }
        for object in &c.objects {
            if let Some(info) = categories.get_mut(object.category as usize) {
                info.objects += 1;
            }
        }
        let seats = c.meta.seat_count;
        ChartInfo {
            name: text(c.meta.name),
            floors: c
                .floors
                .iter()
                .map(|f| FloorInfo {
                    name: text(f.name),
                    level: f.level,
                    seats: f.seat_count,
                })
                .collect(),
            sections: c
                .sections
                .iter()
                .map(|s| SectionInfo {
                    label: text(s.label),
                    floor: s.floor,
                    seats: s.seat_count,
                })
                .collect(),
            categories,
            // At most 1,000,000 objects after a successful decode.
            objects: seats.saturating_add(c.objects.len() as u32),
            seats,
        }
    }
}

/// The direction of `(dx, dy)` in 256ths of a turn, clockwise from +x with y
/// down (04 §6). Uses a polynomial arctangent (error under 0.25°, a fifth of
/// one step) instead of `atan2`, which would pull library maths into the WASM.
pub fn binary_angle(dx: f32, dy: f32) -> u8 {
    let (ax, ay) = (dx.abs(), dy.abs());
    if ax.max(ay) == 0.0 || !(ax + ay).is_finite() {
        return 0;
    }
    // Turns in the first octant, then unfolded.
    let z = ax.min(ay) / ax.max(ay);
    let mut t = z * (0.125 + 0.043_45 * (1.0 - z));
    if ay > ax {
        t = 0.25 - t;
    }
    if dx < 0.0 {
        t = 0.5 - t;
    }
    if dy < 0.0 {
        t = 1.0 - t;
    }
    // Truncation after adding a half rounds a non-negative value.
    ((t * 256.0 + 0.5) as u32 & 255) as u8
}
