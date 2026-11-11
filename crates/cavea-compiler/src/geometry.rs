//! Section outlines, label anchors and label sizes (`docs/03-chart-format.md`
//! §2.3, §3.1 step 11).
//!
//! The automatic outline is a closing of the seats on a square grid: cells near
//! a seat are filled, the filled region is grown and shrunk again so that aisles
//! and gaps close, and its outer boundary is traced and simplified. Grid
//! coordinates are whole half-cells, so tracing and simplification work on
//! integers held exactly in `f64`. Everything uses only `+ − × ÷ √`, `floor`
//! and `ceil`, so every target gets the same bits.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// A point in chart units.
pub(crate) type P = [f64; 2];

/// Points in one explicit outline.
pub(crate) const MAX_POINTS: usize = 4_096;
/// Points of all explicit outlines in one chart.
pub(crate) const MAX_OUTLINE_POINTS: usize = 262_144;
/// Character advance and line height of the label text model, in ems (§3.1.1).
pub(crate) const ADVANCE: f64 = 0.6;
pub(crate) const LINE_HEIGHT: f64 = 1.2;
/// Cells the pole-of-inaccessibility search may evaluate.
const POLE_CELLS: usize = 1_024;

/// What is wrong with an explicit outline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutlineError {
    NotFinite,
    TooManyPoints,
    ZeroSize,
    SelfIntersecting,
}

/// An explicit outline after validation (see [`check`]).
pub(crate) fn explicit_outline(points: &[[f64; 2]]) -> Result<Vec<P>, OutlineError> {
    if !points.iter().flatten().all(|v| v.is_finite()) {
        return Err(OutlineError::NotFinite);
    }
    if points.len() > MAX_POINTS {
        return Err(OutlineError::TooManyPoints);
    }
    let poly = dedupe(points.iter().copied());
    check(&poly)?;
    Ok(poly)
}

/// The points with consecutive repeats, and a last point equal to the first,
/// dropped.
pub(crate) fn dedupe(points: impl Iterator<Item = P>) -> Vec<P> {
    let mut poly: Vec<P> = Vec::new();
    for p in points {
        if poly.last() != Some(&p) {
            poly.push(p);
        }
    }
    while poly.len() > 1 && poly.first() == poly.last() {
        poly.pop();
    }
    poly
}

/// A valid outline: at least three points not all on one line, and no two
/// edges touching except neighbours at their shared point.
pub(crate) fn check(poly: &[P]) -> Result<(), OutlineError> {
    if flat(poly) {
        return Err(OutlineError::ZeroSize);
    }
    if !is_simple(poly) {
        return Err(OutlineError::SelfIntersecting);
    }
    Ok(())
}

/// True for fewer than three points, or all on the line through the first
/// two (which must differ).
pub(crate) fn flat(poly: &[P]) -> bool {
    match poly {
        [a, b, _, ..] => poly.iter().all(|&p| cross(*a, *b, p) == 0.0),
        _ => true,
    }
}

/// The automatic outline of a section's seats, in the section's own space,
/// with at least `padding` between every seat centre and the outline. Empty
/// when there are no seats; `None` when the grid's numbers overflow or the
/// grid would exceed 2²⁶ cells.
pub fn auto_outline(seats: &[P], padding: f64) -> Option<Vec<P>> {
    let Some(&first) = seats.first() else {
        return Some(Vec::new());
    };
    let (mut lo, mut hi) = (first, first);
    for s in seats {
        lo = [lo[0].min(s[0]), lo[1].min(s[1])];
        hi = [hi[0].max(s[0]), hi[1].max(s[1])];
    }
    let (w, h_) = (hi[0] - lo[0], hi[1] - lo[1]);
    let p = padding.max(1.0);
    // Cell size: a third of the padding, coarser for sparse sections so that
    // the grid stays within a few dozen cells per seat.
    let h = (p / 3.0).max((w + h_) / (8.0 * (seats.len() as f64).sqrt() + 32.0));
    // Closing radius in cells, and the drawing radius: padding, plus the
    // tracing and simplification error, plus the closing radius.
    let k = (2.0 * p / h).ceil();
    let r = p + 2.0625 * h + k * h;
    let margin = (r / h).ceil() + 1.0;
    let cols = (w / h).ceil() + 2.0 * margin + 1.0;
    let rows = (h_ / h).ceil() + 2.0 * margin + 1.0;
    if !(h.is_finite() && r.is_finite() && cols * rows <= f64::from(1u32 << 26)) {
        return None;
    }
    let g = Grid {
        cols: cols as usize,
        rows: rows as usize,
        origin: [lo[0] - margin * h, lo[1] - margin * h],
        h,
    };
    let filled = g.disks(seats, r);
    let closed = g.erode(&filled, k as usize);
    let inside = g.fill_holes(&closed);

    let traced = g
        .single_component(&inside)
        .then(|| g.trace(&inside))
        .flatten();
    let poly = traced
        .and_then(|ring| simplify_closed(&ring))
        .filter(|p| p.len() >= 3 && p.len() <= MAX_POINTS && is_simple(p))
        .unwrap_or_else(|| convex_hull(g.boundary_midpoints(&inside)));
    // Half-cell lattice to section space.
    let half = h / 2.0;
    let out: Vec<P> = poly
        .iter()
        .map(|&[x, y]| [g.origin[0] + x * half, g.origin[1] + y * half])
        .collect();
    out.iter().flatten().all(|v| v.is_finite()).then_some(out)
}

/// A grid of square cells of side `h`; cell (i, j) covers
/// `origin + [i, i + 1] × [j, j + 1] × h`.
struct Grid {
    cols: usize,
    rows: usize,
    origin: P,
    h: f64,
}

impl Grid {
    /// Cells whose centre is within `r` of a seat. The grid's margin keeps
    /// every such cell inside it.
    fn disks(&self, seats: &[P], r: f64) -> Vec<bool> {
        let mut cells = vec![false; self.cols * self.rows];
        let [ox, oy] = self.origin;
        let h = self.h;
        let rr = r * r;
        for &[sx, sy] in seats {
            // Seat position in cells, from the first cell's centre.
            let (cx, cy) = ((sx - ox) / h - 0.5, (sy - oy) / h - 0.5);
            let reach = r / h;
            let (j0, j1) = ((cy - reach).ceil().max(0.0), (cy + reach).floor());
            let mut j = j0 as usize;
            while (j as f64) <= j1 && j < self.rows {
                let dy = (j as f64 - cy) * h;
                let rem = rr - dy * dy;
                if rem >= 0.0 {
                    let dx = rem.sqrt() / h;
                    let (i0, i1) = ((cx - dx).ceil().max(0.0), (cx + dx).floor());
                    if i1 >= i0 {
                        let row = j * self.cols;
                        let (a, b) = (row + i0 as usize, row + (i1 as usize).min(self.cols - 1));
                        if let Some(run) = cells.get_mut(a..=b) {
                            run.fill(true);
                        }
                    }
                }
                j += 1;
            }
        }
        cells
    }

    /// Erosion by a disc of radius `k` cells (k ≤ 6, since the cell is at
    /// least a third of the padding): a cell stays filled when no empty
    /// cell (or cell outside the grid) is within `k` cells of it, measured
    /// between centres.
    fn erode(&self, cells: &[bool], k: usize) -> Vec<bool> {
        let (cols, rows) = (self.cols, self.rows);
        let cap = u8::try_from(k + 1).unwrap_or(u8::MAX);
        // Per row: distance to the nearest empty cell in that row, capped.
        let mut run = vec![0u8; cells.len()];
        for (src, dst) in cells.chunks(cols).zip(run.chunks_mut(cols)) {
            let mut d = 0;
            for (&c, out) in src.iter().zip(dst.iter_mut()) {
                d = if c { (d + 1).min(cap) } else { 0 };
                *out = d;
            }
            d = 0;
            for (&c, out) in src.iter().zip(dst.iter_mut()).rev() {
                d = if c { (d + 1).min(cap) } else { 0 };
                *out = (*out).min(d);
            }
        }
        // A cell survives if, for every row offset t within k, the nearest
        // empty cell along that row is farther than √(k² − t²).
        let kk = k * k;
        let mut out = cells.to_vec();
        for t in 0..=2 * k {
            let dj = t.abs_diff(k);
            for (j, row) in out.chunks_mut(cols).enumerate() {
                let other = (j + t)
                    .checked_sub(k)
                    .filter(|&jj| jj < rows)
                    .and_then(|jj| run.get(jj * cols..(jj + 1) * cols));
                match other {
                    Some(g) => {
                        for (o, &g) in row.iter_mut().zip(g) {
                            let g = usize::from(g);
                            *o &= g * g + dj * dj > kk;
                        }
                    }
                    None => row.fill(false),
                }
            }
        }
        out
    }

    /// Filled cells plus every empty cell not 4-connected to the grid's edge.
    fn fill_holes(&self, cells: &[bool]) -> Vec<bool> {
        let (cols, rows) = (self.cols, self.rows);
        let mut outside = vec![false; cells.len()];
        let mut stack: Vec<usize> = Vec::new();
        let visit = |c: usize, outside: &mut Vec<bool>, stack: &mut Vec<usize>| {
            if !cells.get(c).copied().unwrap_or(true) && !outside[c] {
                outside[c] = true;
                stack.push(c);
            }
        };
        for i in 0..cols {
            visit(i, &mut outside, &mut stack);
            visit((rows - 1) * cols + i, &mut outside, &mut stack);
        }
        for j in 0..rows {
            visit(j * cols, &mut outside, &mut stack);
            visit(j * cols + cols - 1, &mut outside, &mut stack);
        }
        while let Some(c) = stack.pop() {
            let (i, j) = (c % cols, c / cols);
            if i > 0 {
                visit(c - 1, &mut outside, &mut stack);
            }
            if i + 1 < cols {
                visit(c + 1, &mut outside, &mut stack);
            }
            if j > 0 {
                visit(c - cols, &mut outside, &mut stack);
            }
            if j + 1 < rows {
                visit(c + cols, &mut outside, &mut stack);
            }
        }
        outside.into_iter().map(|o| !o).collect()
    }

    fn at(&self, cells: &[bool], i: i64, j: i64) -> bool {
        let (Ok(i), Ok(j)) = (usize::try_from(i), usize::try_from(j)) else {
            return false;
        };
        i < self.cols && j < self.rows && cells.get(j * self.cols + i) == Some(&true)
    }

    /// True if the filled cells form one 8-connected component, for a grid
    /// without holes whose edge cells are empty. By Gray's bit-quad count,
    /// components − holes = (Q1 − Q3 − 2 QD) / 4 over all 2 × 2 windows, where
    /// Q1 and Q3 hold one or three filled cells and QD two on a diagonal.
    fn single_component(&self, cells: &[bool]) -> bool {
        let mut euler = 0i64;
        for (top, bottom) in cells.chunks(self.cols).zip(cells.chunks(self.cols).skip(1)) {
            for i in 1..self.cols {
                let q = [top[i - 1], top[i], bottom[i - 1], bottom[i]];
                euler += match q.iter().filter(|&&c| c).count() {
                    1 => 1,
                    3 => -1,
                    2 if q[0] == q[3] => -2,
                    _ => 0,
                };
            }
        }
        euler == 4
    }

    /// The outer boundary of the filled region (8-connected), walked with the
    /// region on the right, as the midpoints of its cell edges in half-cell
    /// units. `None` if the walk does not close within its bound.
    fn trace(&self, cells: &[bool]) -> Option<Vec<P>> {
        let start = cells.iter().position(|&c| c)?;
        let (sx, sy) = ((start % self.cols) as i64, (start / self.cols) as i64);
        let (mut x, mut y, mut dx, mut dy) = (sx, sy, 1i64, 0i64);
        let mut ring = Vec::new();
        let limit = 4 * cells.len() + 4;
        loop {
            ring.push([(2 * x + dx) as f64, (2 * y + dy) as f64]);
            (x, y) = (x + dx, y + dy);
            let (left, right) = ((dy, -dx), (-dy, dx));
            let cell =
                |(lx, ly): (i64, i64)| self.at(cells, x + (dx + lx - 1) / 2, y + (dy + ly - 1) / 2);
            if cell(left) {
                (dx, dy) = left;
            } else if !cell(right) {
                (dx, dy) = right;
            }
            if (x, y, dx, dy) == (sx, sy, 1, 0) {
                return Some(ring);
            }
            if ring.len() > limit {
                return None;
            }
        }
    }

    /// Midpoints of every edge between a filled and an empty cell, in
    /// half-cell units.
    fn boundary_midpoints(&self, cells: &[bool]) -> Vec<P> {
        let mut out = Vec::new();
        for j in 0..self.rows as i64 {
            for i in 0..self.cols as i64 {
                if !self.at(cells, i, j) {
                    continue;
                }
                let (x, y) = ((2 * i + 1) as f64, (2 * j + 1) as f64);
                for (di, dj) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
                    if !self.at(cells, i + di, j + dj) {
                        out.push([x + di as f64, y + dj as f64]);
                    }
                }
            }
        }
        out
    }
}

/// Douglas–Peucker over a closed ring with tolerance one cell, split at
/// the first point and the point farthest from it. Distances are to the
/// segment. `None` if the work exceeds its bound (64 steps per point).
fn simplify_closed(ring: &[P]) -> Option<Vec<P>> {
    let n = ring.len();
    let first = *ring.first()?;
    let far = (1..n).fold(0, |best, i| {
        if dist2(ring[i], first) > dist2(ring[best], first) {
            i
        } else {
            best
        }
    });
    if far == 0 {
        return None;
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[far] = true;
    let mut budget = 64 * n + 1_024;
    let mut stack = vec![(0, far), (far, n)];
    while let Some((a, b)) = stack.pop() {
        let (pa, pb) = (ring[a], ring[b % n]);
        let mut best: Option<(f64, usize)> = None;
        for (i, &q) in ring.iter().enumerate().take(b).skip(a + 1) {
            let d = seg_dist2(q, pa, pb);
            if best.is_none_or(|(bd, _)| d > bd) {
                best = Some((d, i));
            }
        }
        budget = budget.checked_sub(b - a)?;
        if let Some((d, i)) = best
            && d > 4.0
        {
            keep[i] = true;
            stack.push((i, b));
            stack.push((a, i));
        }
    }
    Some(
        ring.iter()
            .zip(&keep)
            .filter(|(_, k)| **k)
            .map(|(p, _)| *p)
            .collect(),
    )
}

/// Convex hull by Andrew's monotone chain, without collinear points.
fn convex_hull(mut pts: Vec<P>) -> Vec<P> {
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let mut hull: Vec<P> = Vec::with_capacity(2 * pts.len());
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &P>> = if pass == 0 {
            Box::new(pts.iter())
        } else {
            Box::new(pts.iter().rev())
        };
        for &p in iter {
            while hull.len() >= start + 2 {
                let (a, b) = (hull[hull.len() - 2], hull[hull.len() - 1]);
                if cross(a, b, p) <= 0.0 {
                    hull.pop();
                } else {
                    break;
                }
            }
            hull.push(p);
        }
        hull.pop();
    }
    hull
}

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}

fn dist2(a: P, b: P) -> f64 {
    let d = sub(a, b);
    d[0] * d[0] + d[1] * d[1]
}

/// (b − a) × (c − a): positive when a, b, c turn clockwise on screen (y down).
fn cross(a: P, b: P, c: P) -> f64 {
    let (u, v) = (sub(b, a), sub(c, a));
    u[0] * v[1] - u[1] * v[0]
}

/// Squared distance from `p` to the segment `a`–`b`.
fn seg_dist2(p: P, a: P, b: P) -> f64 {
    let (ab, ap) = (sub(b, a), sub(p, a));
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = ap[0] * ab[0] + ap[1] * ab[1];
    if len2 == 0.0 || t <= 0.0 {
        dist2(p, a)
    } else if t >= len2 {
        dist2(p, b)
    } else {
        let c = ab[0] * ap[1] - ab[1] * ap[0];
        c * c / len2
    }
}

fn on_segment(p: P, a: P, b: P) -> bool {
    p[0] >= a[0].min(b[0])
        && p[0] <= a[0].max(b[0])
        && p[1] >= a[1].min(b[1])
        && p[1] <= a[1].max(b[1])
}

/// True if the closed segments `a`–`b` and `c`–`d` share a point.
fn segments_meet(a: P, b: P, c: P, d: P) -> bool {
    let (d1, d2) = (cross(c, d, a), cross(c, d, b));
    let (d3, d4) = (cross(a, b, c), cross(a, b, d));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    (d1 == 0.0 && on_segment(a, c, d))
        || (d2 == 0.0 && on_segment(b, c, d))
        || (d3 == 0.0 && on_segment(c, a, b))
        || (d4 == 0.0 && on_segment(d, a, b))
}

/// True if no two edges of the closed polygon meet, except neighbouring
/// edges at their shared point (and those do not fold back onto each other).
pub(crate) fn is_simple(poly: &[P]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let edge = |i: usize| (poly[i], poly[(i + 1) % n]);
    let boxes: Vec<[f64; 4]> = (0..n)
        .map(|i| {
            let (a, b) = edge(i);
            [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[0].max(b[0]),
                a[1].max(b[1]),
            ]
        })
        .collect();
    for i in 0..n {
        let (a, b) = edge(i);
        // A neighbour folding back: the next edge leaves b along a–b.
        let c = poly[(i + 2) % n];
        if cross(a, b, c) == 0.0
            && (c[0] - b[0]) * (a[0] - b[0]) + (c[1] - b[1]) * (a[1] - b[1]) > 0.0
        {
            return false;
        }
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            let (p, q) = (boxes[i], boxes[j]);
            if p[0] > q[2] || q[0] > p[2] || p[1] > q[3] || q[1] > p[3] {
                continue;
            }
            let (c, d) = edge(j);
            if segments_meet(a, b, c, d) {
                return false;
            }
        }
    }
    true
}

/// Twice the signed area of a closed ring, positive when it turns clockwise
/// on screen (y down). Taken relative to the first point: differences of
/// values `f32` holds are exact in `f64`, so far from the origin nothing
/// cancels.
pub(crate) fn area2(ring: &[P]) -> f64 {
    let Some(&o) = ring.first() else {
        return 0.0;
    };
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (a, b) = (sub(ring[i], o), sub(ring[(i + 1) % n], o));
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

/// True if every ring is simple ([`is_simple`]) and no two rings meet.
pub(crate) fn rings_simple(rings: &[Vec<P>]) -> bool {
    fn edges(r: &[P]) -> impl Iterator<Item = (P, P)> + '_ {
        let n = r.len();
        (0..n).map(move |i| (r[i], r[(i + 1) % n]))
    }
    let bbox = |(a, b): (P, P)| {
        [
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[0].max(b[0]),
            a[1].max(b[1]),
        ]
    };
    for (i, ring) in rings.iter().enumerate() {
        if !is_simple(ring) {
            return false;
        }
        for other in rings.iter().skip(i + 1) {
            for e in edges(ring) {
                let p = bbox(e);
                for f in edges(other) {
                    let q = bbox(f);
                    if p[0] <= q[2]
                        && q[0] <= p[2]
                        && p[1] <= q[3]
                        && q[1] <= p[3]
                        && segments_meet(e.0, e.1, f.0, f.1)
                    {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// Twice the area the even-odd rule fills for rings that pass
/// [`rings_simple`]: each ring's area, counted negative when it lies inside
/// an odd number of the others.
pub(crate) fn even_odd_area2(rings: &[Vec<P>]) -> f64 {
    rings
        .iter()
        .enumerate()
        .map(|(i, ring)| {
            let p = ring.first().copied().unwrap_or_default();
            let depth = rings
                .iter()
                .enumerate()
                .filter(|&(j, other)| j != i && contains(other, p))
                .count();
            let a = area2(ring).abs();
            if depth % 2 == 0 { a } else { -a }
        })
        .sum()
}

/// True if `p` is inside the polygon (crossing rule).
fn contains(poly: &[P], p: P) -> bool {
    let n = poly.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
    }
    inside
}

/// Vertices whose `y` falls strictly inside one band of [`Bands`], at most.
const BAND_VERTICES: usize = 32;

/// The crossing-rule test of [`contains`] for many points against one simple
/// polygon of `n` points, in O(log n + 64) per point after O(n² / 32) set-up.
/// Horizontal bands start at every 32nd vertex `y` in sorted order. An edge
/// that crosses a band whole is listed in it by `x`, which is one order
/// across the band since edges of a simple polygon do not cross; an edge
/// ending inside a band is listed apart, and a band holds at most 62 such.
pub(crate) struct Bands<'a> {
    poly: &'a [P],
    /// Band `k` is `limits[k] ≤ y < limits[k + 1]`.
    limits: Vec<f64>,
    /// Per band: the edges crossing it whole, by `x`; the other edges in it.
    whole: Vec<Vec<usize>>,
    part: Vec<Vec<usize>>,
}

impl<'a> Bands<'a> {
    pub fn new(poly: &'a [P]) -> Self {
        let mut ys: Vec<f64> = poly.iter().map(|p| p[1]).collect();
        ys.sort_by(f64::total_cmp);
        let mut limits: Vec<f64> = ys.iter().step_by(BAND_VERTICES).copied().collect();
        limits.extend(ys.last());
        limits.dedup();
        let bands = limits.len().saturating_sub(1);
        let (mut whole, mut part) = (vec![Vec::new(); bands], vec![Vec::new(); bands]);
        let mut b = Self {
            poly,
            limits,
            whole: Vec::new(),
            part: Vec::new(),
        };
        for i in 0..poly.len() {
            let (a, c) = b.edge(i);
            let (lo, hi) = (a[1].min(c[1]), a[1].max(c[1]));
            let first = b.limits.partition_point(|&y| y <= lo).saturating_sub(1);
            for (k, pair) in b.limits.windows(2).enumerate().skip(first) {
                let &[y0, y1] = pair else { break };
                if y0 >= hi {
                    break;
                }
                let list = if lo <= y0 && hi >= y1 {
                    &mut whole
                } else {
                    &mut part
                };
                if let Some(list) = list.get_mut(k) {
                    list.push(i);
                }
            }
        }
        for (k, list) in whole.iter_mut().enumerate() {
            let mid = match b.limits.get(k..k + 2) {
                Some(&[y0, y1]) => (y0 + y1) / 2.0,
                _ => continue,
            };
            list.sort_by(|&i, &j| b.x_at(i, mid).total_cmp(&b.x_at(j, mid)));
        }
        b.whole = whole;
        b.part = part;
        b
    }

    fn edge(&self, i: usize) -> (P, P) {
        let n = self.poly.len();
        let at = |k: usize| self.poly.get(k % n.max(1)).copied().unwrap_or_default();
        (at(i), at(i + 1))
    }

    /// Where edge `i` meets the line at height `y`, as [`contains`] computes it.
    fn x_at(&self, i: usize, y: f64) -> f64 {
        let (a, b) = self.edge(i);
        (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0]
    }

    pub fn contains(&self, p: P) -> bool {
        let k = self.limits.partition_point(|&y| y <= p[1]);
        let (Some(whole), Some(part)) = (
            self.whole.get(k.wrapping_sub(1)),
            self.part.get(k.wrapping_sub(1)),
        ) else {
            return false;
        };
        let left = whole.partition_point(|&i| p[0] >= self.x_at(i, p[1]));
        let mut inside = (whole.len() - left) % 2 == 1;
        for &i in part {
            let (a, b) = self.edge(i);
            if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < self.x_at(i, p[1]) {
                inside = !inside;
            }
        }
        inside
    }
}

/// Distance from `p` to the polygon's boundary, negative outside.
fn signed_distance(poly: &[P], p: P) -> f64 {
    let n = poly.len();
    let d2 = (0..n)
        .map(|i| seg_dist2(p, poly[i], poly[(i + 1) % n]))
        .fold(f64::INFINITY, f64::min);
    let d = d2.sqrt();
    if contains(poly, p) { d } else { -d }
}

#[derive(Clone, Copy)]
struct Cell {
    c: P,
    half: f64,
    d: f64,
    max: f64,
    seq: usize,
}

impl Cell {
    fn new(poly: &[P], c: P, half: f64, seq: usize) -> Self {
        let d = signed_distance(poly, c);
        Self {
            c,
            half,
            d,
            max: d + half * std::f64::consts::SQRT_2,
            seq,
        }
    }
}

impl PartialEq for Cell {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Cell {}
impl PartialOrd for Cell {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Cell {
    /// Highest potential first; among equals, the earlier cell.
    fn cmp(&self, other: &Self) -> Ordering {
        self.max
            .total_cmp(&other.max)
            .then(other.seq.cmp(&self.seq))
    }
}

/// The pole of inaccessibility: the interior point farthest from the
/// boundary, by the quadtree search of Mapbox's polylabel, to within
/// max(0.5 cu, (width + height) / 512), evaluating at most 1,024 cells.
/// `seed`, a point strictly inside, is a candidate too, so the result is
/// always inside.
pub(crate) fn pole(poly: &[P], seed: P) -> P {
    let Some(&first) = poly.first() else {
        return [0.0, 0.0];
    };
    let (mut lo, mut hi) = (first, first);
    for p in poly {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
    let size = w.min(h).max(w.max(h) / 256.0);
    if !(size > 0.0 && size.is_finite()) {
        return seed;
    }
    let precision = 0.5f64.max((w + h) / 512.0);
    let half = size / 2.0;
    let mut seq = 0;
    let mut heap = BinaryHeap::new();
    let (nx, ny) = ((w / size).ceil() as usize, (h / size).ceil() as usize);
    for i in 0..nx {
        for j in 0..ny {
            let c = [
                lo[0] + i as f64 * size + half,
                lo[1] + j as f64 * size + half,
            ];
            heap.push(Cell::new(poly, c, half, seq));
            seq += 1;
        }
    }
    let mut best = Cell::new(poly, centroid(poly).unwrap_or(first), 0.0, seq);
    for (i, c) in [[lo[0] + w / 2.0, lo[1] + h / 2.0], seed]
        .into_iter()
        .enumerate()
    {
        let cell = Cell::new(poly, c, 0.0, seq + 1 + i);
        if cell.d > best.d {
            best = cell;
        }
    }
    seq += 3;
    while let Some(cell) = heap.pop() {
        if cell.d > best.d {
            best = cell;
        }
        if cell.max - best.d <= precision || seq >= POLE_CELLS {
            continue;
        }
        let q = cell.half / 2.0;
        for [sx, sy] in [[-1.0, -1.0], [1.0, -1.0], [-1.0, 1.0], [1.0, 1.0]] {
            let c = [cell.c[0] + sx * q, cell.c[1] + sy * q];
            heap.push(Cell::new(poly, c, q, seq));
            seq += 1;
        }
    }
    best.c
}

fn centroid(poly: &[P]) -> Option<P> {
    let n = poly.len();
    let (mut x, mut y, mut a2) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        let f = p[0] * q[1] - q[0] * p[1];
        x += (p[0] + q[0]) * f;
        y += (p[1] + q[1]) * f;
        a2 += f;
    }
    (a2 != 0.0).then(|| [x / (3.0 * a2), y / (3.0 * a2)])
}

/// True if the closed box `[x0, y0, x1, y1]` meets the closed segment `a`–`b`
/// (Liang–Barsky).
fn segment_meets_box(a: P, b: P, [x0, y0, x1, y1]: [f64; 4]) -> bool {
    let d = sub(b, a);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [
        (-d[0], a[0] - x0),
        (d[0], x1 - a[0]),
        (-d[1], a[1] - y0),
        (d[1], y1 - a[1]),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

/// The largest label size, in quarter units, at which a label of `chars`
/// characters fits inside the polygon centred on `anchor` (§3.1.1): the box
/// 0.6 × chars × size wide and 1.2 × size tall, axis-aligned, meets no edge.
/// 0 without characters or with the anchor outside.
pub(crate) fn label_size(poly: &[P], anchor: P, chars: usize) -> u16 {
    if chars == 0 || !contains(poly, anchor) {
        return 0;
    }
    let n = poly.len();
    let fits = |q: u32| {
        let s = f64::from(q) / 4.0;
        let (hw, hh) = (ADVANCE * chars as f64 * s / 2.0, LINE_HEIGHT * s / 2.0);
        let b = [
            anchor[0] - hw,
            anchor[1] - hh,
            anchor[0] + hw,
            anchor[1] + hh,
        ];
        (0..n).all(|i| !segment_meets_box(poly[i], poly[(i + 1) % n], b))
    };
    if !fits(0) {
        return 0;
    }
    let (mut lo, mut hi) = (0u32, 65_536u32);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo as u16 // < 65,536
}

#[cfg(test)]
mod tests;
