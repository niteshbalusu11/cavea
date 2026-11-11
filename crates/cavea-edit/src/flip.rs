//! Mirroring a node for `flipNodes` (`docs/07-designer.md` §5.1).
//!
//! A flip is a mirror image in the parent's space, made of what the schema
//! can say: the origin is reflected about the line, the rotation `θ`
//! becomes `−θ`, and the node's own geometry is mirrored along the same
//! axis in its own space. A row block cannot be mirrored top to bottom in
//! its own space (rows run towards +y), so a vertical flip turns it by
//! `180° − θ` and mirrors it left to right, which is the same image.
//!
//! A row block mirrored left to right keeps every seat's oid on its own
//! (mirrored) seat: each row's positions are reversed (its oids, possibly
//! as runs of one, and its removed positions), and alignment, offsets,
//! stagger, skew and nudges change sign, so the seat that was `k`-th from
//! the left end is now `k`-th from the right, with its overrides. Seat
//! numbering `ltr` becomes `rtl` and the reverse, so every seat keeps its
//! label; `center-out` and `serpentine` have no mirrored form and are kept,
//! so labels of such rows change sides.

use cavea_compiler::document::{Align, Node, Outline, Path, RowBlock, SeatDirection};

use crate::command::{Axis, CommandError, compress, expand_runs};
use crate::document::each_node;
use crate::draft::normalise;

pub(crate) fn mirror(mut node: Node, axis: Axis, about: f64) -> Result<Node, CommandError> {
    let h = axis == Axis::Horizontal;
    let i = usize::from(!h);
    let turn = matches!(node, Node::RowBlock(_)) && !h;
    each_node!(&mut node, n => {
        let o = n.origin.get_mut(i);
        if let Some(o) = o {
            *o = (about - *o) + about;
        }
        n.rotation = normalise(if turn { 180.0 } else { 0.0 } - n.rotation);
    });
    if !each_node!(&node, n => n.origin.iter().all(|v| v.is_finite())) {
        return Err(CommandError::InvalidValue("origin"));
    }
    let flip = |p: &mut [f64; 2]| {
        if let Some(v) = p.get_mut(i) {
            *v = 0.0 - *v;
        }
    };
    match &mut node {
        Node::Section(s) => {
            if let Outline::Points(p) = &mut s.outline {
                p.points.iter_mut().for_each(flip);
            }
        }
        Node::RowBlock(b) => mirror_block(b)?,
        Node::Row(r) => {
            let Path::Polyline { points } = &mut r.path;
            points.iter_mut().for_each(flip);
            r.seat_overrides
                .values_mut()
                .filter_map(|o| o.nudge.as_mut())
                .for_each(flip);
        }
        Node::Area(a) => a.shape.points.iter_mut().for_each(flip),
        Node::Shape(s) => s.points.iter_mut().flatten().for_each(flip),
        Node::Text(t) if h => t.align = swap(t.align),
        _ => {}
    }
    Ok(node)
}

fn swap(a: Align) -> Align {
    match a {
        Align::Left => Align::Right,
        Align::Right => Align::Left,
        Align::Center => Align::Center,
    }
}

/// The block mirrored left to right in its own space, each oid on its own
/// mirrored seat (see the module comment).
fn mirror_block(b: &mut RowBlock) -> Result<(), CommandError> {
    b.align = swap(b.align);
    b.stagger = 0.0 - b.stagger;
    b.skew = 0.0 - b.skew;
    b.seat_labels.direction = match b.seat_labels.direction {
        SeatDirection::Ltr => SeatDirection::Rtl,
        SeatDirection::Rtl => SeatDirection::Ltr,
        d => d,
    };
    for row in &mut b.rows {
        let mut oids = expand_runs(row, b.id)?;
        oids.reverse();
        row.oids = compress(&oids);
        row.offset = 0.0 - row.offset;
        let last = row.seats.saturating_sub(1);
        row.removed = row
            .removed
            .iter()
            .map(|&p| last.saturating_sub(p))
            .collect();
        row.removed.sort_unstable();
    }
    for nudge in b
        .seat_overrides
        .values_mut()
        .filter_map(|o| o.nudge.as_mut())
    {
        nudge[0] = 0.0 - nudge[0];
    }
    Ok(())
}
