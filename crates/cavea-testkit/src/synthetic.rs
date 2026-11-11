//! Seeded synthetic charts (`docs/03-chart-format.md` §7). For now one stadium
//! generator; the presets of §7 and `cavea gen` (FMT-024, FMT-025) build on it
//! or replace it.

use std::f64::consts::TAU;

use cavea_compiler::document::{
    BlockRow, Category, ChartDocument, Curve, Node, RowBlock, Section, Settings,
};

/// Seat pitch and row pitch in chart units (03 §1 defaults).
pub const SEAT_PITCH: f64 = 55.0;
pub const ROW_PITCH: f64 = 95.0;
/// Sections around the bowl.
pub const SECTIONS: u32 = 24;
/// Categories, with colours that keep at least 3:1 (WCAG) against light,
/// dark and black backgrounds.
pub const CATEGORIES: [(&str, &str); 4] = [
    ("premium", "#2563EB"),
    ("standard", "#B45309"),
    ("value", "#047857"),
    ("family", "#C026D3"),
];

/// Radius of the front row, in chart units.
const FIELD_RADIUS: f64 = 3000.0;
/// Angle, as a fraction of a section's span, left empty as an aisle.
const AISLE: f64 = 0.04;

/// A stadium of exactly `seats` seats: `SECTIONS` sections around a round
/// field, each one row block of concentric arcs, so every row in the chart is
/// a curved row of the same ring. Rings are filled from the front; the last
/// ring is shared out between the sections. The seed picks each section's
/// category; the same arguments always give the same document.
pub fn stadium(seats: u32, seed: u64) -> ChartDocument {
    let mut rng = SplitMix64(seed);
    let mut rows: Vec<Vec<BlockRow>> = vec![Vec::new(); SECTIONS as usize];
    let mut next_oid = 1;
    let mut left = seats;
    let mut ring = 0;
    while left > 0 {
        let radius = FIELD_RADIUS + f64::from(ring) * ROW_PITCH;
        let span = TAU / f64::from(SECTIONS) * (1.0 - AISLE);
        let per_section = ((radius * span / SEAT_PITCH) as u32).max(1);
        // The last ring: as many seats as are left, spread evenly.
        let even = left / SECTIONS;
        let extra = left % SECTIONS;
        for (section, section_rows) in (0..SECTIONS).zip(&mut rows) {
            let n = if left >= per_section * SECTIONS {
                per_section
            } else {
                even + u32::from(section < extra)
            };
            if n > 0 {
                section_rows.push(BlockRow {
                    rid: next_oid,
                    seats: n,
                    offset: 0.0,
                    oids: vec![[next_oid + 1, n]],
                    label: None,
                    removed: Vec::new(),
                });
                next_oid += 1 + n;
            }
        }
        left -= left.min(per_section * SECTIONS);
        ring += 1;
    }

    let mut nodes = Vec::with_capacity(SECTIONS as usize);
    for (section, rows) in (0..SECTIONS).zip(rows) {
        // The section's centre line, clockwise from +x; the block's front
        // (−y in its own space) faces the field centre at the origin.
        let phi = TAU * (f64::from(section) + 0.5) / f64::from(SECTIONS);
        let (category, _) = CATEGORIES[(rng.next() % CATEGORIES.len() as u64) as usize];
        let block = RowBlock {
            id: next_oid + 1,
            name: None,
            floor: None,
            origin: [0.0, 0.0],
            rotation: 0.0,
            locked: false,
            hidden: false,
            seat_pitch: SEAT_PITCH,
            row_pitch: ROW_PITCH,
            curve: Curve::Arc {
                radius: FIELD_RADIUS,
            },
            align: Default::default(),
            stagger: 0.0,
            skew: 0.0,
            category: None,
            row_labels: Default::default(),
            seat_labels: Default::default(),
            rows,
            seat_overrides: Default::default(),
        };
        nodes.push(Node::Section(Section {
            id: next_oid,
            name: None,
            floor: None,
            origin: [FIELD_RADIUS * phi.cos(), FIELD_RADIUS * phi.sin()],
            rotation: phi.to_degrees() - 90.0,
            locked: false,
            hidden: false,
            label: Some((section + 101).to_string()),
            outline: Default::default(),
            category: Some(category.to_owned()),
            priority: 1.0,
            entrance: None,
            children: vec![Node::RowBlock(block)],
        }));
        next_oid += 2;
    }

    ChartDocument {
        schema: 1,
        name: format!("Synthetic stadium, {seats} seats"),
        settings: Settings::default(),
        next_oid,
        categories: CATEGORIES
            .iter()
            .map(|&(key, color)| Category {
                key: key.to_owned(),
                label: key.to_owned(),
                color: color.to_owned(),
                accessible: false,
            })
            .collect(),
        floors: Vec::new(),
        nodes,
    }
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
    fn compiles_to_exactly_the_seats_asked_for() {
        for n in [1, 23, 999, 100_000] {
            let doc = stadium(n, 7);
            assert_eq!(doc, stadium(n, 7));
            let compiled = cavea_compiler::compile(&doc).unwrap();
            assert_eq!(compiled.stats.seats, n);
        }
    }
}
