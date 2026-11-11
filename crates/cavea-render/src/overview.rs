//! Level of detail and the overview (`docs/04-renderer.md` §10): which level a
//! seat size on screen calls for, the cross-fade between section fills and
//! seats, per-section availability counters (04 §5) and the section tints
//! written into the section outlines' vertex colours.

use std::ops::Range;

use cavea_format::{Chart, NONE, Strings};

use crate::{
    protocol::Lod,
    theme::{Rgba, Theme, visual},
    tuning::{LOD_CROSSFADE_SEAT_PX, LOD_DETAIL_SEAT_PX, LOD_HYSTERESIS, LOD_SEATS_SEAT_PX},
};

const LEVELS: [Lod; 4] = [Lod::Overview, Lod::CrossFade, Lod::Seats, Lod::Detail];

/// The level for seats `s` CSS pixels large, coming from `previous` (`None`
/// after a chart load). A level is entered upwards when `s` reaches its
/// threshold and left downwards only when `s` falls below `LOD_HYSTERESIS`
/// times it, so a zoom resting near a threshold keeps its level. A chart
/// without section fills on the active floor has no overview or cross-fade.
pub fn level(previous: Option<Lod>, s: f64, sections: bool) -> Lod {
    let thresholds = [LOD_CROSSFADE_SEAT_PX, LOD_SEATS_SEAT_PX, LOD_DETAIL_SEAT_PX];
    let up = thresholds.iter().filter(|&&t| s >= t).count();
    let down = thresholds
        .iter()
        .filter(|&&t| s >= t * LOD_HYSTERESIS)
        .count();
    let mut at = match previous.and_then(|p| LEVELS.iter().position(|&l| l == p)) {
        Some(p) => p.clamp(up, down),
        None => up,
    };
    if !sections {
        at = at.max(2);
    }
    LEVELS.get(at).copied().unwrap_or(Lod::Detail)
}

/// The seats' opacity at seat size `s`; the section fills take the rest. It
/// rises linearly from 0 at `LOD_CROSSFADE_SEAT_PX` to 1 at the lower edge of
/// the seats threshold's hysteresis band, so it is 0 at overview and 1 at the
/// seats and detail levels whichever way the level was reached: no level
/// change makes anything jump. Without section fills, seats are always opaque.
pub fn seat_alpha(s: f64, sections: bool) -> f32 {
    if !sections {
        return 1.0;
    }
    let end = LOD_SEATS_SEAT_PX * LOD_HYSTERESIS;
    let t = (s - LOD_CROSSFADE_SEAT_PX) / (end - LOD_CROSSFADE_SEAT_PX);
    t.clamp(0.0, 1.0) as f32
}

/// Per-section availability (04 §5) and the overview tint (04 §10).
#[derive(Debug, Default)]
pub struct Sections {
    /// Per section: its seats not drawn unavailable (available, or selected
    /// by the buyer).
    pub open: Vec<u32>,
    /// Per section: the category most of its seats have (the lower on a tie),
    /// or `NONE` for a section without seats.
    dominant: Vec<u32>,
    /// Per category: its colour in the current theme.
    colours: Vec<Rgba>,
    /// Sections whose tint is out of date, each once; `stale` flags them.
    changed: Vec<u32>,
    stale: Vec<bool>,
    /// The index range of the active floor's section outlines: one draw.
    pub fills: Range<u32>,
}

impl Sections {
    /// Counters for `visual` (all open at load), dominant categories, and the
    /// active floor's outline range. One pass over the seats.
    pub fn new(chart: &Chart, floor: u32, visual: &[u8], theme: &Theme) -> Self {
        let mut counts = vec![0u32; chart.categories.len()];
        let dominant = chart
            .sections
            .iter()
            .map(|s| {
                let seats = chart
                    .seat_category
                    .get(s.seat_first as usize..(s.seat_first + s.seat_count) as usize)
                    .unwrap_or_default();
                for &c in seats {
                    if let Some(n) = counts.get_mut(usize::from(c)) {
                        *n += 1;
                    }
                }
                let mut best = (0, NONE);
                for &c in seats {
                    if let Some(n) = counts.get_mut(usize::from(c)) {
                        let c = u32::from(c);
                        if *n > best.0 || (*n == best.0 && *n > 0 && c < best.1) {
                            best = (*n, c);
                        }
                    }
                }
                for &c in seats {
                    if let Some(n) = counts.get_mut(usize::from(c)) {
                        *n = 0;
                    }
                }
                best.1
            })
            .collect();
        let mut fills: Option<Range<u32>> = None;
        for s in chart.sections.iter().filter(|s| s.floor == floor) {
            let r = s.mesh_first..s.mesh_first + s.mesh_count;
            fills = Some(fills.map_or(r.clone(), |f| f.start.min(r.start)..f.end.max(r.end)));
        }
        let n = chart.sections.len();
        let mut sections = Self {
            open: vec![0; n],
            dominant,
            colours: Vec::new(),
            changed: Vec::with_capacity(n),
            stale: vec![false; n],
            fills: fills.filter(|f| !f.is_empty()).unwrap_or(0..0),
        };
        sections.set_theme(chart, theme);
        sections.recount(chart, visual);
        sections
    }

    /// Whether the active floor has section fills to draw.
    pub fn any(&self) -> bool {
        !self.fills.is_empty()
    }

    /// Counts every section's open seats again (after a snapshot): one pass
    /// over the rows' seat ranges. Marks every tint stale.
    pub fn recount(&mut self, chart: &Chart, visual: &[u8]) {
        self.open.fill(0);
        for row in &chart.rows {
            let first = row.seat_first as usize;
            let seats = visual
                .get(first..first + usize::from(row.seat_count))
                .unwrap_or_default();
            if let Some(n) = self.open.get_mut(row.section as usize) {
                *n += seats.iter().filter(|&&v| v != visual::UNAVAILABLE).count() as u32;
            }
        }
        self.all_stale();
    }

    /// One seat of `section` (or `NONE`) went from visual state `was` to `now`.
    #[inline]
    pub fn change(&mut self, section: u32, was: u8, now: u8) {
        let (was, now) = (was != visual::UNAVAILABLE, now != visual::UNAVAILABLE);
        if was == now {
            return;
        }
        let i = section as usize;
        if let (Some(n), Some(stale)) = (self.open.get_mut(i), self.stale.get_mut(i)) {
            *n = if now { *n + 1 } else { n.saturating_sub(1) };
            if !*stale {
                *stale = true;
                self.changed.push(section);
            }
        }
    }

    /// Takes the theme's category colours and marks every tint stale.
    pub fn set_theme(&mut self, chart: &Chart, theme: &Theme) {
        self.colours.clear();
        let strings: &Strings = &chart.strings;
        let colours = chart.categories.iter().map(|c| theme.category(c, strings));
        self.colours.extend(colours);
        self.all_stale();
    }

    fn all_stale(&mut self) {
        self.changed.clear();
        self.changed.extend(0..self.open.len() as u32);
        self.stale.fill(true);
    }

    /// Section `i`'s tint: its dominant category's colour mixed into the
    /// theme's sold-out grey (`seat-unavailable-fill`) by the theme's
    /// `section-tint` strength times the share of its seats still open;
    /// opaque. All available: the strength's share of the colour; sold out,
    /// or without seats: the grey. Integer arithmetic, rounded.
    pub fn tint(&self, chart: &Chart, i: usize, theme: &Theme) -> Rgba {
        let grey = theme.unavailable_fill;
        let seats = chart.sections.get(i).map_or(0, |s| s.seat_count);
        let colour = self
            .dominant
            .get(i)
            .and_then(|&c| self.colours.get(c as usize));
        let (Some(colour), true) = (colour, seats > 0) else {
            return [grey[0], grey[1], grey[2], 255];
        };
        let open = self.open.get(i).map_or(0, |&o| o.min(seats));
        // `strength` in 255ths; the colour's weight is strength × open / seats.
        let strength = (theme.section_tint * 255.0 + 0.5) as u64;
        let total = 255 * u64::from(seats);
        let w = strength * u64::from(open);
        let mix = |g: u8, c: u8| {
            ((u64::from(g) * (total - w) + u64::from(c) * w + total / 2) / total) as u8
        };
        [
            mix(grey[0], colour[0]),
            mix(grey[1], colour[1]),
            mix(grey[2], colour[2]),
            255,
        ]
    }

    /// Writes the tint of every stale section into its outline vertices'
    /// colours and returns the vertex range written, if any.
    pub fn write_tints(&mut self, chart: &mut Chart, theme: &Theme) -> Option<Range<usize>> {
        let mut written: Option<Range<usize>> = None;
        let mut changed = std::mem::take(&mut self.changed);
        for &i in &changed {
            let i = i as usize;
            if let Some(stale) = self.stale.get_mut(i) {
                *stale = false;
            }
            let tint = self.tint(chart, i, theme);
            let Some(s) = chart.sections.get(i) else {
                continue;
            };
            let range = s.mesh_first as usize..(s.mesh_first + s.mesh_count) as usize;
            let mesh = &mut chart.mesh;
            for &v in mesh.indices.get(range).unwrap_or_default() {
                if let Some(vertex) = mesh.vertices.get_mut(v as usize) {
                    vertex.color = tint;
                    let v = v as usize;
                    written = Some(written.map_or(v..v + 1, |w| w.start.min(v)..w.end.max(v + 1)));
                }
            }
        }
        changed.clear();
        self.changed = changed;
        written
    }
}
