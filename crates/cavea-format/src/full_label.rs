//! Seat labels and full labels from a decoded chart (`docs/03-chart-format.md`
//! §4.2.3), as every consumer of a compiled chart produces them: the renderer
//! for its events, the server to resolve an API label to an `idx`. Nothing here
//! allocates except [`LabelIndex::build`].
//!
//! Tables, booths and areas are a later milestone: only seats have labels here.

use core::fmt::{self, Write};

use cavea_core::label::{LabelBuf, LabelError, LabelRule, Scheme, seat_ordinal};
use cavea_core::template::write_full_label;

use crate::{Chart, NONE};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FullLabelError {
    /// The `idx` is not a seat of the chart.
    NotASeat,
    /// The seat's row has no label rule and `LBLX` does not list the seat.
    Unlabelled,
    Label(LabelError),
}

impl fmt::Display for FullLabelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotASeat => f.write_str("not a seat"),
            Self::Unlabelled => f.write_str("seat has no label"),
            Self::Label(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for FullLabelError {}

/// The parts a seat's full label is composed of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeatLabel<'c> {
    /// Empty for a row outside any section or in a section without a label.
    pub section: &'c str,
    pub row: &'c str,
    pub seat: LabelBuf,
}

impl SeatLabel<'_> {
    /// The full label, from the chart's template (`META`).
    pub fn full(&self, chart: &Chart) -> Result<LabelBuf, FullLabelError> {
        let template = chart.strings.get(chart.meta.label_template);
        let mut out = LabelBuf::new();
        write_full_label(
            template.unwrap_or_default(),
            self.section,
            self.row,
            self.seat.as_str(),
            &mut out,
        )
        .map_err(|_| FullLabelError::Label(LabelError::TooLong))?;
        Ok(out)
    }
}

/// The index of the last record whose first seat is at or before `idx`.
fn containing(firsts: impl Fn(usize) -> u32, len: usize, idx: u32) -> Option<usize> {
    // Binary search over a partition of the seats.
    let (mut lo, mut hi) = (0, len);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if firsts(mid) <= idx {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo.checked_sub(1)
}

/// The section, row and seat label of seat `idx`: the seat's `LBLX` entry or
/// its row's rule at its ordinal.
pub fn seat_label(chart: &Chart, idx: u32) -> Result<SeatLabel<'_>, FullLabelError> {
    let text = |i: u32| {
        if i == NONE {
            ""
        } else {
            chart.strings.get(i).unwrap_or_default()
        }
    };
    if idx >= chart.meta.seat_count {
        return Err(FullLabelError::NotASeat);
    }
    let rows = &chart.rows;
    let row = containing(|i| rows.get(i).map_or(0, |r| r.seat_first), rows.len(), idx)
        .and_then(|i| rows.get(i))
        .filter(|r| idx - r.seat_first < u32::from(r.seat_count))
        .ok_or(FullLabelError::NotASeat)?;
    let section = match chart.sections.get(row.section as usize) {
        Some(s) => text(s.label),
        None => "",
    };
    let label_err = FullLabelError::Label;
    let mut seat = LabelBuf::new();
    let exceptions = &chart.label_exceptions;
    if let Ok(i) = exceptions.binary_search_by_key(&idx, |x| x.idx) {
        let label = exceptions.get(i).map_or("", |x| text(x.label));
        seat = LabelBuf::try_from(label).map_err(label_err)?;
    } else {
        let rule = chart
            .label_rules
            .get(row.label_rule as usize)
            .ok_or(FullLabelError::Unlabelled)?;
        let segs = &chart.segments;
        let segment = containing(|i| segs.get(i).map_or(0, |s| s.seat_first), segs.len(), idx)
            .and_then(|i| segs.get(i))
            .ok_or(FullLabelError::NotASeat)?;
        let position = u32::from(segment.position).saturating_add(idx - segment.seat_first);
        let ordinal = row.ordinal_base.saturating_add(seat_ordinal(
            row.direction.into(),
            0,
            row.positions.into(),
            position,
        ));
        let (prefix, suffix) = (text(rule.prefix), text(rule.suffix));
        if rule.scheme == Scheme::Custom {
            // `LabelRule` takes owned custom lists; the decoded chart keeps
            // them as string indices, so write the entry directly.
            let entry = (ordinal < rule.custom_count)
                .then(|| chart.strings.get(rule.custom_first.saturating_add(ordinal)))
                .flatten()
                .ok_or(FullLabelError::Label(LabelError::CustomExhausted))?;
            let too_long = |_| FullLabelError::Label(LabelError::TooLong);
            seat.write_str(prefix).map_err(too_long)?;
            seat.write_str(entry).map_err(too_long)?;
            seat.write_str(suffix).map_err(too_long)?;
        } else {
            LabelRule {
                scheme: rule.scheme,
                start: rule.start,
                step: rule.step,
                overflow: rule.overflow,
                prefix,
                suffix,
                custom: &[],
            }
            .write(ordinal, &mut seat)
            .map_err(label_err)?;
        }
    }
    Ok(SeatLabel {
        section,
        row: text(row.label),
        seat,
    })
}

/// Writes seat `idx`'s full label.
pub fn full_label(chart: &Chart, idx: u32) -> Result<LabelBuf, FullLabelError> {
    seat_label(chart, idx)?.full(chart)
}

/// Full label to seat `idx`: an open-addressing table of `idx + 1` (0 is
/// empty) keyed by a 32-bit FNV-1a hash of the label, with at least twice as
/// many slots as seats: 8 bytes per seat at most. A lookup probes from the
/// label's slot and confirms each candidate by writing its label, so a
/// collision costs one more label, never a wrong answer. No sorting, so no
/// sort code in the WASM. Built only by a consumer that needs it.
#[derive(Clone, Debug, Default)]
pub struct LabelIndex {
    slots: Vec<u32>,
}

fn hash(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}

impl LabelIndex {
    /// One pass over the seats; seats whose label cannot be written are left
    /// out (a compiled chart has none).
    pub fn build(chart: &Chart) -> Self {
        let len = (chart.meta.seat_count as usize * 2)
            .next_power_of_two()
            .max(2);
        let mut slots = vec![0; len];
        let mask = len - 1;
        for idx in 0..chart.meta.seat_count {
            let Ok(label) = full_label(chart, idx) else {
                continue;
            };
            let mut at = hash(label.as_str()) as usize & mask;
            // The table is at most half full, so an empty slot is near.
            while slots.get(at).is_some_and(|&s| s != 0) {
                at = (at + 1) & mask;
            }
            if let Some(slot) = slots.get_mut(at) {
                *slot = idx + 1;
            }
        }
        Self { slots }
    }

    /// The seat whose full label is `label`.
    pub fn find(&self, chart: &Chart, label: &str) -> Option<u32> {
        let mask = self.slots.len().checked_sub(1)?;
        let mut at = hash(label) as usize & mask;
        loop {
            let idx = self.slots.get(at)?.checked_sub(1)?;
            if full_label(chart, idx).is_ok_and(|l| l.as_str() == label) {
                return Some(idx);
            }
            at = (at + 1) & mask;
        }
    }
}
