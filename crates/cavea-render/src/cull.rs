//! Chunk culling and draw ranges (`docs/04-renderer.md` §11).

use std::ops::Range;

use cavea_format::RenderChunk;

/// Writes into `out` (cleared first) the seat ranges to draw: the render chunks
/// within `active` whose box, grown by `pad` chart units, meets `view` (min x,
/// min y, max x, max y in chart units), with chunks adjacent in `idx` order
/// merged into one range. While there are more than `max` ranges, the two
/// separated by the fewest seats are merged, which draws those seats too:
/// some overdraw instead of more draw calls. Allocates nothing once `out` has
/// grown to the number of chunks.
pub fn ranges(
    chunks: &[RenderChunk],
    active: &Range<u32>,
    view: [f32; 4],
    pad: f32,
    max: usize,
    out: &mut Vec<Range<u32>>,
) {
    out.clear();
    let [vx0, vy0, vx1, vy1] = view;
    for chunk in chunks {
        let seats = chunk.seat_first..chunk.seat_first + chunk.seat_count;
        if seats.is_empty() || seats.start < active.start || seats.end > active.end {
            continue;
        }
        let [x0, y0, x1, y1] = chunk.bounds.map(|v| v as f32 * 0.25);
        if x1 + pad < vx0 || x0 - pad > vx1 || y1 + pad < vy0 || y0 - pad > vy1 {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.end == seats.start => last.end = seats.end,
            _ => out.push(seats),
        }
    }
    while out.len() > max.max(1) {
        let smallest = out
            .windows(2)
            .enumerate()
            .min_by_key(|(_, pair)| match pair {
                [a, b] => b.start - a.end,
                _ => u32::MAX,
            })
            .map(|(i, _)| i);
        let Some(i) = smallest else { break };
        let end = out.get(i + 1).map_or(0, |r| r.end);
        if let Some(r) = out.get_mut(i) {
            r.end = end;
        }
        out.remove(i + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// xorshift64, so the cases never depend on a crate version.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: u32) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % u64::from(n)) as u32
        }
    }

    #[test]
    fn ranges_cover_visible_chunks_within_the_limit() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut out = Vec::new();
        for _ in 0..2000 {
            // Chunks on a grid of 100-unit cells, partitioning the seats.
            let mut chunks = Vec::new();
            let mut next = 0;
            for _ in 0..1 + rng.below(200) {
                let count = 1 + rng.below(4096);
                let (x, y) = (rng.below(40) as i32 * 400, rng.below(40) as i32 * 400);
                chunks.push(RenderChunk {
                    section: 0,
                    seat_first: next,
                    seat_count: count,
                    bounds: [x, y, x + 400, y + 400],
                });
                next += count;
            }
            // Floors start at chunk boundaries.
            let first = chunks[rng.below(chunks.len() as u32) as usize].seat_first;
            let active = first..next;
            let (x, y) = (rng.below(40) as f32 * 100.0, rng.below(40) as f32 * 100.0);
            let view = [x, y, x + rng.below(2000) as f32, y + rng.below(2000) as f32];
            let max = 1 + rng.below(70) as usize;
            ranges(&chunks, &active, view, 0.0, max, &mut out);

            assert!(out.len() <= max);
            assert!(out.windows(2).all(|w| w[0].end < w[1].start), "{out:?}");
            let mut visible = 0;
            for c in &chunks {
                let seats = c.seat_first..c.seat_first + c.seat_count;
                let [x0, y0, x1, y1] = c.bounds.map(|v| v as f32 * 0.25);
                let meets = x1 >= view[0] && x0 <= view[2] && y1 >= view[1] && y0 <= view[3];
                let drawn = out
                    .iter()
                    .any(|r| r.start <= seats.start && seats.end <= r.end);
                if meets && seats.start >= active.start {
                    visible += 1;
                    assert!(drawn, "visible chunk {seats:?} not drawn: {out:?}");
                }
                if seats.start < active.start {
                    assert!(!out.iter().any(|r| r.start < active.start));
                }
            }
            if visible == 0 {
                assert!(out.is_empty());
            }
            // Every chunk visible: one range of the active seats.
            ranges(&chunks, &active, [-1e9, -1e9, 1e9, 1e9], 0.0, max, &mut out);
            assert_eq!(out, std::slice::from_ref(&active));
        }
    }
}
