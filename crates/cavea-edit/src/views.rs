//! The shell's small views and which of them are out of date
//! (`docs/07-designer.md` §2, §14): the selection summary, the status bar,
//! the undo history and the floors, and [`ViewName`], the names
//! `Session::changed` reports. Each view is a serde value (camelCase).

use cavea_compiler::document::Node;
use serde::Serialize;

use crate::document::{Document, each_node};
use crate::index::Rect;
use crate::selection::{Level, floor_of};

/// A view of the session the shell renders. `Session::changed` lists those
/// that may differ from when it last asked; the others are as they were.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ViewName {
    /// `Session::history`.
    History,
    /// `Session::title`.
    Title,
    /// `Session::floors`.
    Floors,
    /// `Session::status`.
    Status,
    /// `Session::selection_view` and `Session::fit` with it.
    Selection,
    /// `Session::inspector`.
    Inspector,
    /// `Session::layers`.
    Layers,
    /// `Session::categories`.
    Categories,
    /// `Session::numbering` and `Session::numbering_preview`.
    Numbering,
    /// `Session::validation` (and the overlay's markers).
    Validation,
    /// `Session::overlay`.
    Overlay,
    /// `Session::cursor`.
    Cursor,
    /// `Session::tool_view`.
    Tool,
    /// `Session::guides` and `Session::saved_guides`.
    Guides,
}

/// Every view, in the order `Session::changed` lists them.
pub const VIEWS: [ViewName; 14] = [
    ViewName::History,
    ViewName::Title,
    ViewName::Floors,
    ViewName::Status,
    ViewName::Selection,
    ViewName::Inspector,
    ViewName::Layers,
    ViewName::Categories,
    ViewName::Numbering,
    ViewName::Validation,
    ViewName::Overlay,
    ViewName::Cursor,
    ViewName::Tool,
    ViewName::Guides,
];

/// What a view is built from, as numbers that change whenever it may: two
/// equal marks mean an equal view.
pub(crate) type Mark = [u64; 8];

/// The marks each view had when the shell last asked.
#[derive(Debug, Default)]
pub(crate) struct Seen(pub(crate) [Option<Mark>; VIEWS.len()]);

/// A node's `type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NodeKind {
    Section,
    RowBlock,
    Row,
    Table,
    Booth,
    Area,
    Shape,
    Text,
    Icon,
    Image,
}

impl NodeKind {
    pub fn of(node: &Node) -> Self {
        match node {
            Node::Section(_) => Self::Section,
            Node::RowBlock(_) => Self::RowBlock,
            Node::Row(_) => Self::Row,
            Node::Table(_) => Self::Table,
            Node::Booth(_) => Self::Booth,
            Node::Area(_) => Self::Area,
            Node::Shape(_) => Self::Shape,
            Node::Text(_) => Self::Text,
            Node::Icon(_) => Self::Icon,
            Node::Image(_) => Self::Image,
        }
    }
}

/// The selection in a line, for the inspector's header and the status bar.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionView {
    pub level: Level,
    /// Items selected: nodes, rows or seat positions by level.
    pub count: u32,
    /// At a node level, how many of each kind, in [`NodeKind`] order.
    pub kinds: Vec<KindCount>,
    /// Selected nodes that are locked, and hidden (node levels).
    pub locked: u32,
    pub hidden: u32,
    /// The world box around the selected items' outlines: the box "zoom to
    /// selection" frames.
    pub bounds: Option<Rect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KindCount {
    pub kind: NodeKind,
    pub count: u32,
}

pub(crate) fn kinds(doc: &Document, level: Level, items: &[u32]) -> (Vec<KindCount>, u32, u32) {
    if !matches!(level, Level::Top | Level::Section(_)) {
        return (Vec::new(), 0, 0);
    }
    let mut counts = std::collections::BTreeMap::new();
    let (mut locked, mut hidden) = (0, 0);
    for node in items.iter().filter_map(|&id| doc.node(id)) {
        *counts.entry(NodeKind::of(node)).or_insert(0) += 1;
        let (l, h) = each_node!(node, n => (n.locked, n.hidden));
        locked += u32::from(l);
        hidden += u32::from(h);
    }
    let kinds = counts
        .into_iter()
        .map(|(kind, count)| KindCount { kind, count })
        .collect();
    (kinds, locked, hidden)
}

/// The status bar's numbers (07 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusView {
    /// Seats in the chart: positions not removed, in nodes not hidden (what
    /// the compiler compiles).
    pub seats: u64,
    /// Nodes in the chart, children included.
    pub nodes: u32,
    /// The document's revision: it changes with every edit, undo and redo,
    /// so a shell that notes it when a save succeeds knows whether there
    /// is anything left to save.
    pub revision: u64,
}

pub(crate) fn status(doc: &Document) -> StatusView {
    let hidden = |id: u32| doc.node(id).is_some_and(|n| each_node!(n, x => x.hidden));
    let mut seats = 0u64;
    for id in doc.ids() {
        if hidden(id) || doc.parent(id).is_some_and(hidden) {
            continue;
        }
        // A position listed twice, or past the row's end, removes no more:
        // the compiler reads `removed` as a set of positions.
        let present = |n: u32, removed: &[u32]| {
            let gone: std::collections::BTreeSet<u32> =
                removed.iter().copied().filter(|&p| p < n).collect();
            u64::from(n) - gone.len() as u64
        };
        seats += match doc.node(id) {
            Some(Node::RowBlock(b)) => b.rows.iter().map(|r| present(r.seats, &r.removed)).sum(),
            Some(Node::Row(r)) => present(r.seats, &r.removed),
            _ => 0,
        };
    }
    StatusView {
        seats,
        nodes: u32::try_from(doc.ids().count()).unwrap_or(u32::MAX),
        revision: doc.revision(),
    }
}

/// Undo and redo for the top bar (07 §9): whether each is possible and the
/// key of what it would undo or redo ([`crate::Command::label`], or the
/// session call's name: `move`, `rotate`, `align`, `inspector`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryView {
    pub undo: Option<&'static str>,
    pub redo: Option<&'static str>,
    /// A gesture is open (a drag, a scrub): undo and redo wait for it.
    pub gesture: bool,
}

/// The floor switcher (07 §5.6).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FloorsView {
    pub floors: Vec<FloorView>,
    /// The floor shown; `null` for the implicit floor of a chart that
    /// lists none.
    pub shown: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FloorView {
    pub id: u32,
    pub name: String,
    pub level: i32,
    pub focal_point: Option<[f64; 2]>,
    /// Top-level nodes on it.
    pub nodes: u32,
}

pub(crate) fn floors(doc: &Document, shown: Option<u32>) -> FloorsView {
    let mut counts = std::collections::BTreeMap::new();
    for &id in doc.children(None) {
        *counts.entry(floor_of(doc, id)).or_insert(0u32) += 1;
    }
    let floors = doc
        .floors()
        .iter()
        .map(|f| FloorView {
            id: f.id,
            name: f.name.clone(),
            level: f.level,
            focal_point: f.focal_point,
            nodes: counts.get(&Some(f.id)).copied().unwrap_or(0),
        })
        .collect();
    FloorsView { floors, shown }
}
