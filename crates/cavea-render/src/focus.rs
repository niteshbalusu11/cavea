//! Keyboard focus search (`docs/04-renderer.md` §15, `docs/11-accessibility.md`
//! §3): the seat an arrow key moves the virtual focus to. The rule, in screen
//! directions (the camera has no rotation, so a screen direction is the same
//! direction in chart units, y down) from the focused seat `f`:
//!
//! 1. **Along the row.** Of `f`'s neighbours in its row (the seats before and
//!    after it in `idx` order, across a gap between segments), one that lies
//!    within 45° of the arrow's direction.
//! 2. **In the cone.** Otherwise, of the seats of the floor's other rows
//!    within 45° of the direction, the one with the lowest
//!    `along + 2 × |across|`: `along` its distance in the arrow's direction,
//!    `across` its offset perpendicular.
//! 3. **In the half-plane.** Otherwise, the same over the seats of other rows
//!    strictly ahead (`along > 0`).
//! 4. Otherwise `f` is at the edge in that direction: `None`.
//!
//! Equal scores go to the lower `idx`, so the result depends only on the
//! chart, the floor shown and `f`. Sections, then rows, whose boxes cannot
//! beat the best seat found are skipped, so a step reads a few rows, not
//! every seat.

use cavea_format::Bounds;

use crate::scene::Scene;

/// An arrow key's direction on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// How much an offset across the direction counts against a seat, per unit
/// along it.
const ACROSS_WEIGHT: f32 = 2.0;

/// `v` as (along the direction, across it): with `u` the direction's unit
/// vector, its dot product and a perpendicular one (whose sign the callers
/// ignore). Branch-free, so the loops are not copied per direction.
#[inline]
fn axes(u: [f32; 2], [x, y]: [f32; 2]) -> (f32, f32) {
    (x * u[0] + y * u[1], x * u[1] + y * u[0])
}

impl Dir {
    fn unit(self) -> [f32; 2] {
        match self {
            Dir::Right => [1.0, 0.0],
            Dir::Left => [-1.0, 0.0],
            Dir::Down => [0.0, 1.0],
            Dir::Up => [0.0, -1.0],
        }
    }
}

/// The seat an arrow in `dir` moves the focus to from `from`, or `None` at
/// the edge of the floor shown in that direction.
pub fn step(scene: &Scene, from: u32, dir: Dir) -> Option<u32> {
    let u = dir.unit();
    let p = *scene.pos.get(from as usize)?;
    let offset = |idx: u32| {
        let q = scene.pos.get(idx as usize)?;
        Some(axes(u, [q[0] - p[0], q[1] - p[1]]))
    };
    let row = scene
        .chart
        .rows
        .get(*scene.row.get(from as usize)? as usize)?;
    let seats = row.seat_first..row.seat_first.saturating_add(u32::from(row.seat_count));
    let mut best: Option<(f32, u32)> = None;
    for n in [from.checked_sub(1), from.checked_add(1)]
        .into_iter()
        .flatten()
    {
        if !seats.contains(&n) || !scene.active.contains(&n) {
            continue;
        }
        if let Some((along, across)) = offset(n)
            && along > 0.0
            && across.abs() <= along
        {
            consider(&mut best, along + ACROSS_WEIGHT * across.abs(), n);
        }
    }
    if let Some((_, n)) = best {
        return Some(n);
    }
    search(scene, p, from, u, true).or_else(|| search(scene, p, from, u, false))
}

/// The best seat outside `from`'s row ahead of `p` in `dir`: within 45° of
/// it with `cone`, else anywhere ahead.
#[inline(never)]
fn search(scene: &Scene, p: [f32; 2], from: u32, u: [f32; 2], cone: bool) -> Option<u32> {
    let chart = &scene.chart;
    let mut best: Option<(f32, u32)> = None;
    // The lowest score any point of the box could have, or `None` if no
    // point of it is ahead.
    let bound = |b: Bounds| {
        let [x0, y0, x1, y1] = b.map(|v| v as f32 * 0.25);
        let (a0, c0) = axes(u, [x0 - p[0], y0 - p[1]]);
        let (a1, c1) = axes(u, [x1 - p[0], y1 - p[1]]);
        let (along_min, along_max) = (a0.min(a1), a0.max(a1));
        if along_max <= 0.0 {
            return None;
        }
        let (c_min, c_max) = (c0.min(c1), c0.max(c1));
        let across = if c_min > 0.0 {
            c_min
        } else if c_max < 0.0 {
            -c_max
        } else {
            0.0
        };
        Some(along_min.max(0.0) + ACROSS_WEIGHT * across)
    };
    let beaten = |lower: f32, best: &Option<(f32, u32)>| best.is_some_and(|(s, _)| lower > s);
    let own_row = scene.row.get(from as usize).copied();
    let row = |r: u32, best: &mut Option<(f32, u32)>| {
        let Some(row) = chart.rows.get(r as usize).filter(|_| Some(r) != own_row) else {
            return;
        };
        match bound(row.bounds) {
            Some(lower) if !beaten(lower, best) => {}
            _ => return,
        }
        let seats = row.seat_first..row.seat_first.saturating_add(u32::from(row.seat_count));
        for idx in seats.start.max(scene.active.start)..seats.end.min(scene.active.end) {
            let Some(q) = scene.pos.get(idx as usize) else {
                return;
            };
            let (along, across) = axes(u, [q[0] - p[0], q[1] - p[1]]);
            if along > 0.0 && (!cone || across.abs() <= along) {
                consider(best, along + ACROSS_WEIGHT * across.abs(), idx);
            }
        }
    };
    // `from`'s own section first: the best seat is usually there, and the
    // bound then skips most of the others.
    let own = scene.section_of(from as usize);
    let sections = chart.sections.iter().enumerate();
    let order = sections
        .clone()
        .filter(|&(i, _)| i as u32 == own)
        .chain(sections.filter(|&(i, _)| i as u32 != own));
    for (_, section) in order {
        if section.floor != scene.floor {
            continue;
        }
        match bound(section.bounds) {
            Some(lower) if !beaten(lower, &best) => {}
            _ => continue,
        }
        let first = section.row_first;
        for r in first..first.saturating_add(section.row_count) {
            row(r, &mut best);
        }
    }
    for &r in &scene.loose_rows {
        row(r, &mut best);
    }
    best.map(|(_, idx)| idx)
}

/// The seat of the floor shown nearest `p` (chart units), equal distances to
/// the lower `idx`: where the focus starts when there is none. The hit test's
/// search, near `p` first, else over every row.
pub fn nearest(scene: &Scene, p: [f32; 2]) -> Option<u32> {
    crate::hit::hit_test(scene, p, 4.0 * scene.seat_size)
        .or_else(|| crate::hit::hit_test(scene, p, f32::INFINITY))
}

/// Keeps the lower of `best` and (`score`, `idx`), ties to the lower `idx`.
#[inline]
fn consider(best: &mut Option<(f32, u32)>, score: f32, idx: u32) {
    if best.is_none_or(|b| (score, idx) < b) {
        *best = Some((score, idx));
    }
}

#[cfg(test)]
mod tests;
