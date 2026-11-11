//! The layers panel (`docs/07-designer.md` §3, §5.5, §14): the floor
//! shown's top-level nodes in document (draw) order, a section's children
//! under it while it is expanded, fed to a virtualised list page by page;
//! selecting from the tree, and dragging rows to reorder and reparent.
//!
//! A page is found by binary search over where each top-level row starts,
//! a list rebuilt only when the tree, a node's content, the expansion or
//! the floor shown changes: never a walk of the whole tree per page.

use std::collections::{BTreeMap, BTreeSet};

use cavea_compiler::document::Node;
use serde::{Deserialize, Serialize};

use crate::command::{Command, CommandError, Property};
use crate::document::{Document, each_node};
use crate::selection::{Level, Selection, floor_of};
use crate::views::NodeKind;

/// One row of the tree.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerRow {
    /// Its place in the list as shown, from 0.
    pub index: u32,
    /// 0 at the top level, 1 for a section's child.
    pub depth: u8,
    pub id: u32,
    pub kind: NodeKind,
    /// The node's `name`, else its label, else a text's string; `null`
    /// when it has none, and the shell shows its kind and id ("Row block
    /// 12"), translated.
    pub name: Option<String>,
    pub locked: bool,
    pub hidden: bool,
    pub selected: bool,
    /// For a section: whether its children are listed under it.
    pub expanded: Option<bool>,
    /// A section's children.
    pub children: u32,
}

/// A window of the tree: `rows` from `start`, of `total`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayersPage {
    pub total: u32,
    pub start: u32,
    pub rows: Vec<LayerRow>,
}

/// How a click on a row selects: alone, toggled (Ctrl), or the range from
/// the last row clicked (Shift).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LayerClick {
    Replace,
    Toggle,
    Range,
}

/// Where dragged rows land: before or after a row, or at the end of a
/// section's children. In JSON `{"kind": "before", "id": 12}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
pub enum Drop {
    Before(u32),
    After(u32),
    Into(u32),
}

#[derive(Debug, Default)]
pub(crate) struct Layers {
    expanded: BTreeSet<u32>,
    /// Raised when the expansion changes.
    pub(crate) generation: u64,
    /// The row last clicked, where a Shift-click's range starts.
    anchor: Option<u32>,
    list: Option<List>,
}

/// The top-level rows of the floor shown and where each starts.
#[derive(Debug)]
struct List {
    key: (u64, u64, Option<u32>),
    roots: Vec<u32>,
    starts: Vec<u32>,
    total: u32,
}

/// The node's own name, label or text.
fn name(node: &Node) -> Option<String> {
    let label = match node {
        Node::Section(n) => n.label.as_ref(),
        Node::Row(n) => n.label.as_ref(),
        Node::Table(n) => n.label.as_ref(),
        Node::Booth(n) => n.label.as_ref(),
        Node::Area(n) => n.label.as_ref(),
        Node::Text(n) => Some(&n.text),
        Node::RowBlock(_) | Node::Shape(_) | Node::Icon(_) | Node::Image(_) => None,
    };
    each_node!(node, n => n.name.as_ref()).or(label).cloned()
}

fn expandable(doc: &Document, id: u32) -> bool {
    matches!(doc.node(id), Some(Node::Section(_)))
}

/// The list for the document, expansion and floor, rebuilt if stale.
fn list<'a>(
    slot: &'a mut Option<List>,
    expanded: &BTreeSet<u32>,
    key: (u64, u64, Option<u32>),
    doc: &Document,
) -> &'a List {
    match slot.take() {
        Some(list) if list.key == key => slot.insert(list),
        _ => {
            let floor = key.2;
            let roots: Vec<u32> = doc
                .children(None)
                .iter()
                .copied()
                .filter(|&id| floor_of(doc, id) == floor)
                .collect();
            let mut starts = Vec::with_capacity(roots.len());
            let mut total = 0u32;
            for &id in &roots {
                starts.push(total);
                let kids = shown_children(doc, expanded, id).len();
                total = total.saturating_add(1 + u32::try_from(kids).unwrap_or(u32::MAX));
            }
            slot.insert(List {
                key,
                roots,
                starts,
                total,
            })
        }
    }
}

/// The children listed under `id`: a section's, while it is expanded.
fn shown_children<'a>(doc: &'a Document, expanded: &BTreeSet<u32>, id: u32) -> &'a [u32] {
    if expanded.contains(&id) && expandable(doc, id) {
        doc.children(Some(id))
    } else {
        &[]
    }
}

impl Layers {
    fn list(&mut self, doc: &Document, floor: Option<u32>) -> &List {
        let key = (doc.structure_revision(), self.generation, floor);
        list(&mut self.list, &self.expanded, key, doc)
    }

    /// `count` rows from `start` (a page of the virtualised list): found by
    /// binary search, then only the rows of the page are visited.
    pub(crate) fn page(
        &mut self,
        doc: &Document,
        floor: Option<u32>,
        sel: &Selection,
        start: u32,
        count: u32,
    ) -> LayersPage {
        let key = (doc.structure_revision(), self.generation, floor);
        let expanded = &self.expanded;
        let list = list(&mut self.list, expanded, key, doc);
        let total = list.total;
        let picked = |id| sel.items().contains(&id);
        let row = |index, depth, id: u32, selected| {
            let node = doc.node(id)?;
            let (locked, hidden) = each_node!(node, n => (n.locked, n.hidden));
            Some(LayerRow {
                index,
                depth,
                id,
                kind: NodeKind::of(node),
                name: name(node),
                locked,
                hidden,
                selected,
                expanded: expandable(doc, id).then(|| expanded.contains(&id)),
                children: u32::try_from(doc.children(Some(id)).len()).unwrap_or(u32::MAX),
            })
        };
        let end = start.saturating_add(count).min(total);
        let mut rows = Vec::new();
        // The top-level row at or before `start`.
        let mut k = list
            .starts
            .partition_point(|&s| s <= start)
            .saturating_sub(1);
        while let (Some(&id), Some(&at)) = (list.roots.get(k), list.starts.get(k)) {
            if at >= end {
                break;
            }
            if at >= start {
                let top = matches!(sel.level(), Level::Top) && picked(id);
                rows.extend(row(at, 0, id, top));
            }
            let inside = sel.level() == Level::Section(id);
            let kids = shown_children(doc, expanded, id);
            // Children from the first one in the page.
            let skip = start.saturating_sub(at + 1);
            let rest = kids.get(skip as usize..).unwrap_or_default();
            for (index, &c) in (at + 1 + skip..end).zip(rest) {
                rows.extend(row(index, 1, c, inside && picked(c)));
            }
            k += 1;
        }
        LayersPage { total, start, rows }
    }

    pub(crate) fn expand(&mut self, id: u32, open: bool) {
        let changed = if open {
            self.expanded.insert(id)
        } else {
            self.expanded.remove(&id)
        };
        if changed {
            self.generation += 1;
        }
    }

    /// Expands the section holding `id` and returns the row it is on, if
    /// it is in the tree of the floor shown.
    pub(crate) fn reveal(&mut self, doc: &Document, floor: Option<u32>, id: u32) -> Option<u32> {
        let parent = doc.parent(id);
        let top = parent.unwrap_or(id);
        if !self.list(doc, floor).roots.contains(&top) {
            return None;
        }
        if let Some(s) = parent {
            self.expand(s, true);
        }
        let list = self.list(doc, floor);
        let k = list.roots.iter().position(|&r| r == top)?;
        let at = *list.starts.get(k)?;
        match parent {
            None => Some(at),
            Some(s) => {
                let i = doc.children(Some(s)).iter().position(|&c| c == id)?;
                Some(at + 1 + u32::try_from(i).ok()?)
            }
        }
    }

    /// The selection a click on row `id` makes, from `sel`.
    pub(crate) fn click(
        &mut self,
        doc: &Document,
        floor: Option<u32>,
        sel: &Selection,
        id: u32,
        how: LayerClick,
    ) -> Option<(Level, Vec<u32>)> {
        doc.node(id)?;
        let parent = doc.parent(id);
        let level = parent.map_or(Level::Top, Level::Section);
        let anchor = self.anchor.replace(id);
        let same = sel.level() == level;
        let items = match how {
            LayerClick::Toggle if same => {
                let mut items = sel.items().clone();
                if !items.remove(&id) {
                    items.insert(id);
                }
                items.into_iter().collect()
            }
            LayerClick::Range => {
                let siblings: Vec<u32> = doc
                    .children(parent)
                    .iter()
                    .copied()
                    .filter(|&s| parent.is_some() || floor_of(doc, s) == floor)
                    .collect();
                let at = |x| siblings.iter().position(|&s| s == x);
                match (anchor.and_then(at), at(id)) {
                    (Some(a), Some(b)) => {
                        let (lo, hi) = (a.min(b), a.max(b));
                        // Keep the range's start for the next Shift-click.
                        self.anchor = anchor;
                        siblings.get(lo..=hi).unwrap_or_default().to_vec()
                    }
                    _ => vec![id],
                }
            }
            LayerClick::Replace | LayerClick::Toggle => vec![id],
        };
        Some((level, items))
    }
}

/// The commands that move `ids` to `drop`, in document order, keeping each
/// on the canvas (`placeNode`) and on the floor it is shown on.
pub(crate) fn place(doc: &Document, ids: &[u32], drop: Drop) -> Result<Command, CommandError> {
    let moved: BTreeSet<u32> = ids.iter().copied().collect();
    if let Some(&id) = moved.iter().find(|&&id| doc.node(id).is_none()) {
        return Err(CommandError::MissingNode(id));
    }
    // Document order: each node's top-level position, then its own.
    let mut rank = BTreeMap::new();
    for (i, &top) in (0u32..).zip(doc.children(None)) {
        rank.insert(top, (i, 0u32));
        for (j, &c) in (1u32..).zip(doc.children(Some(top))) {
            rank.insert(c, (i, j));
        }
    }
    let mut order: Vec<u32> = moved.iter().copied().collect();
    order.sort_by_key(|id| rank.get(id).copied().unwrap_or((u32::MAX, u32::MAX)));
    let (parent, mut follower) = match drop {
        Drop::Into(s) => {
            if !matches!(doc.node(s), Some(Node::Section(_))) {
                return Err(CommandError::NotASection(s));
            }
            (Some(s), None)
        }
        Drop::Before(x) | Drop::After(x) => {
            doc.node(x).ok_or(CommandError::MissingNode(x))?;
            if moved.contains(&x) {
                return Err(CommandError::InvalidValue("target"));
            }
            let parent = doc.parent(x);
            let next = match drop {
                Drop::After(_) => {
                    let sibs = doc.children(parent);
                    let at = sibs.iter().position(|&s| s == x);
                    let rest = at.and_then(|i| sibs.get(i + 1..)).unwrap_or_default();
                    rest.iter().copied().find(|s| !moved.contains(s))
                }
                _ => Some(x),
            };
            (parent, next)
        }
    };
    if let Some(p) = parent {
        if moved.contains(&p) {
            return Err(CommandError::Cycle { id: p, parent: p });
        }
        let sections = order
            .iter()
            .any(|&id| matches!(doc.node(id), Some(Node::Section(_))));
        if sections || doc.parent(p).is_some() {
            return Err(CommandError::Placement);
        }
    }
    // Each `placeNode` index counts after the node has left its place, so
    // the list is simulated, last node first, each before the one after it.
    let mut list = doc.children(parent).to_vec();
    let mut commands = Vec::with_capacity(order.len() * 2);
    for &id in order.iter().rev() {
        list.retain(|&c| c != id);
        let at = follower
            .and_then(|f| list.iter().position(|&c| c == f))
            .unwrap_or(list.len());
        list.insert(at, id);
        let index = u32::try_from(at).map_err(|_| CommandError::IndexOutOfRange)?;
        commands.push(Command::PlaceNode { id, parent, index });
        follower = Some(id);
    }
    // A child leaving for the top level keeps its section's floor.
    if parent.is_none() {
        let listed = |f: u32| doc.floors().iter().any(|l| l.id == f);
        let first = doc.floors().first().map(|f| f.id);
        for &id in &order {
            let was = floor_of(doc, id);
            let own = doc.node(id).and_then(|n| each_node!(n, x => x.floor));
            if doc.parent(id).is_some() && own.filter(|&f| listed(f)).or(first) != was {
                let property = Property::Floor(was);
                commands.push(Command::SetProperty { id, property });
            }
        }
    }
    Ok(Command::Composite { commands })
}
