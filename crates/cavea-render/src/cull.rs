//! Chunk culling and draw ranges (`docs/04-renderer.md` §11).

use std::ops::Range;

use cavea_format::RenderChunk;

/// Writes into `out` (cleared first) the seat ranges to draw: the parts within
/// `active` of the render chunks whose box, grown by `pad` chart units, meets
/// `view` (min x, min y, max x, max y in chart units), with chunks adjacent in
/// `idx` order merged into one range. While there are more than `max` ranges,
/// the two separated by the fewest seats are merged, which draws those seats
/// too: some overdraw instead of more draw calls. `chunks` partition the seats
/// in `idx` order (`cavea_format` checks it), so the ranges come out sorted and
/// disjoint. `visible` (cleared first) gets the indices of the chunks drawn,
/// before merging: the seat labels' draws (04 §12). Allocates nothing once
/// the vectors have grown to the number of chunks.
pub fn ranges(
    chunks: &[RenderChunk],
    active: &Range<u32>,
    view: [f32; 4],
    pad: f32,
    max: usize,
    out: &mut Vec<Range<u32>>,
    visible: &mut Vec<u32>,
) {
    out.clear();
    visible.clear();
    let [vx0, vy0, vx1, vy1] = view;
    for (i, chunk) in (0u32..).zip(chunks) {
        let end = chunk.seat_first.saturating_add(chunk.seat_count);
        let seats = chunk.seat_first.max(active.start)..end.min(active.end);
        if seats.is_empty() {
            continue;
        }
        let [x0, y0, x1, y1] = chunk.bounds.map(|v| v as f32 * 0.25);
        if x1 + pad < vx0 || x0 - pad > vx1 || y1 + pad < vy0 || y0 - pad > vy1 {
            continue;
        }
        visible.push(i);
        match out.last_mut() {
            Some(last) if last.end == seats.start => last.end = seats.end,
            _ => out.push(seats),
        }
    }
    merge_smallest_gaps(out, max.max(1));
}

/// Merges `ranges` (sorted, disjoint) down to `max` by joining across the
/// smallest gaps, ties in `idx` order. Merging across one gap leaves the
/// others unchanged, so this is the same as repeatedly merging the two ranges
/// separated by the fewest seats, but linear: the threshold gap is found by
/// binary search over gap sizes, then the ranges are compacted in one pass.
fn merge_smallest_gaps(ranges: &mut Vec<Range<u32>>, max: usize) {
    let Some(merges) = ranges.len().checked_sub(max).filter(|&m| m > 0) else {
        return;
    };
    let at_most = |limit: u32| ranges.windows(2).filter(|w| gap(w) <= limit).count();
    // The smallest gap size `t` with at least `merges` gaps of `t` or fewer.
    let (mut lo, mut hi) = (0u32, u32::MAX);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if at_most(mid) >= merges {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let threshold = lo;
    let mut ties = merges - threshold.checked_sub(1).map_or(0, at_most);
    // `w` is the range being grown. Merging only extends `end` to a later
    // range's end, so `ranges[r - 1].end` is still that range's own end.
    let mut w = 0;
    for r in 1..ranges.len() {
        let Some(g) = ranges.get(r - 1..=r).map(gap) else {
            break;
        };
        let merge = g < threshold || (g == threshold && ties > 0);
        if merge && g == threshold {
            ties -= 1;
        }
        let next = ranges.get(r).cloned().unwrap_or_default();
        if merge {
            if let Some(grown) = ranges.get_mut(w) {
                grown.end = next.end;
            }
        } else {
            w += 1;
            if let Some(slot) = ranges.get_mut(w) {
                *slot = next;
            }
        }
    }
    ranges.truncate(w + 1);
}

/// Seats between the two ranges of `pair`.
fn gap(pair: &[Range<u32>]) -> u32 {
    match pair {
        [a, b] => b.start.saturating_sub(a.end),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule as 04 §11 states it: merge the two ranges separated by the
    /// fewest seats (the first such pair on ties) until `max` are left.
    fn naive(ranges: &mut Vec<Range<u32>>, max: usize) {
        while ranges.len() > max {
            let i = (0..ranges.len() - 1)
                .min_by_key(|&i| gap(&ranges[i..i + 2]))
                .unwrap();
            ranges[i].end = ranges[i + 1].end;
            ranges.remove(i + 1);
        }
    }

    #[test]
    fn merging_matches_the_stated_rule() {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut below = |n: u32| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % u64::from(n)) as u32
        };
        for _ in 0..3000 {
            let mut ranges = Vec::new();
            let mut at = 0;
            for _ in 0..1 + below(150) {
                // Small gaps so that ties are common.
                at += 1 + below(6);
                let len = 1 + below(5);
                ranges.push(at..at + len);
                at += len;
            }
            let max = 1 + below(80) as usize;
            let mut expected = ranges.clone();
            naive(&mut expected, max);
            merge_smallest_gaps(&mut ranges, max);
            assert_eq!(ranges, expected);
        }
    }
}
