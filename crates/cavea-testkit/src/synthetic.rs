//! Seeded synthetic charts (`docs/03-chart-format.md` §7). For now they are raw
//! position and category arrays; compiled charts come with the compiler.

use std::f32::consts::{PI, TAU};

/// Seat pitch, row pitch and seat size in chart units (03 §1 defaults).
pub const SEAT_PITCH: f32 = 55.0;
pub const ROW_PITCH: f32 = 95.0;
pub const SEAT_SIZE: f32 = 44.0;
/// Number of price categories the generator assigns.
pub const CATEGORIES: u16 = 6;

const FIELD_HALF_WIDTH: f32 = 5000.0;
const FIELD_HALF_HEIGHT: f32 = 3300.0;
const ROWS_PER_TIER: u32 = 35;
const CONCOURSE: f32 = 500.0;

/// Seats as structure-of-arrays, indexed by seat.
#[derive(Debug, Clone, PartialEq)]
pub struct SyntheticSeats {
    /// Centres in chart units, quantised to 0.25 cu (03 §1).
    pub pos: Vec<[f32; 2]>,
    pub cat: Vec<u16>,
}

/// A stadium bowl of exactly `seats` seats around an elliptical field: tiers of
/// 35 rows, each tier split into sections by aisles, a concourse between tiers.
/// 100,000 seats fill three tiers (the `stadium-100k` preset). The seed picks
/// each section's category; the same seed always gives the same arrays.
pub fn stadium(seats: usize, seed: u64) -> SyntheticSeats {
    let mut rng = SplitMix64(seed);
    let mut pos = Vec::with_capacity(seats);
    let mut cat = Vec::with_capacity(seats);
    let mut offset = 0.0;
    let mut tier = 0;
    while pos.len() < seats {
        let sections = 24 + 8 * tier;
        let section_cat: Vec<u16> = (0..sections)
            .map(|_| (rng.next() % u64::from(CATEGORIES)) as u16)
            .collect();
        for row in 0..ROWS_PER_TIER {
            let a = FIELD_HALF_WIDTH + offset + row as f32 * ROW_PITCH;
            let b = FIELD_HALF_HEIGHT + offset + row as f32 * ROW_PITCH;
            // Ramanujan's approximation of the ellipse's perimeter.
            let perimeter = PI * (3.0 * (a + b) - ((3.0 * a + b) * (a + 3.0 * b)).sqrt());
            // One seat's width per section is left empty as an aisle.
            let per_section = ((perimeter / sections as f32 / SEAT_PITCH) as u32).saturating_sub(1);
            let span = TAU / sections as f32;
            for (section, &c) in section_cat.iter().enumerate() {
                for seat in 0..per_section {
                    if pos.len() == seats {
                        return SyntheticSeats { pos, cat };
                    }
                    let theta =
                        span * (section as f32 + (seat as f32 + 1.0) / (per_section as f32 + 1.0));
                    let quantise = |v: f32| (v * 4.0).round() / 4.0;
                    pos.push([quantise(a * theta.cos()), quantise(b * theta.sin())]);
                    cat.push(c);
                }
            }
        }
        offset += ROWS_PER_TIER as f32 * ROW_PITCH + CONCOURSE;
        tier += 1;
    }
    SyntheticSeats { pos, cat }
}

/// A tiny deterministic generator, so the output never depends on a crate version.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_count_and_deterministic_for_a_seed() {
        for n in [0, 1, 999, 100_000] {
            let a = stadium(n, 7);
            assert_eq!((a.pos.len(), a.cat.len()), (n, n));
            assert_eq!(a, stadium(n, 7));
        }
        let a = stadium(100_000, 7);
        assert!(a.cat.iter().all(|&c| c < CATEGORIES));
        assert_ne!(a.cat, stadium(100_000, 8).cat);
    }
}
