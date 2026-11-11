//! The select tool's handles (`docs/07-designer.md` §5.1): rotate with a
//! movable pivot for any node selection, and resize for one row block or
//! shape. Sides and corners are named in the node's own space (y down): on
//! a row block the top side is the front (row 0) and the left side holds
//! each row's first position.

use std::collections::BTreeSet;

use cavea_compiler::document::{Align, BlockRow, Curve, Image, Node, RowBlock, Shape, ShapeKind};
use cavea_core::limits::MAX_SEATS_PER_ROW;

use super::draw::{dist, snap15};
use super::{Modifiers, Pointer, Readout, tuning};
use crate::command::{Command, End, NodeTransform, Property};
use crate::draft::{MIN_SIZE, normalise};
use crate::index::{P, Place, Quad, Rect};
use crate::selection::{Level, Selection, View, pickable};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomRight,
    BottomLeft,
}

/// In JSON `"rotate"`, `{"side": "top"}`, `{"end": 1}`, and so on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HandleKind {
    /// Drag to rotate about the pivot.
    Rotate,
    /// Drag to move the pivot.
    Pivot,
    Side(Side),
    Corner(Corner),
    /// A line's first (0) or last (1) point.
    End(u8),
    /// The middle of a row block's front row: drag to curve it (07 §5.3).
    Curve,
}

/// A handle: drawn at `at` (chart units) moved by `offset` CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Handle {
    pub kind: HandleKind,
    pub at: [f64; 2],
    pub offset: [f64; 2],
}

/// The selected nodes the handles act on: those that can be picked, at a
/// node level, and the section they are in.
pub(crate) fn targets(view: &View<'_>, sel: &Selection) -> Option<(Vec<u32>, Option<u32>)> {
    let parent = match sel.level() {
        Level::Top => None,
        Level::Section(s) => Some(s),
        Level::Rows(_) | Level::Seats(_) => return None,
    };
    let ids: Vec<u32> = sel
        .items()
        .iter()
        .copied()
        .filter(|&id| pickable(view.doc, view.floor, id))
        .collect();
    (!ids.is_empty()).then_some((ids, parent))
}

/// The frame around these nodes: one node's own box, else the bounds.
pub(crate) fn frame(view: &View<'_>, ids: &[u32]) -> Option<Quad> {
    match ids {
        [one] => view.index.quad(*one),
        _ => Rect::around(ids.iter().filter_map(|&id| view.index.quad(id)).flatten())
            .map(|r| r.corners()),
    }
}

fn mid(a: P, b: P) -> P {
    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
}

pub(crate) fn centre(q: &Quad) -> P {
    mid(mid(q[0], q[2]), mid(q[1], q[3]))
}

/// The handles for the selection; `pivot` is a pivot the user moved.
pub(crate) fn handles(view: &View<'_>, sel: &Selection, pivot: Option<P>) -> Vec<Handle> {
    if let Level::Rows(id) = sel.level() {
        return row_handles(view, sel, id);
    }
    let Some((ids, _)) = targets(view, sel) else {
        return Vec::new();
    };
    let Some(q) = frame(view, &ids) else {
        return Vec::new();
    };
    let handle = |kind, at| Handle {
        kind,
        at,
        offset: [0.0, 0.0],
    };
    // The rotate handle sits outside the top side, along its outward normal.
    let (top, bottom) = (mid(q[0], q[1]), mid(q[2], q[3]));
    let (mut nx, mut ny) = (top[0] - bottom[0], top[1] - bottom[1]);
    if nx.hypot(ny) == 0.0 {
        (nx, ny) = (q[1][1] - q[0][1], q[0][0] - q[1][0]);
    }
    let len = nx.hypot(ny);
    let n = if len > 0.0 && len.is_finite() {
        [nx / len, ny / len]
    } else {
        [0.0, -1.0]
    };
    let mut out = vec![
        handle(HandleKind::Pivot, pivot.unwrap_or_else(|| centre(&q))),
        Handle {
            kind: HandleKind::Rotate,
            at: top,
            offset: [n[0] * tuning::ROTATE_OFFSET, n[1] * tuning::ROTATE_OFFSET],
        },
    ];
    let [id] = ids[..] else {
        return out;
    };
    let sides = [
        (Side::Top, mid(q[0], q[1])),
        (Side::Right, mid(q[1], q[2])),
        (Side::Bottom, mid(q[2], q[3])),
        (Side::Left, mid(q[3], q[0])),
    ];
    let corners = [
        (Corner::TopLeft, q[0]),
        (Corner::TopRight, q[1]),
        (Corner::BottomRight, q[2]),
        (Corner::BottomLeft, q[3]),
    ];
    match view.doc.node(id) {
        Some(Node::RowBlock(b)) => {
            out.extend(sides.map(|(s, at)| handle(HandleKind::Side(s), at)));
            // After the sides, which win where they meet on a small block.
            // Two positions have no middle to move.
            let curved = b.rows.first().is_some_and(|r| r.seats > 2);
            let middle = front_middle(view, id).filter(|_| curved);
            if let Some(at) = middle.and_then(|p| view.index.to_world(id, p)) {
                let (sin, cos) = b.rotation.to_radians().sin_cos();
                let k = tuning::CURVE_OFFSET;
                out.push(Handle {
                    kind: HandleKind::Curve,
                    at,
                    offset: [-sin * k, cos * k],
                });
            }
        }
        Some(Node::Shape(s)) => match s.kind {
            ShapeKind::Rect | ShapeKind::Ellipse | ShapeKind::Polygon => {
                out.extend(corners.map(|(c, at)| handle(HandleKind::Corner(c), at)));
                out.extend(sides.map(|(s, at)| handle(HandleKind::Side(s), at)));
            }
            ShapeKind::Line => {
                let points = s.points.as_deref().unwrap_or_default();
                for (i, p) in [(0, points.first()), (1, points.last())] {
                    if let Some(at) = p.and_then(|&p| view.index.to_world(id, p)) {
                        out.push(handle(HandleKind::End(i), at));
                    }
                }
            }
            ShapeKind::Path => {}
        },
        Some(Node::Image(_)) => {
            out.extend(corners.map(|(c, at)| handle(HandleKind::Corner(c), at)));
            out.extend(sides.map(|(s, at)| handle(HandleKind::Side(s), at)));
        }
        _ => {}
    }
    out
}

/// At the rows level: a handle on the left and right ends of the selected
/// rows' box, in the block's space, adding or removing seats at that end of
/// each selected row (07 §5.3).
fn row_handles(view: &View<'_>, sel: &Selection, id: u32) -> Vec<Handle> {
    let Some(Node::RowBlock(b)) = view.doc.node(id) else {
        return Vec::new();
    };
    if !view.pickable(sel.level(), id) {
        return Vec::new();
    }
    let rows = picked_rows(b, sel.items());
    let seats = view.index.seats(id).iter();
    let present = seats.filter(|s| !s.removed && rows.contains(&s.row));
    let Some(r) = Rect::around(present.map(|s| s.pos)) else {
        return Vec::new();
    };
    let q = r
        .grow(view.index.seat_pad(id))
        .corners()
        .map(|p| view.index.to_world(id, p).unwrap_or(p));
    let handle = |side, at| Handle {
        kind: HandleKind::Side(side),
        at,
        offset: [0.0, 0.0],
    };
    vec![
        handle(Side::Right, mid(q[1], q[2])),
        handle(Side::Left, mid(q[3], q[0])),
    ]
}

/// The indices of the block's rows whose rids are in `rids`.
fn picked_rows(b: &RowBlock, rids: &BTreeSet<u32>) -> BTreeSet<u32> {
    (0u32..)
        .zip(&b.rows)
        .filter(|(_, r)| rids.contains(&r.rid))
        .map(|(i, _)| i)
        .collect()
}

/// The handle under the pointer, if any: the first within reach.
pub(crate) fn hit(handles: &[Handle], p: &Pointer) -> Option<Handle> {
    let reach = match p.kind {
        super::PointerKind::Mouse => tuning::HANDLE_MOUSE,
        _ => tuning::HANDLE_TOUCH,
    };
    handles.iter().copied().find(|h| {
        let at = [
            h.at[0] + h.offset[0] / p.zoom,
            h.at[1] + h.offset[1] / p.zoom,
        ];
        dist(at, p.world) * p.zoom <= reach
    })
}

/// Each node turned by `angle` degrees about `pivot` (in the parent's
/// space): its origin on a circle about the pivot, its rotation changed by
/// the angle and reduced to (−180, 180]. The sine and cosine of the angle
/// are the platform's: they only place the stored origin, which the
/// compiler then reads as plain numbers.
pub(crate) fn rotate(nodes: &[(u32, P, f64)], pivot: P, angle: f64) -> Command {
    let (sin, cos) = angle.to_radians().sin_cos();
    let nodes = nodes
        .iter()
        .map(|&(id, o, rotation)| {
            let (x, y) = (o[0] - pivot[0], o[1] - pivot[1]);
            NodeTransform {
                id,
                origin: [pivot[0] + x * cos - y * sin, pivot[1] + x * sin + y * cos],
                rotation: normalise(rotation + angle),
            }
        })
        .collect();
    Command::TransformNodes { nodes }
}

/// What a resize step shows.
#[derive(Debug)]
pub(crate) enum Step {
    /// The node as it was at the start.
    Base,
    Show(Command, Readout),
    /// Not possible: what is shown stays.
    Keep,
}

/// A resize of the node as it was at the start (`base`) by the handle's
/// travel `d` in the node's own space, the pointer being at `q` there.
/// `room` is how many seats may be added; `rows`, at the rows level, the
/// rows whose ends the left and right handles move.
pub(crate) fn resize(
    base: &Node,
    id: u32,
    kind: HandleKind,
    d: P,
    q: P,
    mods: Modifiers,
    (room, rows): (u64, Option<&BTreeSet<u32>>),
) -> Step {
    match (base, kind) {
        (Node::RowBlock(b), HandleKind::Curve) if rows.is_none() => curve(b, id, d),
        (Node::RowBlock(_), HandleKind::Side(Side::Top | Side::Bottom)) if rows.is_some() => {
            Step::Keep
        }
        (Node::RowBlock(b), HandleKind::Side(side)) if rows.is_some() => {
            counts(b, id, side, d, room, rows)
        }
        (Node::RowBlock(b), HandleKind::Side(side)) if mods.ctrl => pitch(b, id, side, d),
        (Node::RowBlock(b), HandleKind::Side(side)) => counts(b, id, side, d, room, None),
        (Node::Shape(s), _) => scale(s, id, kind, d, q, mods.shift),
        (Node::Image(i), _) => scale_image(i, id, kind, d, mods.shift),
        _ => Step::Keep,
    }
}

fn readout(b: &RowBlock) -> Readout {
    Readout::Block {
        rows: u32::try_from(b.rows.len()).unwrap_or(u32::MAX),
        seats: b.rows.iter().map(|r| r.seats).max().unwrap_or(0),
        seat_pitch: b.seat_pitch,
        row_pitch: b.row_pitch,
    }
}

/// `origin` moved by `v` in the node's own space.
fn shifted(b: &RowBlock, v: P) -> P {
    let t = Place::of([0.0, 0.0], b.rotation).turn(v);
    [b.origin[0] + t[0], b.origin[1] + t[1]]
}

/// What the origin moves by, in the block's own space, when the kept rows
/// move back by `travel` in node space (rows added at the front, or the
/// front row moved forward): they shift by the skew's shear too
/// (03 §2.4: `x' = x − y × tan(skew)`), so the origin takes both up.
fn front_shift(b: &RowBlock, travel: f64) -> P {
    [travel * b.skew.to_radians().tan(), -travel]
}

fn align(b: &RowBlock) -> f64 {
    match b.align {
        Align::Left => 0.0,
        Align::Center => 0.5,
        Align::Right => 1.0,
    }
}

/// Whether the compiler accepts the block's arc (03 §2.4): no row's radius
/// reaches zero or changes sign, and no seat is more than half a turn from
/// the apex. The same test as the compiler's, on the same numbers.
pub(crate) fn arc_ok(b: &RowBlock) -> bool {
    let Curve::Arc { radius } = b.curve else {
        return true;
    };
    b.rows.iter().enumerate().all(|(i, row)| {
        let length = f64::from(row.seats.saturating_sub(1)) * b.seat_pitch;
        let stagger = if i % 2 == 1 {
            b.stagger * b.seat_pitch
        } else {
            0.0
        };
        let s0 = -align(b) * length + row.offset + stagger;
        let r = radius + i as f64 * b.row_pitch;
        let reach = s0.abs().max((s0 + length).abs());
        r != 0.0 && (r > 0.0) == (radius > 0.0) && reach <= std::f64::consts::PI * r.abs()
    })
}

/// The block with `n` rows added (removed, if negative) at `side`, or `n`
/// seats at each row's end: what the structural commands make of it, to
/// check the arc. Kept seats stay put: the offset takes up the shift
/// (07 §2.2 `addSeats`). With `rows`, seats change in those rows only.
fn changed(b: &RowBlock, side: Side, n: i64, rows: Option<&BTreeSet<u32>>) -> RowBlock {
    let mut c = b.clone();
    let k = n.unsigned_abs() as usize;
    let row = |seats| BlockRow {
        rid: 0,
        seats,
        offset: 0.0,
        oids: Vec::new(),
        label: None,
        removed: Vec::new(),
    };
    match side {
        Side::Bottom if n >= 0 => {
            let seats = b.rows.last().map_or(1, |r| r.seats);
            c.rows.extend((0..k).map(|_| row(seats)));
        }
        Side::Bottom => c.rows.truncate(b.rows.len().saturating_sub(k)),
        Side::Top => {
            if n >= 0 {
                let seats = b.rows.first().map_or(1, |r| r.seats);
                c.rows.splice(0..0, (0..k).map(|_| row(seats)));
            } else {
                c.rows.drain(..k.min(c.rows.len()));
            }
            if let Curve::Arc { radius } = &mut c.curve {
                *radius -= n as f64 * b.row_pitch;
            }
        }
        Side::Left | Side::Right => {
            let length = n as f64 * b.seat_pitch;
            let front = if side == Side::Left { length } else { 0.0 };
            for r in c
                .rows
                .iter_mut()
                .filter(|r| rows.is_none_or(|s| s.contains(&r.rid)))
            {
                r.seats = (i64::from(r.seats) + n).clamp(1, i64::from(u32::MAX)) as u32;
                r.offset += align(b) * length - front;
            }
        }
    }
    c
}

/// Rows or seats added or removed at a side, in whole rows or seats; with
/// `picked`, seats at the left or right end of those rows only.
fn counts(
    b: &RowBlock,
    id: u32,
    side: Side,
    d: P,
    room: u64,
    picked: Option<&BTreeSet<u32>>,
) -> Step {
    let on = |r: &&BlockRow| picked.is_none_or(|s| s.contains(&r.rid));
    let rows = b.rows.len() as i64;
    let lines = b.rows.iter().filter(on).count() as i64;
    let widest = i64::from(b.rows.iter().filter(on).map(|r| r.seats).max().unwrap_or(1));
    let narrowest = i64::from(b.rows.iter().filter(on).map(|r| r.seats).min().unwrap_or(1));
    let room = room.min(u64::from(tuning::LIVE_SEATS)) as i64;
    let steps = |v: f64, pitch: f64| {
        let n = (v / pitch).round();
        if n.is_finite() {
            n.clamp(-1e9, 1e9) as i64
        } else {
            0
        }
    };
    let (mut n, low, high) = match side {
        Side::Top | Side::Bottom => {
            let v = if side == Side::Top { -d[1] } else { d[1] };
            let edge = match side {
                Side::Top => b.rows.first(),
                _ => b.rows.last(),
            };
            let seats = i64::from(edge.map_or(1, |r| r.seats)).max(1);
            (steps(v, b.row_pitch), 1 - rows, room / seats)
        }
        Side::Left | Side::Right => {
            let v = if side == Side::Left { -d[0] } else { d[0] };
            let fit = (i64::from(MAX_SEATS_PER_ROW) - widest).min(room / lines.max(1));
            (steps(v, b.seat_pitch), 1 - narrowest, fit)
        }
    };
    n = n.clamp(low.min(0), high.max(0));
    // The most rows or seats the arc allows, towards zero.
    if !arc_ok(&changed(b, side, n, picked)) {
        let (mut ok, mut bad) = (0i64, n);
        while (bad - ok).abs() > 1 {
            let m = ok + (bad - ok) / 2;
            if arc_ok(&changed(b, side, m, picked)) {
                ok = m;
            } else {
                bad = m;
            }
        }
        n = ok;
    }
    // Rows added or removed at the front change the parity of the kept
    // rows, and so their stagger (03 §2.4): a staggered block takes them in
    // pairs, so every kept seat stays put.
    if side == Side::Top && b.stagger != 0.0 {
        n -= n % 2;
    }
    if n == 0 {
        return Step::Base;
    }
    let k = n.unsigned_abs() as u32;
    let front: Vec<u32> = b.rows.iter().map(|r| r.rid).take(k as usize).collect();
    let back: Vec<u32> = b
        .rows
        .iter()
        .rev()
        .map(|r| r.rid)
        .take(k as usize)
        .collect();
    let mut commands = Vec::new();
    match side {
        Side::Bottom if n > 0 => commands.push(Command::AddRows {
            id,
            index: b.rows.len() as u32,
            count: k,
            seats: b.rows.last().map_or(1, |r| r.seats),
        }),
        Side::Bottom => commands.push(Command::RemoveRows { id, rids: back }),
        Side::Top => {
            commands.push(if n > 0 {
                Command::AddRows {
                    id,
                    index: 0,
                    count: k,
                    seats: b.rows.first().map_or(1, |r| r.seats),
                }
            } else {
                Command::RemoveRows { id, rids: front }
            });
            // The kept rows stay put: the origin moves to the new front
            // row, and an arc's centre stays where it was.
            let travel = n as f64 * b.row_pitch;
            commands.push(Command::TransformNodes {
                nodes: vec![NodeTransform {
                    id,
                    origin: shifted(b, front_shift(b, travel)),
                    rotation: b.rotation,
                }],
            });
            if let Curve::Arc { radius } = b.curve {
                let curve = Curve::Arc {
                    radius: radius - travel,
                };
                commands.push(Command::SetProperty {
                    id,
                    property: Property::Curve(curve),
                });
            }
        }
        Side::Left | Side::Right => {
            let end = if side == Side::Left {
                End::Start
            } else {
                End::End
            };
            match picked {
                None => {
                    let count = i32::try_from(n).unwrap_or(0);
                    commands.push(Command::ResizeRows { id, end, count });
                }
                Some(_) => commands.extend(b.rows.iter().filter(on).map(|r| {
                    let rid = r.rid;
                    match n > 0 {
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
                    }
                })),
            }
        }
    }
    let shown = readout(&changed(b, side, n, picked));
    Step::Show(Command::Composite { commands }, shown)
}

/// With Ctrl: the row pitch (top, bottom) or seat pitch (left, right),
/// in whole chart units of at least 1, so the dragged side follows the
/// pointer and the opposite one stays.
fn pitch(b: &RowBlock, id: u32, side: Side, d: P) -> Step {
    let set = |property| Command::SetProperty { id, property };
    let mut c = b.clone();
    let mut commands = Vec::new();
    match side {
        Side::Top | Side::Bottom => {
            let gaps = b.rows.len().saturating_sub(1) as f64;
            let v = if side == Side::Top { -d[1] } else { d[1] };
            let p = (b.row_pitch + v / gaps).round().max(1.0);
            if gaps == 0.0 || !p.is_finite() {
                return Step::Base;
            }
            c.row_pitch = p;
            commands.push(set(Property::RowPitch(p)));
            // The back row stays: the front moves, an arc's centre stays.
            let travel = (p - b.row_pitch) * gaps;
            if side == Side::Top {
                let origin = shifted(b, front_shift(b, travel));
                c.origin = origin;
                commands.push(Command::TransformNodes {
                    nodes: vec![NodeTransform {
                        id,
                        origin,
                        rotation: b.rotation,
                    }],
                });
                if let Curve::Arc { radius } = &mut c.curve {
                    *radius -= travel;
                    commands.push(set(Property::Curve(c.curve)));
                }
            }
        }
        Side::Left | Side::Right => {
            let widest = b.rows.iter().map(|r| r.seats).max().unwrap_or(1);
            let gaps = f64::from(widest.saturating_sub(1));
            let v = if side == Side::Left { -d[0] } else { d[0] };
            let p = (b.seat_pitch + v / gaps).round().max(1.0);
            if gaps == 0.0 || !p.is_finite() {
                return Step::Base;
            }
            c.seat_pitch = p;
            commands.push(set(Property::SeatPitch(p)));
            // The widest row's far end stays, by its alignment; an arc
            // keeps its apex.
            let travel = (p - b.seat_pitch) * gaps;
            let x = match side {
                Side::Right => align(b) * travel,
                _ => (align(b) - 1.0) * travel,
            };
            if x != 0.0 && matches!(b.curve, Curve::None {} | Curve::Bulge { .. }) {
                commands.push(Command::TransformNodes {
                    nodes: vec![NodeTransform {
                        id,
                        origin: shifted(b, [x, 0.0]),
                        rotation: b.rotation,
                    }],
                });
            }
        }
    }
    if c.row_pitch == b.row_pitch && c.seat_pitch == b.seat_pitch {
        return Step::Base;
    }
    if !arc_ok(&c) {
        return Step::Keep;
    }
    Step::Show(Command::Composite { commands }, readout(&c))
}

/// A shape scaled by a corner or side handle about the opposite one, in
/// its own space (Shift keeps a corner drag's aspect ratio), each side at
/// least [`MIN_SIZE`]; a line's end dragged (Shift: 15° steps about the
/// other end).
/// The middle of the block's front row in its own space: its middle
/// position, or between the two middle ones.
fn front_middle(view: &View<'_>, id: u32) -> Option<P> {
    let front: Vec<P> = view
        .index
        .seats(id)
        .iter()
        .filter(|s| s.row == 0)
        .map(|s| s.pos)
        .collect();
    let (a, b) = (
        front.get((front.len().max(1) - 1) / 2)?,
        front.get(front.len() / 2)?,
    );
    Some([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0])
}

/// Where the front row's positions lie along it (03 §2.4: `s = a + offset
/// + k × seatPitch`; row 0 is never staggered): its first, its middle one
/// or two (where the handle is, `front_middle`), and its last.
#[derive(Debug, Clone, Copy)]
struct Span {
    first: f64,
    middle: [f64; 2],
    last: f64,
    /// A bulge's weight at the middle, `1 − (2t − 1)²` (03 §2.4).
    weight: f64,
}

fn front_span(b: &RowBlock) -> Option<Span> {
    let n = b.rows.first()?.seats;
    let offset = b.rows.first()?.offset;
    let last = n.saturating_sub(1);
    let first = -align(b) * f64::from(last) * b.seat_pitch + offset;
    let at = |k: u32| first + f64::from(k) * b.seat_pitch;
    let (ma, mb) = (last / 2, (n / 2).min(last));
    let w = |k: u32| {
        let t = f64::from(k) / f64::from(last.max(1));
        1.0 - (2.0 * t - 1.0).powi(2)
    };
    Some(Span {
        first,
        middle: [at(ma), at(mb)],
        last: at(last),
        weight: (w(ma) + w(mb)) / 2.0,
    })
}

/// The y of the front row's position at `s` on an arc of curvature `k`
/// (`1 / radius`): `R cos(s / R) − R` (03 §2.4), 0 when straight.
fn arc_y(k: f64, s: f64) -> f64 {
    if k == 0.0 {
        0.0
    } else {
        ((k * s).cos() - 1.0) / k
    }
}

/// On an arc of curvature `k`: how far the front row's middle lies behind
/// (+y) the midpoint of its ends, and that midpoint's y.
fn arc_depth(k: f64, s: Span) -> (f64, f64) {
    let ends = (arc_y(k, s.first) + arc_y(k, s.last)) / 2.0;
    let middle = (arc_y(k, s.middle[0]) + arc_y(k, s.middle[1])) / 2.0;
    (middle - ends, ends)
}

/// The curvature in (0, π / reach] whose front row has depth `target`
/// (above 0), the farthest seat staying within half a turn of the apex:
/// the first crossing of 64 samples, refined by bisection, else the
/// sample of greatest depth (the depth is not monotonic up to half a turn).
fn curvature(span: Span, target: f64) -> Option<f64> {
    let reach = span.first.abs().max(span.last.abs());
    if !(reach > 0.0 && reach.is_finite()) {
        return None;
    }
    let max = std::f64::consts::PI / reach;
    let depth = |k: f64| arc_depth(k, span).0;
    let mut best = (0.0, 0.0);
    let mut lo = 0.0;
    for i in 1..=64 {
        let k = max * f64::from(i) / 64.0;
        let d = depth(k);
        if d >= target {
            let mut hi = k;
            for _ in 0..60 {
                let mid = (lo + hi) / 2.0;
                if depth(mid) < target {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            return Some(hi);
        }
        if d > best.1 {
            best = (k, d);
        }
        lo = k;
    }
    (best.0 > 0.0).then_some(best.0)
}

/// The curve handle dragged by `d` in the block's own space (07 §5.3): the
/// front row's depth, how far its middle lies behind the midpoint of its
/// ends, becomes its depth at the press plus the pointer's travel along
/// the block's y axis. A bulge takes it as its amount. A straight block or
/// an arc becomes the arc of the nearest depth it can have, straight again
/// within half a chart unit, and its origin moves along y so that the
/// midpoint of the front row's ends stays where it was: its middle follows
/// the pointer.
fn curve(b: &RowBlock, id: u32, d: P) -> Step {
    let Some(span) = front_span(b) else {
        return Step::Keep;
    };
    let k0 = match b.curve {
        Curve::Arc { radius } => 1.0 / radius,
        _ => 0.0,
    };
    let start = match b.curve {
        Curve::Bulge { amount } => amount * span.weight,
        _ => arc_depth(k0, span).0,
    };
    let to = start + d[1];
    if !to.is_finite() || d[1] == 0.0 {
        return Step::Base;
    }
    let (curve, k, depth) = match b.curve {
        Curve::Bulge { .. } if span.weight > 0.0 => (
            Curve::Bulge {
                amount: to / span.weight,
            },
            None,
            to,
        ),
        Curve::Bulge { .. } => return Step::Keep,
        _ if to.abs() < 0.5 => (Curve::None {}, Some(0.0), 0.0),
        _ => {
            let Some(k) = curvature(span, to.abs()).map(|k| k.copysign(to)) else {
                return Step::Keep;
            };
            let curve = Curve::Arc { radius: 1.0 / k };
            (curve, Some(k), arc_depth(k, span).0)
        }
    };
    let mut changed = b.clone();
    changed.curve = curve;
    if !arc_ok(&changed) {
        return Step::Keep;
    }
    let mut commands = vec![Command::SetProperty {
        id,
        property: Property::Curve(curve),
    }];
    if let Some(k) = k {
        let shift = arc_depth(k0, span).1 - arc_depth(k, span).1;
        commands.push(Command::TransformNodes {
            nodes: vec![NodeTransform {
                id,
                origin: shifted(b, [0.0, shift]),
                rotation: b.rotation,
            }],
        });
    }
    let radius = match curve {
        Curve::Arc { radius } => Some(radius),
        _ => None,
    };
    let shown = Readout::Curve { depth, radius };
    Step::Show(Command::Composite { commands }, shown)
}

fn scale(s: &Shape, id: u32, kind: HandleKind, d: P, q: P, shift: bool) -> Step {
    let mut next = s.clone();
    let points = s.points.as_deref().unwrap_or_default();
    if let HandleKind::End(i) = kind {
        let (Some(&first), Some(&last)) = (points.first(), points.last()) else {
            return Step::Keep;
        };
        let other = if i == 0 { last } else { first };
        let q = if shift { snap15(other, q) } else { q };
        if dist(other, q) < MIN_SIZE || !q.iter().all(|v| v.is_finite()) {
            return Step::Keep;
        }
        let mut list = points.to_vec();
        let slot = if i == 0 {
            list.first_mut()
        } else {
            list.last_mut()
        };
        if let Some(p) = slot {
            *p = q;
        }
        next.points = Some(list);
        let (dx, dy) = (q[0] - other[0], q[1] - other[1]);
        let readout = Readout::Line {
            length: dx.hypot(dy),
            angle: dy.atan2(dx).to_degrees(),
        };
        return decoration(id, next, readout);
    }
    let (x0, y0, x1, y1) = match s.kind {
        ShapeKind::Rect | ShapeKind::Ellipse => {
            let [w, h] = s.size.unwrap_or_default();
            (-w / 2.0, -h / 2.0, w / 2.0, h / 2.0)
        }
        ShapeKind::Polygon => match Rect::around(points.iter().copied()) {
            Some(r) => (r.min[0], r.min[1], r.max[0], r.max[1]),
            None => return Step::Keep,
        },
        _ => return Step::Keep,
    };
    let Some([nx0, ny0, nx1, ny1]) = resized([x0, y0, x1, y1], kind, d, shift) else {
        return Step::Keep;
    };
    let (w, h, nw, nh) = (x1 - x0, y1 - y0, nx1 - nx0, ny1 - ny0);
    match s.kind {
        ShapeKind::Polygon => {
            let (sx, sy) = (nw / w, nh / h);
            let list = points
                .iter()
                .map(|p| [nx0 + (p[0] - x0) * sx, ny0 + (p[1] - y0) * sy])
                .collect();
            next.points = Some(list);
        }
        _ => {
            next.size = Some([nw, nh]);
            let t = Place::of([0.0, 0.0], s.rotation).turn(mid([nx0, ny0], [nx1, ny1]));
            next.origin = [s.origin[0] + t[0], s.origin[1] + t[1]];
        }
    }
    let readout = Readout::Size {
        width: nw,
        height: nh,
    };
    decoration(id, next, readout)
}

/// The box `[x0, y0, x1, y1]` (in the node's own space) after dragging
/// the side or corner handle `kind` by `d`: the opposite side or corner
/// stays, each side at least [`MIN_SIZE`]; Shift on a corner keeps the
/// aspect ratio. `None` when the box is empty or the result not finite.
fn resized([x0, y0, x1, y1]: [f64; 4], kind: HandleKind, d: P, shift: bool) -> Option<[f64; 4]> {
    let (w, h) = (x1 - x0, y1 - y0);
    if !(w > 0.0 && h > 0.0) {
        return None;
    }
    // Which edges move: -1 the low one, 1 the high one, 0 neither.
    let (ex, ey) = match kind {
        HandleKind::Side(Side::Top) => (0, -1),
        HandleKind::Side(Side::Bottom) => (0, 1),
        HandleKind::Side(Side::Left) => (-1, 0),
        HandleKind::Side(Side::Right) => (1, 0),
        HandleKind::Corner(Corner::TopLeft) => (-1, -1),
        HandleKind::Corner(Corner::TopRight) => (1, -1),
        HandleKind::Corner(Corner::BottomRight) => (1, 1),
        HandleKind::Corner(Corner::BottomLeft) => (-1, 1),
        _ => return None,
    };
    let grow = |e: i32, v: f64, size: f64| match e {
        0 => size,
        _ => (size + f64::from(e) * v).max(MIN_SIZE),
    };
    let (mut nw, mut nh) = (grow(ex, d[0], w), grow(ey, d[1], h));
    if shift && ex != 0 && ey != 0 {
        let k = (nw / w).max(nh / h);
        (nw, nh) = (w * k, h * k);
    }
    if !(nw.is_finite() && nh.is_finite()) {
        return None;
    }
    // The fixed edge on each axis, which the new box keeps.
    let fixed = |e: i32, lo: f64, hi: f64| if e < 0 { hi } else { lo };
    let (ax, ay) = (fixed(ex, x0, x1), fixed(ey, y0, y1));
    let span = |e: i32, a: f64, n: f64, lo: f64, hi: f64| match e {
        0 => (lo, hi),
        e if e < 0 => (a - n, a),
        _ => (a, a + n),
    };
    let (nx0, nx1) = span(ex, ax, nw, x0, x1);
    let (ny0, ny1) = span(ey, ay, nh, y0, y1);
    Some([nx0, ny0, nx1, ny1])
}

/// An image scaled by a side or corner handle, as a rectangle is (07 §5.1):
/// a new `size`, and its centre moved to the new box's.
fn scale_image(i: &Image, id: u32, kind: HandleKind, d: P, shift: bool) -> Step {
    let [w, h] = i.size;
    let Some([nx0, ny0, nx1, ny1]) = resized([-w / 2.0, -h / 2.0, w / 2.0, h / 2.0], kind, d, shift)
    else {
        return Step::Keep;
    };
    let mut next = i.clone();
    next.size = [nx1 - nx0, ny1 - ny0];
    let t = Place::of([0.0, 0.0], i.rotation).turn(mid([nx0, ny0], [nx1, ny1]));
    next.origin = [i.origin[0] + t[0], i.origin[1] + t[1]];
    let readout = Readout::Size {
        width: next.size[0],
        height: next.size[1],
    };
    let property = Property::Decoration(Box::new(Node::Image(next)));
    Step::Show(Command::SetProperty { id, property }, readout)
}

fn decoration(id: u32, shape: Shape, readout: Readout) -> Step {
    let property = Property::Decoration(Box::new(Node::Shape(shape)));
    Step::Show(Command::SetProperty { id, property }, readout)
}
