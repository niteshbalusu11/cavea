//! Row-level and seat-level edits on the selection (`docs/07-designer.md`
//! §5.3), as commands. Each acts only on a row block or row that can be
//! picked (not locked or hidden, on the floor shown); a selection it does
//! not fit gives no command.

use std::collections::BTreeSet;

use cavea_compiler::document::{BlockRow, Curve, Node, RowBlock};

use crate::command::{Command, End, SeatEdit};
use crate::index::{CachedSeat, P};
use crate::selection::{Level, Selection, View};
use crate::tool::handles::targets;

/// The row block whose rows are selected, with the selected rids.
fn rows<'a>(view: &View<'a>, sel: &Selection) -> Option<(u32, &'a RowBlock, Vec<u32>)> {
    let Level::Rows(id) = sel.level() else {
        return None;
    };
    let Some(Node::RowBlock(b)) = view.doc.node(id) else {
        return None;
    };
    let rids: Vec<u32> = sel.items().iter().copied().collect();
    (view.pickable(sel.level(), id) && !rids.is_empty()).then_some((id, b, rids))
}

/// The row block or row whose seats are selected, with the selected oids.
pub(crate) fn seats(view: &View<'_>, sel: &Selection) -> Option<(u32, Vec<u32>)> {
    let Level::Seats(id) = sel.level() else {
        return None;
    };
    let oids: Vec<u32> = sel.items().iter().copied().collect();
    (view.pickable(sel.level(), id) && !oids.is_empty()).then_some((id, oids))
}

/// Adds (`count` above 0) or removes positions at one end of each selected
/// row; the kept seats stay where they are (07 §2.2 `addSeats`).
pub(crate) fn row_seats(view: &View<'_>, sel: &Selection, end: End, count: i32) -> Option<Command> {
    let (id, _, rids) = rows(view, sel)?;
    let k = count.unsigned_abs();
    let commands = rids
        .into_iter()
        .map(|rid| match count > 0 {
            true => Command::AddSeats {
                id,
                rid,
                end,
                count: k,
            },
            false => Command::RemoveSeats {
                id,
                rid,
                end,
                count: k,
            },
        })
        .collect();
    (k > 0).then_some(Command::Composite { commands })
}

/// Shifts the selected rows sideways by `delta` chart units.
pub(crate) fn shift(view: &View<'_>, sel: &Selection, delta: f64) -> Option<Command> {
    let (id, _, rids) = rows(view, sel)?;
    Some(Command::ShiftRows { id, rids, delta })
}

/// Half a seat pitch of the selected rows' block: the arrow keys' step.
pub(crate) fn half_pitch(view: &View<'_>, sel: &Selection) -> Option<f64> {
    rows(view, sel).map(|(_, b, _)| b.seat_pitch / 2.0)
}

pub(crate) fn label(view: &View<'_>, sel: &Selection, label: Option<String>) -> Option<Command> {
    let (id, _, rids) = rows(view, sel)?;
    let commands = rids
        .into_iter()
        .map(|rid| Command::SetRowLabel {
            id,
            rid,
            label: label.clone(),
        })
        .collect();
    Some(Command::Composite { commands })
}

/// Where a seat lies across the rows, for an aisle through several rows:
/// with the skew's shear undone (03 §2.4: `x' = x − y × tan(skew)`), its
/// angle about an arc's centre, else its x, so one value names the same
/// column in every row.
fn column(b: &RowBlock, p: P) -> f64 {
    let x = p[0] + p[1] * b.skew.to_radians().tan();
    match b.curve {
        Curve::Arc { radius } => x.atan2((p[1] + radius) * radius.signum()),
        _ => x,
    }
}

/// A selected row: its rid, its seats and their columns.
type Line<'a> = (u32, &'a [CachedSeat], Vec<f64>);

/// Each selected row's seats in the block's space, with their columns, and
/// the column of the world point `at`.
fn across<'a>(view: &View<'a>, sel: &Selection, at: P) -> Option<(u32, Vec<Line<'a>>, f64)> {
    let (id, b, rids) = rows(view, sel)?;
    let c0 = column(b, view.index.place(id)?.invert(at));
    let lines = view
        .index
        .seats(id)
        .chunk_by(|x, y| x.row == y.row)
        .filter_map(|row| {
            let rid = b.rows.get(row.first()?.row as usize)?.rid;
            rids.binary_search(&rid).ok()?;
            Some((rid, row, row.iter().map(|s| column(b, s.pos)).collect()))
        })
        .collect();
    Some((id, lines, c0))
}

/// Cuts an aisle at the world point `at` through the selected rows: in each
/// row the position whose column is nearest, if the point is within half
/// the smaller gap to its neighbours (a row of one position is skipped), is
/// marked removed (it keeps its oid and its
/// number, so no other seat is renamed).
pub(crate) fn cut_aisle(view: &View<'_>, sel: &Selection, at: P) -> Option<Command> {
    let (id, lines, c0) = across(view, sel, at)?;
    let mut oids = Vec::new();
    for (_, row, cols) in lines {
        let d = |k: usize| cols.get(k).map_or(f64::INFINITY, |c| (c - c0).abs());
        let near = (0..cols.len()).min_by(|&a, &b| d(a).total_cmp(&d(b)));
        let Some(k) = near else { continue };
        let gap = |j: Option<usize>| j.and_then(|j| Some((cols.get(j)? - cols.get(k)?).abs()));
        let half = gap(k.checked_sub(1))
            .into_iter()
            .chain(gap(Some(k + 1)))
            .fold(f64::INFINITY, f64::min)
            / 2.0;
        // A lone position has no gap to measure: it is never cut.
        if let Some(s) = row.get(k)
            && !s.removed
            && half.is_finite()
            && d(k) <= half
        {
            oids.push(s.oid);
        }
    }
    Some(Command::EditSeats {
        id,
        oids,
        edit: SeatEdit::Remove,
    })
}

/// Opens an aisle `count` positions wide at the world point `at` through
/// the selected rows: in each row whose seats lie on both sides of it,
/// `count` removed positions with fresh oids go in before the first seat
/// past the point. The seats before stay; the seats after move `count`
/// pitches on, keeping their oids, and their numbers go up by `count`.
pub(crate) fn open_aisle(view: &View<'_>, sel: &Selection, at: P, count: u32) -> Option<Command> {
    let (id, lines, c0) = across(view, sel, at)?;
    let commands = lines
        .into_iter()
        .filter_map(|(rid, _, cols)| {
            let k = cols.iter().position(|&c| c > c0).filter(|&k| k > 0)?;
            let at = u32::try_from(k).ok()?;
            Some(Command::InsertGap { id, rid, at, count })
        })
        .collect();
    Some(Command::Composite { commands })
}

/// One edit to every selected seat.
pub(crate) fn edit(view: &View<'_>, sel: &Selection, edit: SeatEdit) -> Option<Command> {
    let (id, oids) = seats(view, sel)?;
    Some(Command::EditSeats { id, oids, edit })
}

/// The selected seats nudged by the world vector `v`, turned into their
/// node's space.
pub(crate) fn nudge(view: &View<'_>, sel: &Selection, v: P) -> Option<Command> {
    let (id, _) = seats(view, sel)?;
    let place = view.index.place(id)?;
    let (a, b) = (place.invert([0.0, 0.0]), place.invert(v));
    edit(
        view,
        sel,
        SeatEdit::Nudge {
            by: [b[0] - a[0], b[1] - a[1]],
        },
    )
}

/// Delete or Backspace at any level, and the selection after it: nodes are
/// deleted. Rows: all of a block's rows delete the block; selected rows at
/// the back are removed (nothing else moves); any other selected row is
/// emptied, every position marked removed, since removing it would move
/// the rows behind it forward a pitch and rename them (03 §2.4: row `i` is
/// at `i × rowPitch`, labelled by its place). The next row not deleted,
/// else the one before, is selected. Seats are marked removed and stay
/// selected.
pub(crate) fn delete(view: &View<'_>, sel: &Selection) -> Option<(Command, Level, Vec<u32>)> {
    match sel.level() {
        Level::Top | Level::Section(_) => {
            let (ids, _) = targets(view, sel)?;
            Some((Command::DeleteNodes { ids }, sel.level(), Vec::new()))
        }
        Level::Rows(_) => {
            let (id, b, rids) = rows(view, sel)?;
            let gone: BTreeSet<u32> = rids.iter().copied().collect();
            let on = |r: &&BlockRow| gone.contains(&r.rid);
            if b.rows.iter().all(|r| on(&r)) {
                let level = view.doc.parent(id).map_or(Level::Top, Level::Section);
                return Some((Command::DeleteNodes { ids: vec![id] }, level, Vec::new()));
            }
            let back: Vec<u32> = b.rows.iter().rev().take_while(on).map(|r| r.rid).collect();
            let emptied = b.rows.iter().filter(on).filter(|r| !back.contains(&r.rid));
            let oids: Vec<u32> = emptied
                .flat_map(|r| r.oids.iter().flat_map(|o| o[0]..o[0].saturating_add(o[1])))
                .collect();
            let mut commands = Vec::new();
            if !back.is_empty() {
                commands.push(Command::RemoveRows { id, rids: back });
            }
            if !oids.is_empty() {
                let edit = SeatEdit::Remove;
                commands.push(Command::EditSeats { id, oids, edit });
            }
            let last = b.rows.iter().rposition(|r| on(&r))?;
            let after = b.rows.iter().skip(last).find(|r| !on(r));
            let next = after.or_else(|| b.rows.iter().rev().find(|r| !on(r)));
            let next = next.map(|r| r.rid).into_iter().collect();
            Some((Command::Composite { commands }, sel.level(), next))
        }
        Level::Seats(_) => {
            let items = sel.items().iter().copied().collect();
            Some((edit(view, sel, SeatEdit::Remove)?, sel.level(), items))
        }
    }
}
