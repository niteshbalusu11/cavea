//! Hierarchical hit testing (04 §15): sections whose box, grown by the
//! largest tolerance, holds the point; their rows whose box, grown by the
//! row's tolerance, holds it; and in those rows the seat with the nearest
//! centre. Rows outside any section are tested directly. Removed positions are
//! not in the compiled chart, so every seat found is a real one.

use cavea_format::Bounds;

use crate::scene::Scene;

/// The active floor's seat nearest `p` (chart units) among those within
/// tolerance of it: the larger of the seat's half size and `tolerance` (chart
/// units). Equal distances go to the lower `idx`.
pub fn hit_test(scene: &Scene, p: [f32; 2], tolerance: f32) -> Option<u32> {
    let chart = &scene.chart;
    // No seat's tolerance exceeds this, so a section box grown by it holds
    // every point within tolerance of one of the section's seats.
    let reach = scene.max_half.max(tolerance);
    let mut best: Option<(f32, u32)> = None;
    let mut row = |r: u32| {
        let Some(row) = chart.rows.get(r as usize) else {
            return;
        };
        let tol = (f32::from(row.seat_size) * 0.125).max(tolerance);
        // `ROWS` boxes bound seat centres; the slack keeps rounding at the
        // box edge from losing a seat the distance test would accept.
        if !contains(row.bounds, p, tol * 1.001) {
            return;
        }
        let seats = row.seat_first..row.seat_first.saturating_add(u32::from(row.seat_count));
        let seats = seats.start.max(scene.active.start)..seats.end.min(scene.active.end);
        for idx in seats {
            let Some(&[x, y]) = scene.pos.get(idx as usize) else {
                return;
            };
            let d2 = distance2([x, y], p);
            if d2 <= tol * tol && best.is_none_or(|b| (d2, idx) < b) {
                best = Some((d2, idx));
            }
        }
    };
    for section in &chart.sections {
        if section.floor == scene.floor && contains(section.bounds, p, reach) {
            let first = section.row_first;
            (first..first.saturating_add(section.row_count)).for_each(&mut row);
        }
    }
    scene.loose_rows.iter().copied().for_each(row);
    best.map(|(_, idx)| idx)
}

/// Squared distance, as both the hierarchical and the brute-force search
/// compute it.
pub fn distance2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
    dx * dx + dy * dy
}

/// Whether the quantised box `b` grown by `grow` chart units holds `p`.
fn contains(b: Bounds, p: [f32; 2], grow: f32) -> bool {
    let [x0, y0, x1, y1] = b.map(|v| v as f32 * 0.25);
    p[0] >= x0 - grow && p[0] <= x1 + grow && p[1] >= y0 - grow && p[1] <= y1 + grow
}
