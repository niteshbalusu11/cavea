//! Copy and paste (`docs/07-designer.md` §2.3, §5.1). The edit core keeps
//! no clipboard: `copy` gives the selection as text for the chrome to put
//! on the system clipboard, and `paste` takes such text back, from this
//! chart or another. The text is arbitrary input, so it is read
//! defensively: bounded in size, parsed strictly, every refusal typed.
//!
//! The form, version 1, is one JSON object written by serde, so the same
//! selection always gives the same bytes:
//! `{"format":"cavea-clipboard","version":1,"centre":[x,y],"categories":[…],"nodes":[…]}`.
//! `nodes` are the selected nodes with their subtrees, in document order,
//! placed in world space and without `floor` fields; `centre` is the middle
//! of their world bounds; `categories` are the chart's definitions of every
//! category key they name, in the chart's order.

use std::collections::BTreeSet;

use cavea_compiler::document::{Category, Node};
use cavea_core::limits::MAX_CATEGORIES;
use serde::{Deserialize, Serialize};

use crate::arrange::{DUPLICATE_OFFSET, MAX_NEW_NODES};
use crate::command::{Command, CommandError, category};
use crate::document::each_node;
use crate::draft::normalise;
use crate::index::{P, Place, Rect};
use crate::selection::{Level, Selection, View};
use crate::tool::Spot;
use crate::tool::handles::targets;

const FORMAT: &str = "cavea-clipboard";
const VERSION: u32 = 1;
/// The longest clipboard text `paste` reads (a 20,000-seat section is
/// well under a megabyte).
pub const MAX_CLIPBOARD: usize = 8 << 20;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Clip {
    format: String,
    version: u32,
    centre: P,
    categories: Vec<Category>,
    nodes: Vec<Node>,
}

/// The world place of `parent`'s children, and its rotation.
fn space(view: &View<'_>, parent: Option<u32>) -> (Place, f64) {
    let node = parent.and_then(|p| view.doc.node(p));
    let rotation = node.map_or(0.0, |n| each_node!(n, x => x.rotation));
    let place = parent.and_then(|p| view.index.place(p));
    (place.unwrap_or(Place::of([0.0, 0.0], 0.0)), rotation)
}

/// Drops the `floor` of a node and its children, collecting the categories
/// they name.
fn strip(node: &mut Node, keys: &mut BTreeSet<String>) {
    each_node!(&mut *node, n => n.floor = None);
    keys.extend(category(node).cloned().flatten());
    match node {
        Node::Section(s) => s.children.iter_mut().for_each(|c| strip(c, keys)),
        Node::RowBlock(b) => {
            keys.extend(b.seat_overrides.values().filter_map(|o| o.category.clone()))
        }
        Node::Row(r) => keys.extend(r.seat_overrides.values().filter_map(|o| o.category.clone())),
        _ => {}
    }
}

/// The selected nodes as clipboard text (see the module comment).
pub(crate) fn copy(view: &View<'_>, sel: &Selection) -> Option<String> {
    let (ids, parent) = targets(view, sel)?;
    let kids = view.doc.children(parent).iter();
    let ids: Vec<u32> = kids.copied().filter(|id| ids.contains(id)).collect();
    let (place, turn) = space(view, parent);
    let mut keys = BTreeSet::new();
    let mut nodes = Vec::with_capacity(ids.len());
    for &id in &ids {
        let mut node = view.doc.tree(id)?;
        if parent.is_some() {
            each_node!(&mut node, n => {
                n.origin = place.apply(n.origin);
                if turn != 0.0 {
                    n.rotation = normalise(n.rotation + turn);
                }
            });
        }
        strip(&mut node, &mut keys);
        nodes.push(node);
    }
    let all = Rect::around(
        ids.iter()
            .filter_map(|&id| view.index.bounds(id))
            .flat_map(|r| [r.min, r.max]),
    )?;
    let clip = Clip {
        format: FORMAT.into(),
        version: VERSION,
        centre: [
            (all.min[0] + all.max[0]) / 2.0,
            (all.min[1] + all.max[1]) / 2.0,
        ],
        categories: view
            .doc
            .categories()
            .iter()
            .filter(|c| keys.contains(&c.key))
            .cloned()
            .collect(),
        nodes,
    };
    serde_json::to_string(&clip).ok()
}

fn count(node: &Node) -> u64 {
    match node {
        Node::Section(s) => 1 + s.children.iter().map(count).sum::<u64>(),
        _ => 1,
    }
}

/// The command pasting clipboard text, and the level its nodes land at:
/// in the section whose children are selected, or beside the node whose
/// rows or seats are, else at the top level on the floor shown. With `at`
/// (world) the nodes' centre goes there; without, they land where they
/// were copied from, moved by [`DUPLICATE_OFFSET`]. Every id is fresh
/// (`createNode`); category keys the chart lacks are added with the
/// clipboard's definitions, and keys it has keep the chart's.
pub(crate) fn paste(
    view: &View<'_>,
    sel: &Selection,
    text: &str,
    at: Option<P>,
) -> Result<(Command, Level), CommandError> {
    if text.len() > MAX_CLIPBOARD {
        return Err(CommandError::Limit("clipboard bytes"));
    }
    let clip: Clip =
        serde_json::from_str(text).map_err(|_| CommandError::InvalidValue("clipboard"))?;
    if clip.format != FORMAT || clip.version != VERSION {
        return Err(CommandError::InvalidValue("clipboard version"));
    }
    if clip.categories.len() > MAX_CATEGORIES as usize {
        return Err(CommandError::Limit("categories"));
    }
    if clip.nodes.iter().map(count).sum::<u64>() > MAX_NEW_NODES {
        return Err(CommandError::Limit("nodes"));
    }
    let parent = match sel.level() {
        Level::Top => None,
        Level::Section(s) => Some(s),
        Level::Rows(h) | Level::Seats(h) => view.doc.parent(h),
    };
    if parent.is_some() && clip.nodes.iter().any(|n| matches!(n, Node::Section(_))) {
        return Err(CommandError::Placement);
    }
    let d = match at {
        Some(p) => [p[0] - clip.centre[0], p[1] - clip.centre[1]],
        None => [DUPLICATE_OFFSET; 2],
    };
    let spot = Spot::new(view.doc, parent, view.floor);
    let (place, turn) = space(view, parent);
    let mut commands = Vec::new();
    let mut known: BTreeSet<&str> = view
        .doc
        .categories()
        .iter()
        .map(|c| c.key.as_str())
        .collect();
    for c in &clip.categories {
        if known.insert(&c.key) {
            let index = u32::try_from(known.len() - 1).unwrap_or(u32::MAX);
            commands.push(Command::AddCategory {
                index,
                category: c.clone(),
            });
        }
    }
    for (k, mut node) in clip.nodes.into_iter().enumerate() {
        strip(&mut node, &mut BTreeSet::new());
        each_node!(&mut node, n => {
            let world = [n.origin[0] + d[0], n.origin[1] + d[1]];
            n.origin = place.invert(world);
            if turn != 0.0 {
                n.rotation = normalise(n.rotation - turn);
            }
            n.floor = spot.floor;
        });
        let index = u32::try_from(spot.index as usize + k).unwrap_or(u32::MAX);
        commands.push(Command::CreateNode {
            parent,
            index,
            node: Box::new(node),
        });
    }
    Ok((Command::Composite { commands }, spot.level()))
}
