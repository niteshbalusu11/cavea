//! Names for new objects (07 §5.1): what a person would call them, "Row
//! block 3", so the layers panel and the validation panel name them in
//! words rather than by id. Like a new section's label (§5.5), the number
//! is the smallest that no object of the same kind's name uses. A node that
//! has a name, or a label or text the layers panel shows, keeps it.

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
}
