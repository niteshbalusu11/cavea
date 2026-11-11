//! Arranging the selected nodes (`docs/07-designer.md` §5.1): align,
//! distribute, flip, duplicate, array duplicate, order, lock and hide, as
//! commands. They act on the selected nodes that can be picked, at the top
//! level or in one section; positions and bounds are the index's world
//! bounds, and moves are carried into the parent's space.

use std::collections::BTreeSet;

use cavea_compiler::document::Node;
use cavea_core::limits::MAX_OBJECTS;

use crate::command::{Axis, Command, CommandError, NodeTransform, Property, own_objects};
use crate::document::each_node;
use crate::draft::normalise;
use crate::index::{P, Rect};
use crate::selection::{Level, Selection, View};
use crate::tool::handles::targets;

/// How far a duplicate, or a paste in place, lands from the original, in
/// chart units, right and down.
pub const DUPLICATE_OFFSET: f64 = 50.0;
/// The most copies one array duplicate makes.
pub const MAX_COPIES: u32 = 1_000;
/// The most nodes one array duplicate or paste creates, sections' children
/// included (the per-type limits of 02 §14 are 65,535).
pub const MAX_NEW_NODES: u64 = 65_535;

/// Which edge or centre line `align` brings the nodes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignTo {
    Left,
    Centre,
    Right,
    Top,
    Middle,
    Bottom,
}

/// Where `order` moves the nodes among their siblings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// One step up, past the next sibling not selected.
    Forward,
    Backward,
    Front,
    Back,
}

/// How each copy of an array duplicate is placed relative to the one
/// before: moved by a world vector, or turned about a world point by an
/// angle in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArrayStep {
    Offset(P),
    Around { centre: P, angle: f64 },
}

fn boxes(view: &View<'_>, ids: &[u32]) -> Vec<(u32, Rect)> {
    ids.iter()
        .filter_map(|&id| Some((id, view.index.bounds(id)?)))
        .collect()
}

fn around(boxes: &[(u32, Rect)]) -> Option<Rect> {
    Rect::around(boxes.iter().flat_map(|(_, r)| [r.min, r.max]))
}

/// Moves each node by its world vector, as one `transformNodes`.
fn moves(view: &View<'_>, parent: Option<u32>, by: Vec<(u32, P)>) -> Command {
    let nodes = by
        .into_iter()
        .filter(|(_, d)| *d != [0.0, 0.0])
        .filter_map(|(id, d)| {
            let (o, rotation) = each_node!(view.doc.node(id)?, n => (n.origin, n.rotation));
            let v = view.index.to_parent(parent, d);
            Some(NodeTransform {
                id,
                origin: [o[0] + v[0], o[1] + v[1]],
                rotation,
            })
        })
        .collect();
    Command::TransformNodes { nodes }
}

/// Lines two or more nodes up on an edge or centre of their joint bounds.
pub(crate) fn align(view: &View<'_>, sel: &Selection, to: AlignTo) -> Option<Command> {
    let (ids, parent) = targets(view, sel)?;
    let boxes = boxes(view, &ids);
    let all = around(&boxes).filter(|_| boxes.len() > 1)?;
    let mid = |r: &Rect, i: usize| (r.min[i] + r.max[i]) / 2.0;
    let by = boxes
        .iter()
        .map(|(id, r)| {
            let d = match to {
                AlignTo::Left => [all.min[0] - r.min[0], 0.0],
                AlignTo::Centre => [mid(&all, 0) - mid(r, 0), 0.0],
                AlignTo::Right => [all.max[0] - r.max[0], 0.0],
                AlignTo::Top => [0.0, all.min[1] - r.min[1]],
                AlignTo::Middle => [0.0, mid(&all, 1) - mid(r, 1)],
                AlignTo::Bottom => [0.0, all.max[1] - r.max[1]],
            };
            (*id, d)
        })
        .collect();
    Some(moves(view, parent, by))
}

/// Spaces three or more nodes with equal gaps along `axis` (horizontal: x),
/// in the order of their low edges; the joint bounds stay.
pub(crate) fn distribute(view: &View<'_>, sel: &Selection, axis: Axis) -> Option<Command> {
    let (ids, parent) = targets(view, sel)?;
    let i = usize::from(axis == Axis::Vertical);
    let mut boxes = boxes(view, &ids);
    let all = around(&boxes).filter(|_| boxes.len() > 2)?;
    boxes.sort_by(|a, b| a.1.min[i].total_cmp(&b.1.min[i]).then(a.0.cmp(&b.0)));
    let sizes: f64 = boxes.iter().map(|(_, r)| r.max[i] - r.min[i]).sum();
    let gap = (all.max[i] - all.min[i] - sizes) / (boxes.len() - 1) as f64;
    let mut at = all.min[i];
    let by = boxes
        .iter()
        .map(|(id, r)| {
            let mut d = [0.0; 2];
            d[i] = at - r.min[i];
            at += r.max[i] - r.min[i] + gap;
            (*id, d)
        })
        .collect();
    Some(moves(view, parent, by))
}

/// Mirrors the nodes about the centre of their joint bounds, along the axes
/// of the space holding them (07 §5.1; `flipNodes`).
pub(crate) fn flip(view: &View<'_>, sel: &Selection, axis: Axis) -> Option<Command> {
    let (ids, parent) = targets(view, sel)?;
    let all = around(&boxes(view, &ids))?;
    let centre = [
        (all.min[0] + all.max[0]) / 2.0,
        (all.min[1] + all.max[1]) / 2.0,
    ];
    let c = view.index.to_local(parent, centre);
    let about = if axis == Axis::Horizontal { c[0] } else { c[1] };
    Some(Command::FlipNodes { ids, axis, about })
}

/// The selected nodes in their siblings' order, with their parent.
fn in_order(view: &View<'_>, sel: &Selection) -> Option<(Vec<u32>, Option<u32>)> {
    let (ids, parent) = targets(view, sel)?;
    let kids = view.doc.children(parent).iter().copied();
    Some((kids.filter(|id| ids.contains(id)).collect(), parent))
}

/// Bookable objects and nodes in a subtree.
fn census(node: &Node) -> (u64, u64) {
    let kids = match node {
        Node::Section(s) => s.children.iter().map(census).collect(),
        _ => Vec::new(),
    };
    kids.into_iter()
        .fold((own_objects(node), 1), |a, b| (a.0 + b.0, a.1 + b.1))
}

/// `copies` copies of the selected nodes, each with every id fresh (the
/// `createNode` rule), after the parent's children; copy `i` (from 1) moved
/// or turned `i` steps. Limits are checked before anything is made.
pub(crate) fn array(
    view: &View<'_>,
    sel: &Selection,
    copies: u32,
    step: ArrayStep,
) -> Result<Option<(Command, Level)>, CommandError> {
    let Some((ids, parent)) = in_order(view, sel) else {
        return Ok(None);
    };
    if copies == 0 {
        return Err(CommandError::InvalidValue("count"));
    }
    if copies > MAX_COPIES {
        return Err(CommandError::Limit("copies"));
    }
    let trees: Vec<Node> = ids.iter().filter_map(|&id| view.doc.tree(id)).collect();
    let (objects, nodes) = trees
        .iter()
        .map(census)
        .fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    let n = u64::from(copies);
    if nodes * n > MAX_NEW_NODES {
        return Err(CommandError::Limit("nodes"));
    }
    if view.doc.objects() + objects * n > u64::from(MAX_OBJECTS) {
        return Err(CommandError::Limit("objects"));
    }
    let base = view.doc.children(parent).len();
    let mut commands = Vec::new();
    for i in 1..=copies {
        let k = f64::from(i);
        for tree in &trees {
            let mut node = tree.clone();
            each_node!(&mut node, x => match step {
                ArrayStep::Offset(d) => {
                    let v = view.index.to_parent(parent, d);
                    x.origin = [x.origin[0] + k * v[0], x.origin[1] + k * v[1]];
                }
                ArrayStep::Around { centre, angle } => {
                    let c = view.index.to_local(parent, centre);
                    let (sin, cos) = (k * angle).to_radians().sin_cos();
                    let (dx, dy) = (x.origin[0] - c[0], x.origin[1] - c[1]);
                    x.origin = [c[0] + dx * cos - dy * sin, c[1] + dx * sin + dy * cos];
                    x.rotation = normalise(x.rotation + k * angle);
                }
            });
            let index = u32::try_from(base + commands.len()).unwrap_or(u32::MAX);
            commands.push(Command::CreateNode {
                parent,
                index,
                node: Box::new(node),
            });
        }
    }
    let level = parent.map_or(Level::Top, Level::Section);
    Ok(Some((Command::Composite { commands }, level)))
}

/// Ctrl+D: one copy moved by the world vector `offset`.
pub(crate) fn duplicate(
    view: &View<'_>,
    sel: &Selection,
    offset: P,
) -> Result<Option<(Command, Level)>, CommandError> {
    array(view, sel, 1, ArrayStep::Offset(offset))
}

/// Moves the selected nodes among their siblings, keeping their order, as
/// `placeNode`s.
pub(crate) fn order(view: &View<'_>, sel: &Selection, order: Order) -> Option<Command> {
    let (ids, parent) = in_order(view, sel)?;
    let picked: BTreeSet<u32> = ids.into_iter().collect();
    let on = |id: &u32| picked.contains(id);
    let now = view.doc.children(parent).to_vec();
    let mut want = now.clone();
    match order {
        Order::Front | Order::Back => {
            let (mine, rest): (Vec<u32>, Vec<u32>) = now.iter().partition(|id| on(id));
            want = match order {
                Order::Front => [rest, mine].concat(),
                _ => [mine, rest].concat(),
            };
        }
        Order::Forward => {
            for i in (0..want.len().saturating_sub(1)).rev() {
                if want.get(i).is_some_and(on) && !want.get(i + 1).is_some_and(on) {
                    want.swap(i, i + 1);
                }
            }
        }
        Order::Backward => {
            for i in 1..want.len() {
                if want.get(i).is_some_and(on) && !want.get(i - 1).is_some_and(on) {
                    want.swap(i, i - 1);
                }
            }
        }
    }
    // Each place counts after the node has left its own (07 §2.2).
    let (mut cur, mut commands) = (now, Vec::new());
    for (i, &id) in want.iter().enumerate() {
        if cur.get(i) != Some(&id) {
            let j = cur.iter().position(|&c| c == id)?;
            cur.remove(j);
            cur.insert(i, id);
            let index = u32::try_from(i).ok()?;
            commands.push(Command::PlaceNode { id, parent, index });
        }
    }
    Some(Command::Composite { commands })
}

/// Locks or hides (or the reverse) every selected node, those that cannot
/// be picked included: that is how they are unlocked and shown again.
pub(crate) fn set_flag(sel: &Selection, property: Property) -> Option<Command> {
    if !matches!(sel.level(), Level::Top | Level::Section(_)) {
        return None;
    }
    let commands = sel
        .items()
        .iter()
        .map(|&id| Command::SetProperty {
            id,
            property: property.clone(),
        })
        .collect();
    Some(Command::Composite { commands })
}
