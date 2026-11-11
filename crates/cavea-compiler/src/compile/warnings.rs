//! The warnings of `docs/03-chart-format.md` §3.2, checked on a chart that
//! compiled without errors, in code order (W101 to W110); within a code, in
//! `idx`, `SECT`, `TEXT` or document order as each says. Every check is
//! linear in the chart, or O(n log n), except W107, which compares each pair
//! of categories (at most 4,095).

use std::collections::BTreeSet;

use cavea_format::{self as ccf, NONE, Point};

use super::Unit;
use crate::document::ChartDocument;
use crate::expand::{ExpandedRow, Expansion, Seat};
use crate::finding::{Code, Detail, Findings};
use crate::geometry::{Bands, P};

/// W101 findings listed at most.
const MAX_OVERLAPS: usize = 100;
/// W107: the backgrounds of the built-in light and dark themes (04 §19), and
/// the non-text contrast every category colour should reach on both (11 §6).
const BACKGROUNDS: [(&str, [u8; 3]); 2] =
    [("light", [0xFA, 0xFA, 0xF9]), ("dark", [0x1C, 0x19, 0x17])];
const MIN_CONTRAST: f64 = 3.0;
/// W107: two colours closer than this "redmean" distance are too similar.
const MIN_DISTANCE: f64 = 40.0;
/// W108: the reference viewport, the fit padding (04 §13) and the smallest
/// text size, all in CSS pixels.
const VIEWPORT: [f64; 2] = [1280.0, 720.0];
const FIT_PADDING: f64 = 24.0;
const MIN_TEXT_PX: f64 = 10.0;
/// W110, in quarter units: 200,000 cu.
const MAX_EXTENT: i64 = 800_000;

/// What the warnings look at: the document, the compiled chart, and the map
/// from each `idx` back to its node.
pub(super) struct Scene<'a, 'd> {
    pub doc: &'d ChartDocument,
    pub units: &'a [Unit<'d>],
    pub expansion: &'a Expansion<'d>,
    /// Per `idx`: the seat in the expansion.
    pub seat_of: &'a [u32],
    /// Sections with an explicit outline: `SECT` index, node, world outline.
    pub explicit: &'a [(u32, u32, Vec<P>)],
    /// Per `TEXT` record: its node.
    pub text_nodes: &'a [u32],
    pub chart: &'a ccf::Chart,
}

impl<'d> Scene<'_, 'd> {
    pub fn check(&self, f: &mut Findings) {
        self.overlaps(f);
        self.focal_points(f);
        self.single_seats(f);
        self.outside_outlines(f);
        self.categories(f);
        self.texts(f);
        for unit in self.units {
            if let Some(s) = unit.section
                && s.label.as_deref().is_none_or(str::is_empty)
            {
                f.push(Code::W109, Some(s.id), Detail::NoSectionLabel);
            }
        }
        let [x0, y0, x1, y1] = self.chart.meta.bounds.map(i64::from);
        let extent = (x1 - x0).max(y1 - y0);
        if extent > MAX_EXTENT {
            let extent = (extent / 4) as u32; // < 2³²
            f.push(Code::W110, None, Detail::ChartTooLarge { extent });
        }
    }

    fn seat(&self, idx: u32) -> Option<(&ExpandedRow<'d>, &Seat<'d>)> {
        let e = self.expansion;
        let seat = e.seats.get(*self.seat_of.get(idx as usize)? as usize)?;
        Some((e.rows.get(seat.row as usize)?, seat))
    }

    fn node(&self, idx: u32) -> u32 {
        self.seat(idx).map_or(0, |(row, _)| row.node)
    }

    /// W101: seats on one floor whose centres are closer than 0.6 seat
    /// sizes, once per node (the first such seat in `idx` order), at most
    /// [`MAX_OVERLAPS`]. Positions are quantised, so the test is exact:
    /// `25 d² < 9 s²` in quarter units. Seats go into square cells small
    /// enough that two seats in one cell always overlap; a seat alone in its
    /// cell is compared with the seats of the cells within reach of it. A
    /// cell is within reach of at most (2R + 1)² cells, so after the sort the
    /// work is linear, however the seats are packed.
    fn overlaps(&self, f: &mut Findings) {
        let c = self.chart;
        let s = i128::from(c.meta.seat_size);
        let limit = 9 * s * s;
        if limit == 0 {
            return;
        }
        let close = |a: Point, b: Point| {
            let d = |i: usize| i128::from(a[i]) - i128::from(b[i]);
            let (dx, dy) = (d(0), d(1));
            25 * (dx * dx + dy * dy) < limit
        };
        // The largest offset that can overlap, and a cell side whose diagonal
        // cannot reach: 50 (side − 1)² < 9 s².
        let reach_q = isqrt_below(limit, 25);
        // Both below 2¹⁶ for a seat size below 2¹⁶ quarter units.
        let side = isqrt_below(limit, 50) as i32 + 1;
        let reach = reach_q as i32 / side + 1;
        let mut keys: Vec<(u32, i32, i32, u32)> = Vec::with_capacity(c.seat_pos.len());
        for (fl, floor) in (0u32..).zip(&c.floors) {
            for idx in floor.seat_first..floor.seat_first + floor.seat_count {
                if let Some(&[x, y]) = c.seat_pos.get(idx as usize) {
                    keys.push((fl, y.div_euclid(side), x.div_euclid(side), idx));
                }
            }
        }
        keys.sort_unstable();
        let cells: Vec<&[(u32, i32, i32, u32)]> = keys
            .chunk_by(|a, b| (a.0, a.1, a.2) == (b.0, b.1, b.2))
            .collect();
        let pos = |idx: u32| c.seat_pos.get(idx as usize).copied().unwrap_or_default();
        let mut partner = vec![NONE; c.seat_pos.len()];
        let set = |partner: &mut Vec<u32>, a: u32, b: u32| {
            if let Some(p) = partner.get_mut(a as usize)
                && *p == NONE
            {
                *p = b;
            }
        };
        // One cursor per row of cells within reach: the first cell a seat
        // could reach in that row only moves forward as the cells do.
        // Neighbouring cells in `i64`: a cell at the edge of the range has
        // none beyond it, but its neighbours' coordinates must not overflow.
        let key = |cell: &[(u32, i32, i32, u32)]| {
            cell.first().map(|k| (k.0, i64::from(k.1), i64::from(k.2)))
        };
        let reach = i64::from(reach);
        let mut cursors = vec![0usize; (2 * reach + 1) as usize];
        for &cell in &cells {
            match cell {
                [] => {}
                &[(floor, cy, cx, i)] => {
                    let (cy, cx) = (i64::from(cy), i64::from(cx));
                    if partner.get(i as usize) != Some(&NONE) {
                        continue;
                    }
                    'search: for (y, cur) in (cy - reach..).zip(cursors.iter_mut()) {
                        let from = (floor, y, cx - reach);
                        while cells
                            .get(*cur)
                            .and_then(|c| key(c))
                            .is_some_and(|k| k < from)
                        {
                            *cur += 1;
                        }
                        for other in cells.get(*cur..).unwrap_or_default() {
                            match key(other) {
                                Some((of, oy, ox))
                                    if (of, oy) == (floor, y) && ox <= cx + reach => {}
                                _ => break,
                            }
                            for &(.., j) in *other {
                                if j != i && close(pos(i), pos(j)) {
                                    set(&mut partner, i, j);
                                    set(&mut partner, j, i);
                                    break 'search;
                                }
                            }
                        }
                    }
                }
                [first, second, ..] => {
                    for &(.., i) in cell {
                        let other = if i == first.3 { second.3 } else { first.3 };
                        set(&mut partner, i, other);
                    }
                }
            }
        }
        let mut reported = BTreeSet::new();
        for (idx, &other) in (0u32..).zip(&partner) {
            if reported.len() == MAX_OVERLAPS {
                break;
            }
            if other == NONE {
                continue;
            }
            let node = self.node(idx);
            if reported.insert(node) {
                let other = self.node(other);
                f.push(Code::W101, Some(node), Detail::Overlap { other });
            }
        }
    }

    /// W102: a floor with seats and no focal point, in document order.
    fn focal_points(&self, f: &mut Findings) {
        for (i, fl) in self.chart.floors.iter().enumerate() {
            if fl.seat_count > 0 && fl.focal.is_none() {
                let floor = self.doc.floors.get(i).map(|d| d.id);
                f.push(Code::W102, None, Detail::NoFocalPoint { floor });
            }
        }
    }

    /// W103: a row with exactly one seat, once per node, in `idx` order.
    fn single_seats(&self, f: &mut Findings) {
        let mut reported = BTreeSet::new();
        for row in self.chart.rows.iter().filter(|r| r.seat_count == 1) {
            if let Some((r, _)) = self.seat(row.seat_first)
                && reported.insert(r.node)
            {
                f.push(Code::W103, Some(r.node), Detail::SingleSeat { rid: r.rid });
            }
        }
    }

    /// W104: a seat whose centre is not inside its section's explicit world
    /// outline (a centre on the outline is outside), once per node, by
    /// section in `SECT` order, then `idx`.
    fn outside_outlines(&self, f: &mut Findings) {
        let mut reported = BTreeSet::new();
        for (index, section, outline) in self.explicit {
            let Some(s) = self.chart.sections.get(*index as usize) else {
                continue;
            };
            let bands = Bands::new(outline);
            for idx in s.seat_first..s.seat_first + s.seat_count {
                let Some(&[x, y]) = self.chart.seat_pos.get(idx as usize) else {
                    continue;
                };
                let p = [f64::from(x) / 4.0, f64::from(y) / 4.0];
                if bands.contains(p) {
                    continue;
                }
                if let Some((row, seat)) = self.seat(idx)
                    && reported.insert(row.node)
                {
                    let detail = Detail::OutsideOutline {
                        section: *section,
                        rid: row.rid,
                        position: seat.index,
                    };
                    f.push(Code::W104, Some(row.node), detail);
                }
            }
        }
    }

    /// W105, W106 and W107, by category in document order.
    fn categories(&self, f: &mut Findings) {
        let c = self.chart;
        let mut used = vec![false; c.categories.len()];
        for &k in &c.seat_category {
            if let Some(u) = used.get_mut(usize::from(k)) {
                *u = true;
            }
        }
        // Without errors, compiled categories are the document's, in order.
        let keys = || self.doc.categories.iter().map(|k| k.key.clone());
        for (key, _) in keys().zip(&used).filter(|(_, u)| !**u) {
            f.push(Code::W105, None, Detail::UnusedCategory { key });
        }
        let accessible = c.seat_flags.iter().zip(&c.seat_category).any(|(&fl, &k)| {
            fl & 1 != 0
                || c.categories
                    .get(usize::from(k))
                    .is_some_and(|x| x.flags & 1 != 0)
        });
        if !c.seat_pos.is_empty() && !accessible {
            f.push(Code::W106, None, Detail::NoAccessibleSeat);
        }
        let rgb: Vec<[u8; 3]> = c
            .categories
            .iter()
            .map(|x| [x.color[0], x.color[1], x.color[2]])
            .collect();
        let keys: Vec<String> = keys().collect();
        for (j, (a, key)) in rgb.iter().zip(&keys).enumerate() {
            let earlier = rgb.get(..j).unwrap_or_default();
            if let Some(i) = earlier.iter().position(|b| too_similar(*a, *b)) {
                let other = keys.get(i).cloned().unwrap_or_default();
                let key = key.clone();
                f.push(Code::W107, None, Detail::SimilarColors { key, other });
            }
            let l = luminance(*a);
            let worst = BACKGROUNDS
                .iter()
                .map(|&(name, bg)| (contrast(l, luminance(bg)), name))
                .min_by(|x, y| x.0.total_cmp(&y.0));
            if let Some((ratio, background)) = worst
                && ratio < MIN_CONTRAST
            {
                let detail = Detail::LowContrast {
                    key: key.clone(),
                    background,
                    ratio: (ratio * 100.0) as u32, // < 2,100
                };
                f.push(Code::W107, None, detail);
            }
        }
    }

    /// W108: a text shown at its floor's fit zoom (its `minZoom` at most the
    /// fit zoom) whose size there is below [`MIN_TEXT_PX`], in `TEXT` order.
    /// The fit zoom is that of 04 §13 on the reference viewport: the floor's
    /// bounds, padded 24 px, fill 1280 × 720 CSS px.
    fn texts(&self, f: &mut Findings) {
        let c = self.chart;
        for (t, &node) in c.texts.iter().zip(self.text_nodes) {
            let Some(fl) = c.floors.get(t.floor as usize) else {
                continue;
            };
            let [x0, y0, x1, y1] = fl.bounds.map(f64::from);
            let room = |px: f64, extent: f64| {
                if extent > 0.0 {
                    (px - 2.0 * FIT_PADDING) / (extent / 4.0)
                } else {
                    f64::INFINITY
                }
            };
            let fit = room(VIEWPORT[0], x1 - x0).min(room(VIEWPORT[1], y1 - y0));
            let px = f64::from(t.size) / 4.0 * fit;
            if fit.is_finite() && f64::from(t.min_zoom) <= fit && px < MIN_TEXT_PX {
                let px = (px * 100.0) as u32; // < 1,000
                f.push(Code::W108, Some(node), Detail::TextTooSmall { px });
            }
        }
    }
}

/// The largest `v ≥ 0` with `k v² < limit`, for `limit > 0`.
fn isqrt_below(limit: i128, k: i128) -> i128 {
    let mut v = (limit as f64 / k as f64).sqrt() as i128;
    while v > 0 && k * v * v >= limit {
        v -= 1;
    }
    while k * (v + 1) * (v + 1) < limit {
        v += 1;
    }
    v
}

/// W107: the "redmean" distance between two sRGB colours (a low-cost
/// approximation of perceived difference), below [`MIN_DISTANCE`]. Exact in
/// `f64`.
fn too_similar(a: [u8; 3], b: [u8; 3]) -> bool {
    let d = |i: usize| f64::from(a[i]) - f64::from(b[i]);
    let r = (f64::from(a[0]) + f64::from(b[0])) / 2.0;
    let d2 = (2.0 + r / 256.0) * d(0) * d(0)
        + 4.0 * d(1) * d(1)
        + (2.0 + (255.0 - r) / 256.0) * d(2) * d(2);
    d2 < MIN_DISTANCE * MIN_DISTANCE
}

/// WCAG 2 relative luminance, by the exact sRGB curve, as the renderer's
/// themes compute it (04 §19), in `f64`.
fn luminance([r, g, b]: [u8; 3]) -> f64 {
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// An sRGB channel as linear light: `t^2.4` is `t² · t^0.4`, and `t^0.4` the
/// root of `y⁵ = t²` by Newton's method from `√t`, so no `powf` is needed.
/// Six steps reach `f64` precision for every `t` the curve uses (0.09 to 1).
fn linear(v: u8) -> f64 {
    let c = f64::from(v) / 255.0;
    if c <= 0.040_45 {
        return c / 12.92;
    }
    let t = (c + 0.055) / 1.055;
    let t2 = t * t;
    let mut y = t.sqrt();
    for _ in 0..6 {
        y = 0.8 * y + 0.2 * t2 / (y * y * y * y);
    }
    t2 * y
}

/// The WCAG 2 contrast ratio of two luminances.
fn contrast(a: f64, b: f64) -> f64 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[cfg(test)]
mod tests;
