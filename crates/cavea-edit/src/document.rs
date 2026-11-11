//! The editor's document (`docs/07-designer.md` §2): the chart-level data and
//! an arena of the compiler's own node values, keyed by node id.
//!
//! A node is stored as the [`Node`] it was loaded as, except that a section's
//! `children` is always empty: the arena holds the tree as parent and child
//! links instead, so a single node is found, changed or moved without touching
//! its neighbours. Saving rebuilds the nested form, so a document loaded and
//! saved without edits gives exactly the JSON it was loaded from.
//!
//! Change tracking: a revision counter goes up with every change; each node
//! keeps the revision of its last change (`stamp`) and of its last change of
//! content (`content`: anything but its transform and place in the tree),
//! and removed ids are kept with the revision that removed them.
//! [`Document::changes_since`] answers "what changed since revision n".

use std::collections::BTreeMap;
use std::fmt;

use cavea_compiler::ChartDocument;
use cavea_compiler::document::{Category, Floor, Node, Settings};

/// Why a chart document cannot be opened in the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// Two nodes share an id, so commands could not name either (E009).
    DuplicateNodeId(u32),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateNodeId(id) => write!(f, "two nodes have id {id}"),
        }
    }
}

impl std::error::Error for LoadError {}

#[derive(Debug)]
pub(crate) struct Slot {
    /// A section's `children` is always empty; see `children`.
    pub(crate) node: Node,
    pub(crate) parent: Option<u32>,
    pub(crate) children: Vec<u32>,
    pub(crate) stamp: u64,
    pub(crate) content: u64,
}

/// What changed since a revision, for the live scene and the spatial index.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Nodes added or changed (content, transform, parent or children), by id.
    /// A section's transform moves its children, which are not listed.
    pub nodes: Vec<u32>,
    /// Nodes removed and not restored since, by id.
    pub removed: Vec<u32>,
    /// Chart name, settings, categories, floors or `nextOid` changed.
    pub chart: bool,
    /// The order of the top-level nodes changed.
    pub roots: bool,
}

#[derive(Debug)]
pub struct Document {
    pub(crate) schema: u32,
    pub(crate) name: String,
    pub(crate) settings: Settings,
    pub(crate) next_oid: u32,
    pub(crate) categories: Vec<Category>,
    pub(crate) floors: Vec<Floor>,
    pub(crate) slots: BTreeMap<u32, Slot>,
    pub(crate) roots: Vec<u32>,
    pub(crate) revision: u64,
    pub(crate) chart_stamp: u64,
    pub(crate) roots_stamp: u64,
    /// The revision of the last change that was not only a transform: the
    /// tree, a node's content or chart-level data (views that show no
    /// position need nothing else).
    pub(crate) structure: u64,
    pub(crate) removed: BTreeMap<u32, u64>,
    /// The next id to allocate: never decreases, and is above every id the
    /// document held when loaded. Undo does not touch it.
    pub(crate) fresh: u64,
    /// One past the last id allocated in this session, or 0.
    pub(crate) high_water: u32,
}

impl Document {
    /// Opens a parsed chart document. Fails only on duplicate node ids.
    pub fn load(document: ChartDocument) -> Result<Self, LoadError> {
        let ChartDocument {
            schema,
            name,
            settings,
            next_oid,
            categories,
            floors,
            nodes,
        } = document;
        let mut doc = Self {
            schema,
            name,
            settings,
            next_oid,
            categories,
            floors,
            slots: BTreeMap::new(),
            roots: Vec::with_capacity(nodes.len()),
            revision: 0,
            chart_stamp: 0,
            roots_stamp: 0,
            structure: 0,
            removed: BTreeMap::new(),
            fresh: u64::from(next_oid),
            high_water: 0,
        };
        for node in nodes {
            let id = node_id(&node);
            doc.fresh = doc.fresh.max(max_id(&node));
            doc.insert_tree(None, node)
                .map_err(LoadError::DuplicateNodeId)?;
            doc.roots.push(id);
        }
        Ok(doc)
    }

    /// The document exactly as it stands, `nextOid` included: what undo
    /// restores byte for byte.
    pub fn snapshot(&self) -> ChartDocument {
        self.build(self.next_oid)
    }

    /// The document to store: [`Self::snapshot`] with `nextOid` raised past
    /// every id this session allocated, even ids whose creation was undone, so
    /// that a stored draft never lets an id be handed out twice (07 §9).
    pub fn save(&self) -> ChartDocument {
        self.build(self.next_oid.max(self.high_water))
    }

    fn build(&self, next_oid: u32) -> ChartDocument {
        ChartDocument {
            schema: self.schema,
            name: self.name.clone(),
            settings: self.settings.clone(),
            next_oid,
            categories: self.categories.clone(),
            floors: self.floors.clone(),
            nodes: self.roots.iter().filter_map(|&id| self.tree(id)).collect(),
        }
    }

    /// The node `id` with its subtree, in the nested form.
    pub fn tree(&self, id: u32) -> Option<Node> {
        let slot = self.slots.get(&id)?;
        let mut node = slot.node.clone();
        if let Node::Section(s) = &mut node {
            s.children = slot.children.iter().filter_map(|&c| self.tree(c)).collect();
        }
        Some(node)
    }

    /// The current revision; every change raises it.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The revision of the last change other than a node's transform.
    pub fn structure_revision(&self) -> u64 {
        self.structure
    }

    pub fn changes_since(&self, revision: u64) -> Changes {
        Changes {
            nodes: self
                .slots
                .iter()
                .filter(|(_, s)| s.stamp > revision)
                .map(|(&id, _)| id)
                .collect(),
            removed: self
                .removed
                .iter()
                .filter(|&(_, &r)| r > revision)
                .map(|(&id, _)| id)
                .collect(),
            chart: self.chart_stamp > revision,
            roots: self.roots_stamp > revision,
        }
    }

    /// The node's own data. A section's `children` is empty: see
    /// [`Self::children`].
    pub fn node(&self, id: u32) -> Option<&Node> {
        self.slots.get(&id).map(|s| &s.node)
    }

    /// The node's section, or `None` for a top-level or unknown node.
    pub fn parent(&self, id: u32) -> Option<u32> {
        self.slots.get(&id).and_then(|s| s.parent)
    }

    /// Children in document (and draw) order: the top-level nodes for `None`.
    pub fn children(&self, parent: Option<u32>) -> &[u32] {
        match parent {
            None => &self.roots,
            Some(id) => self.slots.get(&id).map_or(&[], |s| &s.children),
        }
    }

    /// Revision of the node's last change of any kind, and of its content.
    pub fn stamps(&self, id: u32) -> Option<(u64, u64)> {
        self.slots.get(&id).map(|s| (s.stamp, s.content))
    }

    /// Node ids in ascending order.
    pub fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.slots.keys().copied()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn categories(&self) -> &[Category] {
        &self.categories
    }

    pub fn floors(&self) -> &[Floor] {
        &self.floors
    }

    pub fn next_oid(&self) -> u32 {
        self.next_oid
    }

    /// Inserts a subtree under `parent` (not into its child list). Stops at
    /// the first id already taken and returns it.
    pub(crate) fn insert_tree(&mut self, parent: Option<u32>, mut node: Node) -> Result<(), u32> {
        let id = node_id(&node);
        if self.slots.contains_key(&id) {
            return Err(id);
        }
        let kids = match &mut node {
            Node::Section(s) => std::mem::take(&mut s.children),
            _ => Vec::new(),
        };
        let children = kids.iter().map(node_id).collect();
        self.removed.remove(&id);
        self.slots.insert(
            id,
            Slot {
                node,
                parent,
                children,
                stamp: self.revision,
                content: self.revision,
            },
        );
        kids.into_iter()
            .try_for_each(|k| self.insert_tree(Some(id), k))
    }

    /// Removes a subtree from the arena (not from its parent's child list).
    pub(crate) fn take_tree(&mut self, id: u32) -> Option<Node> {
        let slot = self.slots.remove(&id)?;
        self.removed.insert(id, self.revision);
        let mut node = slot.node;
        if let Node::Section(s) = &mut node {
            s.children = slot
                .children
                .iter()
                .filter_map(|&c| self.take_tree(c))
                .collect();
        }
        Some(node)
    }

    /// Takes `n` fresh ids; the first is returned.
    pub(crate) fn allocate(&mut self, n: u64) -> Option<u32> {
        let first = u32::try_from(self.fresh).ok()?;
        let end = self.fresh.checked_add(n)?;
        // `nextOid` is a u32 one past the last id.
        let high = u32::try_from(end).ok()?;
        self.fresh = end;
        self.high_water = high;
        Some(first)
    }
}

/// Applies `$body` to the struct inside any [`Node`], bound as `$n`.
macro_rules! each_node {
    ($node:expr, $n:ident => $body:expr) => {
        match $node {
            Node::Section($n) => $body,
            Node::RowBlock($n) => $body,
            Node::Row($n) => $body,
            Node::Table($n) => $body,
            Node::Booth($n) => $body,
            Node::Area($n) => $body,
            Node::Shape($n) => $body,
            Node::Text($n) => $body,
            Node::Icon($n) => $body,
            Node::Image($n) => $body,
        }
    };
}
pub(crate) use each_node;

pub(crate) fn node_id(node: &Node) -> u32 {
    each_node!(node, n => n.id)
}

/// One past the largest id, `rid` or `oid` in the subtree.
fn max_id(node: &Node) -> u64 {
    let end = |first: u32, count: u32| u64::from(first) + u64::from(count);
    let runs = |runs: &[[u32; 2]]| runs.iter().map(|r| end(r[0], r[1])).max().unwrap_or(0);
    let keys = |m: &BTreeMap<u32, _>| m.keys().next_back().map_or(0, |&k| end(k, 1));
    let own = end(node_id(node), 1);
    let inner = match node {
        Node::Section(s) => s.children.iter().map(max_id).max().unwrap_or(0),
        Node::RowBlock(b) => b
            .rows
            .iter()
            .map(|r| end(r.rid, 1).max(runs(&r.oids)))
            .max()
            .unwrap_or(0)
            .max(keys(&b.seat_overrides)),
        Node::Row(r) => runs(&r.oids).max(keys(&r.seat_overrides)),
        Node::Table(t) => runs(&t.oids).max(t.table_oid.map_or(0, |o| end(o, 1))),
        Node::Booth(b) => end(b.oid, 1),
        Node::Area(a) => end(a.oid, 1),
        Node::Shape(_) | Node::Text(_) | Node::Icon(_) | Node::Image(_) => 0,
    };
    own.max(inner)
}
