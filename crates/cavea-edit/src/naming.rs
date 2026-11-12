//! Names of objects (07 §5.1): what a person would call them, "Row block
//! 3", so that no view ever shows an object by its internal id.
//!
//! A node's name on display ([`shown`]) is its own: its `name`, else its
//! label, else a text's string. A node with none (one made by `cavea gen`,
//! imported, made through the API, or saved before new objects were named)
//! is shown by its kind's word and a number, worked out here and never
//! written into the document: opening a chart changes nothing. The numbers
//! go in document order to the smallest that no other node's name or shown
//! name uses with that word, so a chart opened twice shows the same names;
//! while it is edited, a node keeps the number it was shown by until a name
//! some node takes uses it, so deleting one unnamed node renumbers no other.
//!
//! A new object gets a stored name the same way ([`fresh`]): its kind's
//! word and the smallest number no other node's name or shown name uses.

use std::collections::{BTreeMap, BTreeSet};

use cavea_compiler::document::{Node, ShapeKind};

use crate::document::{Document, each_node};

/// Every word [`shown_word`] gives, longest first where one starts another
/// ("Row block" before "Row").
const WORDS: [&str; 13] = [
    "Row block",
    "Row",
    "Rectangle",
    "Ellipse",
    "Polygon",
    "Line",
    "Icon",
    "Image",
    "Section",
    "Table",
    "Booth",
    "Area",
    "Text",
];

/// The word a node of this kind is shown by when it has no name of its own.
fn shown_word(node: &Node) -> &'static str {
    match node {
        Node::Section(_) => "Section",
        Node::RowBlock(_) => "Row block",
        Node::Row(_) => "Row",
        Node::Table(_) => "Table",
        Node::Booth(_) => "Booth",
        Node::Area(_) => "Area",
        Node::Text(_) => "Text",
        Node::Shape(s) => match s.kind {
            ShapeKind::Rect => "Rectangle",
            ShapeKind::Ellipse => "Ellipse",
            ShapeKind::Polygon => "Polygon",
            ShapeKind::Line | ShapeKind::Path => "Line",
        },
        Node::Icon(_) => "Icon",
        Node::Image(_) => "Image",
    }
}

/// The word a new node's stored name takes, or `None` for a kind named by
/// its own label or text (sections, texts, tables, booths, areas, a
/// labelled row).
fn word(node: &Node) -> Option<&'static str> {
    match node {
        Node::RowBlock(_) | Node::Shape(_) | Node::Icon(_) | Node::Image(_) => {
            Some(shown_word(node))
        }
        Node::Row(r) if r.label.is_none() => Some(shown_word(node)),
        _ => None,
    }
}

/// The word and number of a name made like these ("Row block 3").
fn parse(name: &str) -> Option<(&'static str, u32)> {
    WORDS.iter().find_map(|&w| {
        let n = name.strip_prefix(w)?.strip_prefix(' ')?;
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some((w, n.parse().ok()?))
    })
}

/// The node's own name: its `name`, else its label, else a text's string;
/// `None` when that is empty or blank.
pub(crate) fn own(node: &Node) -> Option<String> {
    let label = match node {
        Node::Section(n) => n.label.as_deref(),
        Node::Row(n) => n.label.as_deref(),
        Node::Table(n) => n.label.as_deref(),
        Node::Booth(n) => n.label.as_deref(),
        Node::Area(n) => n.label.as_deref(),
        Node::Text(n) => Some(n.text.as_str()),
        Node::RowBlock(_) | Node::Shape(_) | Node::Icon(_) | Node::Image(_) => None,
    };
    let name = each_node!(node, n => n.name.as_deref()).filter(|s| !s.trim().is_empty());
    name.or(label.filter(|s| !s.trim().is_empty()))
        .map(str::to_owned)
}

/// The names unnamed nodes are shown by, for one structure revision of a
/// document (kept in [`Document`], worked out again from the last when the
/// structure changes).
#[derive(Debug, Default)]
pub(crate) struct Shown {
    revision: Option<u64>,
    /// Each unnamed node's word and number.
    names: BTreeMap<u32, (&'static str, u32)>,
    /// For each word, the numbers in use: by nodes' own names and by the
    /// names unnamed nodes are shown by.
    taken: BTreeMap<&'static str, BTreeSet<u32>>,
}

impl Shown {
    /// Brought up to date with `doc`'s structure.
    fn update(&mut self, doc: &Document) {
        let revision = doc.structure_revision();
        if self.revision == Some(revision) {
            return;
        }
        let mut taken: BTreeMap<&'static str, BTreeSet<u32>> = BTreeMap::new();
        let mut unnamed = Vec::new();
        // Document order: each top-level node, then its section's children.
        let mut stack: Vec<u32> = doc.children(None).iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            let Some(node) = doc.node(id) else { continue };
            match own(node) {
                Some(name) => {
                    if let Some((w, n)) = parse(&name) {
                        taken.entry(w).or_default().insert(n);
                    }
                }
                None => unnamed.push((id, shown_word(node))),
            }
            stack.extend(doc.children(Some(id)).iter().rev());
        }
        // A node keeps the number it was shown by while no name uses it.
        let mut names = BTreeMap::new();
        let mut rest = Vec::new();
        for (id, w) in unnamed {
            let kept = match self.names.get(&id) {
                Some(&(was, n)) if was == w => taken.entry(w).or_default().insert(n).then_some(n),
                _ => None,
            };
            match kept {
                Some(n) => {
                    names.insert(id, (w, n));
                }
                None => rest.push((id, w)),
            }
        }
        // The others, in document order, take the smallest numbers free.
        let mut next: BTreeMap<&'static str, u32> = BTreeMap::new();
        for (id, w) in rest {
            let used = taken.entry(w).or_default();
            let mut n = next.get(w).copied().unwrap_or(1);
            while used.contains(&n) {
                n = n.saturating_add(1);
            }
            used.insert(n);
            next.insert(w, n.saturating_add(1));
            names.insert(id, (w, n));
        }
        *self = Self {
            revision: Some(revision),
            names,
            taken,
        };
    }
}

/// Runs `f` on the names of `doc`'s unnamed nodes, up to date; `None` only
/// if they are being worked out already (never: nothing here calls back).
fn with_shown<T>(doc: &Document, f: impl FnOnce(&Shown) -> T) -> Option<T> {
    let mut shown = doc.shown.try_borrow_mut().ok()?;
    shown.update(doc);
    Some(f(&shown))
}

/// The name node `id` is shown by in every view (the layers panel, the
/// inspector, the findings, the status bar): its own, else its kind's word
/// and a number (see the module's comment). `None` for an unknown id.
pub(crate) fn shown(doc: &Document, id: u32) -> Option<String> {
    let node = doc.node(id)?;
    if let Some(name) = own(node) {
        return Some(name);
    }
    let word = shown_word(node);
    let n = with_shown(doc, |s| s.names.get(&id).map(|&(_, n)| n)).flatten();
    Some(n.map_or_else(|| word.to_owned(), |n| format!("{word} {n}")))
}

/// The name a new `node` gets in `doc`, if it has none and its kind takes
/// one: its kind's word and the smallest number no other node's name, or
/// name on display, uses with that word. A node drawn is in `doc` already,
/// unnamed: it keeps the name it was shown by while it was drawn.
pub(crate) fn fresh(doc: &Document, node: &Node) -> Option<String> {
    if each_node!(node, n => n.name.is_some()) {
        return None;
    }
    let word = word(node)?;
    let own = node_id(node);
    let (shown, used) = with_shown(doc, |s| {
        let shown = own.and_then(|id| s.names.get(&id)).copied();
        (shown, s.taken.get(word).cloned().unwrap_or_default())
    })
    .unwrap_or_default();
    let n = match shown {
        Some((w, n)) if w == word => n,
        _ => (1..=u32::MAX).find(|n| !used.contains(n))?,
    };
    Some(format!("{word} {n}"))
}

/// The names copies are named against in one paste or duplicate: every
/// node's name, the copies' as they are named, and for each kind's word
/// the number below which every "<word> N" is taken (nothing is freed
/// during one operation), so each copy's search starts there, not at 1.
#[derive(Debug, Default)]
pub(crate) struct Names {
    taken: BTreeSet<String>,
    next: BTreeMap<&'static str, u32>,
}

/// Every name the nodes of `doc` have or are shown by (stored, label,
/// text, or kind and number): a copy takes none of them.
pub(crate) fn names(doc: &Document) -> Names {
    let mut taken: BTreeSet<String> = doc
        .ids()
        .filter_map(|id| doc.node(id))
        .flat_map(|n| [each_node!(n, x => x.name.clone()), own(n)])
        .flatten()
        .collect();
    with_shown(doc, |s| {
        taken.extend(s.names.values().map(|(w, n)| format!("{w} {n}")));
    });
    Names {
        taken,
        next: BTreeMap::new(),
    }
}

/// Names a pasted or duplicated `node` and its section's children: one
/// whose name is an automatic one of its kind ("Row block 2") already
/// taken (the document's names, and earlier copies') gets the smallest
/// free number, so a copy never shares its original's name. Any other name
/// is a person's, and kept.
pub(crate) fn rename_copy(node: &mut Node, names: &mut Names) {
    if let Some(word) = word(node)
        && let Some(name) = each_node!(&*node, n => n.name.clone())
        && name
            .strip_prefix(word)
            .and_then(|r| r.strip_prefix(' '))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        && names.taken.contains(&name)
    {
        let from = names.next.get(word).copied().unwrap_or(1);
        let found = (from..=u32::MAX).find(|n| !names.taken.contains(&format!("{word} {n}")));
        if let Some(n) = found {
            names.next.insert(word, n.saturating_add(1));
            each_node!(node, x => x.name = Some(format!("{word} {n}")));
        }
    }
    if let Some(name) = each_node!(&*node, n => n.name.clone()) {
        names.taken.insert(name);
    }
    if let Node::Section(sec) = node {
        for child in &mut sec.children {
            rename_copy(child, names);
        }
    }
}

/// A node's id; 0 (not yet given) counts as none.
fn node_id(node: &Node) -> Option<u32> {
    Some(each_node!(node, n => n.id)).filter(|&id| id != 0)
}

#[cfg(test)]
mod tests {
    use cavea_compiler::ChartDocument;

    use super::*;
    use crate::command::{Command, Property};
    use crate::editor::Editor;

    #[test]
    fn smallest_free_number_of_the_kind() {
        let doc = Document::load(
            serde_json::from_value(serde_json::json!({
                "schema": 1, "name": "n", "nextOid": 100,
                "categories": [{ "key": "std", "label": "Standard", "color": "#2563EB" }],
                "nodes": [
                    { "id": 1, "type": "rowBlock", "name": "Row block 1", "rows": [] },
                    { "id": 2, "type": "rowBlock", "name": "Row block 3", "rows": [] },
                    { "id": 3, "type": "shape", "kind": "rect", "name": "Row block 2", "size": [1, 1] },
                    { "id": 4, "type": "rowBlock", "name": "Balcony", "rows": [] },
                    { "id": 5, "type": "shape", "kind": "rect", "size": [1, 1] }
                ]
            }))
            .expect("chart"),
        )
        .expect("load");
        let named = |id: u32| doc.node(id).and_then(|n| fresh(&doc, n));
        // Another kind's "Row block 2" counts as used: names stay unique.
        assert_eq!(named(5).as_deref(), Some("Rectangle 1"));
        assert_eq!(named(1), None, "a named node keeps its name");
        let mut block = doc.node(4).cloned().expect("block");
        if let Node::RowBlock(b) = &mut block {
            (b.id, b.name) = (0, None);
        }
        assert_eq!(fresh(&doc, &block).as_deref(), Some("Row block 4"));
    }

    #[test]
    fn copies_get_their_own_automatic_names() {
        let doc = Document::load(
            serde_json::from_value(serde_json::json!({
                "schema": 1, "name": "n", "nextOid": 100,
                "nodes": [
                    { "id": 1, "type": "rowBlock", "name": "Row block 1", "rows": [] },
                    { "id": 2, "type": "rowBlock", "name": "Balcony", "rows": [] }
                ]
            }))
            .expect("chart"),
        )
        .expect("load");
        let copy = |id: u32| doc.node(id).cloned().expect("node");
        let mut taken = names(&doc);
        let (mut a, mut b, mut c) = (copy(1), copy(1), copy(2));
        for n in [&mut a, &mut b, &mut c] {
            rename_copy(n, &mut taken);
        }
        // Thousands of copies: each search starts where the last ended.
        let mut many = names(&doc);
        for _ in 0..5_000 {
            rename_copy(&mut copy(1), &mut many);
        }
        assert!(many.taken.contains("Row block 5001"));
        assert_eq!(many.next.get("Row block"), Some(&5_002));
        let name = |n: &Node| each_node!(n, x => x.name.clone());
        // Two copies of "Row block 1": 2, then 3; a person's name is kept.
        assert_eq!(name(&a).as_deref(), Some("Row block 2"));
        assert_eq!(name(&b).as_deref(), Some("Row block 3"));
        assert_eq!(name(&c).as_deref(), Some("Balcony"));

        // A name shown from a label or text is taken too.
        let doc = Document::load(
            serde_json::from_value(serde_json::json!({
                "schema": 1, "name": "n", "nextOid": 100,
                "nodes": [
                    { "id": 1, "type": "shape", "kind": "rect", "name": "Rectangle 1", "size": [1, 1] },
                    { "id": 2, "type": "text", "text": "Rectangle 2", "size": 10, "color": "#000000" }
                ]
            }))
            .expect("chart"),
        )
        .expect("load");
        let mut rect = doc.node(1).cloned().expect("rect");
        rename_copy(&mut rect, &mut names(&doc));
        assert_eq!(name(&rect).as_deref(), Some("Rectangle 3"));
    }

    /// Unnamed nodes (a chart made by `cavea gen`, imported, made through
    /// the API) are shown by kind and number in document order, around the
    /// names nodes have, without the document changing; deleting one
    /// renumbers no other, and a new object's name takes none of theirs.
    #[test]
    fn unnamed_nodes_are_shown_by_kind_and_number() {
        let chart: ChartDocument = serde_json::from_value(serde_json::json!({
            "schema": 1, "name": "n", "nextOid": 100,
            "nodes": [
                { "id": 9, "type": "rowBlock", "rows": [] },
                { "id": 3, "type": "rowBlock", "name": "Row block 2", "rows": [] },
                { "id": 20, "type": "section", "children": [
                    { "id": 7, "type": "rowBlock", "rows": [] },
                    { "id": 8, "type": "shape", "kind": "rect", "size": [1, 1] }
                ] },
                { "id": 5, "type": "rowBlock", "name": " ", "rows": [] },
                { "id": 6, "type": "text", "text": "", "size": 10, "color": "#000000" },
                { "id": 4, "type": "section", "label": "Section 1", "children": [] }
            ]
        }))
        .expect("chart");
        let mut editor = Editor::load(chart.clone()).expect("load");
        let shown = |e: &Editor, id| shown(e.document(), id).unwrap_or_default();
        let all = |e: &Editor| [9, 3, 20, 7, 8, 5, 6, 4].map(|id| shown(e, id));
        assert_eq!(
            all(&editor),
            [
                "Row block 1",
                "Row block 2",
                "Section 2",
                "Row block 3",
                "Rectangle 1",
                "Row block 4",
                "Text 1",
                "Section 1",
            ]
        );
        assert_eq!(editor.document().snapshot(), chart, "nothing is written");
        assert_eq!((editor.document().revision(), editor.undo_len()), (0, 0));

        editor
            .apply(Command::DeleteNodes { ids: vec![9] })
            .expect("delete");
        assert_eq!(shown(&editor, 7), "Row block 3", "kept after a delete");
        let block = editor.document().node(7).cloned().expect("block");
        let mut new = block.clone();
        each_node!(&mut new, n => n.id = 0);
        assert_eq!(
            fresh(editor.document(), &new).as_deref(),
            Some("Row block 1")
        );
        // A drawn node, in the document unnamed, keeps the name it showed.
        assert_eq!(
            fresh(editor.document(), &block).as_deref(),
            Some("Row block 3")
        );
        editor.undo().expect("undo");
        assert_eq!(shown(&editor, 9), "Row block 1");

        // A person's name taking a shown one moves only that node on.
        editor
            .apply(Command::SetProperty {
                id: 9,
                property: Property::Name(Some("Row block 3".into())),
            })
            .expect("rename");
        assert_eq!(shown(&editor, 7), "Row block 1");
        assert_eq!(shown(&editor, 5), "Row block 4");
    }
}
