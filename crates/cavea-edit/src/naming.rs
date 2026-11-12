//! Names for new objects (07 §5.1): what a person would call them, "Row
//! block 3", so the layers panel and the validation panel name them in
//! words rather than by id. Like a new section's label (§5.5), the number
//! is the smallest that no object of the same kind's name uses. A node that
//! has a name, or a label or text the layers panel shows, keeps it.

use std::collections::{BTreeMap, BTreeSet};

use cavea_compiler::document::{Node, ShapeKind};

use crate::document::{Document, each_node};

/// The word for a node's kind, or `None` for a kind the layers panel names
/// by its own label or text (sections, texts, tables, booths, areas).
fn word(node: &Node) -> Option<&'static str> {
    Some(match node {
        Node::RowBlock(_) => "Row block",
        Node::Row(r) if r.label.is_none() => "Row",
        Node::Shape(s) => match s.kind {
            ShapeKind::Rect => "Rectangle",
            ShapeKind::Ellipse => "Ellipse",
            ShapeKind::Polygon => "Polygon",
            ShapeKind::Line | ShapeKind::Path => "Line",
        },
        Node::Icon(_) => "Icon",
        Node::Image(_) => "Image",
        _ => return None,
    })
}

/// The name a new `node` gets in `doc`, if it has none and its kind takes
/// one: its kind's word and the smallest number no other node's name uses
/// with that word.
pub(crate) fn fresh(doc: &Document, node: &Node) -> Option<String> {
    if each_node!(node, n => n.name.is_some()) {
        return None;
    }
    let word = word(node)?;
    let own = node_id(node);
    let mut used: Vec<u32> = doc
        .ids()
        .filter(|&id| Some(id) != own)
        .filter_map(|id| doc.node(id))
        .filter_map(|n| each_node!(n, x => x.name.as_deref()))
        .filter_map(|name| name.strip_prefix(word)?.strip_prefix(' ')?.parse().ok())
        .collect();
    used.sort_unstable();
    used.dedup();
    let mut n = 1;
    for u in used {
        if u == n {
            n += 1;
        } else if u > n {
            break;
        }
    }
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

/// Every name the nodes of `doc` have.
pub(crate) fn names(doc: &Document) -> Names {
    Names {
        taken: doc
            .ids()
            .filter_map(|id| doc.node(id))
            .filter_map(|n| each_node!(n, x => x.name.clone()))
            .collect(),
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
    use super::*;

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
    }
}
