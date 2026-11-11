//! Sections (`docs/07-designer.md` §5.5): grouping top-level nodes into a new
//! section, dissolving one, and its automatic outline as the compiler
//! computes it. Nodes keep their place on the canvas because `placeNode`
//! rewrites their transforms (§2.2); each of these is one undo entry.

use cavea_compiler::document::{Node, Outline, Points, Section};
use cavea_compiler::{DEFAULT_PADDING, auto_outline};
use cavea_core::limits::MAX_SECTIONS;

use crate::command::{Command, CommandError};
use crate::document::{Document, each_node};
use crate::draft::{MAX_POINTS, simple};
use crate::editor::Editor;
use crate::index::{P, Place, Rect, SpatialIndex};

/// Points of all explicit outlines in a chart (02 §14).
const MAX_OUTLINE_POINTS: usize = 262_144;

/// Applies `commands`, built from the ids the first one creates, in one
/// gesture: one undo entry, or nothing on error.
fn in_gesture(
    editor: &mut Editor,
    run: impl FnOnce(&mut Editor) -> Result<u32, CommandError>,
) -> Result<u32, CommandError> {
    editor.begin_gesture()?;
    match run(editor) {
        Ok(id) => editor.end_gesture().map(|()| id),
        Err(e) => {
            let _ = editor.cancel_gesture();
            Err(e)
        }
    }
}

/// "Section N" for the smallest N no section's label already uses.
fn label(doc: &Document) -> String {
    let taken: std::collections::BTreeSet<&str> = doc
        .children(None)
        .iter()
        .filter_map(|&id| match doc.node(id) {
            Some(Node::Section(s)) => s.label.as_deref(),
            _ => None,
        })
        .collect();
    (1..)
        .map(|n| format!("Section {n}"))
        .find(|l| !taken.contains(l.as_str()))
        .unwrap_or_default()
}

/// Creates a top-level section holding `ids` (top-level nodes, not
/// sections), which keep their place on the canvas. With `outline` (world
/// points, a simple polygon) it is explicit; otherwise automatic. The
/// section's origin is the centre of the outline's box, or of the nodes'
/// bounds, its rotation 0; it goes where the lowest of `ids` was, or at the
/// end. Returns its id.
pub(crate) fn group(
    editor: &mut Editor,
    index: &SpatialIndex,
    ids: &[u32],
    outline: Option<Vec<P>>,
    (category, floor): (Option<&str>, Option<u32>),
) -> Result<u32, CommandError> {
    let doc = editor.document();
    let top = doc.children(None);
    let mut taken: Vec<(usize, u32)> = Vec::with_capacity(ids.len());
    for &id in ids {
        let at = top.iter().position(|&t| t == id);
        match (doc.node(id), at) {
            (None, _) => return Err(CommandError::MissingNode(id)),
            (Some(Node::Section(_)), _) | (_, None) => return Err(CommandError::Placement),
            (Some(_), Some(at)) => taken.push((at, id)),
        }
    }
    taken.sort_unstable();
    taken.dedup();
    let sections = top
        .iter()
        .filter(|&&id| matches!(doc.node(id), Some(Node::Section(_))));
    if sections.clone().count() >= MAX_SECTIONS as usize {
        return Err(CommandError::Limit("sections"));
    }
    let (origin, outline) = match outline {
        Some(points) => {
            let used: usize = sections
                .filter_map(|&id| match doc.node(id) {
                    Some(Node::Section(s)) => match &s.outline {
                        Outline::Points(p) => Some(p.points.len()),
                        Outline::Auto => None,
                    },
                    _ => None,
                })
                .sum();
            if points.len() > MAX_POINTS || used + points.len() > MAX_OUTLINE_POINTS {
                return Err(CommandError::Limit("outline points"));
            }
            let finite = points.iter().flatten().all(|v| v.is_finite());
            if !finite || !simple(&points) {
                return Err(CommandError::InvalidValue("outline"));
            }
            let c = Rect::around(points.iter().copied()).map_or([0.0; 2], |r| centre(&r));
            let local = points.iter().map(|p| [p[0] - c[0], p[1] - c[1]]);
            let outline = Outline::Points(Points {
                points: local.collect(),
            });
            (c, outline)
        }
        None => {
            let bounds = taken
                .iter()
                .fold(None, |a, &(_, id)| Rect::union(a, index.bounds(id)));
            (bounds.map_or([0.0; 2], |r| centre(&r)), Outline::Auto)
        }
    };
    let node = Node::Section(Section {
        id: 0,
        name: None,
        floor,
        origin,
        rotation: 0.0,
        locked: false,
        hidden: false,
        label: Some(label(doc)),
        outline,
        category: category.map(String::from),
        priority: 1.0,
        entrance: None,
        children: Vec::new(),
    });
    let at = taken.first().map_or(top.len(), |t| t.0);
    in_gesture(editor, |editor| {
        let created = editor.apply(Command::CreateNode {
            parent: None,
            index: u32::try_from(at).map_err(|_| CommandError::IndexOutOfRange)?,
            node: Box::new(node),
        })?;
        let section = created.created.first().copied();
        let section = section.ok_or(CommandError::Internal)?;
        for (k, &(_, id)) in (0u32..).zip(&taken) {
            editor.apply(Command::PlaceNode {
                id,
                parent: Some(section),
                index: k,
            })?;
        }
        Ok(section)
    })
}

/// Moves a top-level section's children to the top level where the section
/// was, in order and in place on the canvas, and deletes the section.
pub(crate) fn dissolve(editor: &mut Editor, id: u32) -> Result<(), CommandError> {
    let doc = editor.document();
    if !matches!(doc.node(id), Some(Node::Section(_))) {
        return Err(CommandError::NotASection(id));
    }
    let at = doc.children(None).iter().position(|&t| t == id);
    let at = at.ok_or(CommandError::Placement)?;
    let at = u32::try_from(at).map_err(|_| CommandError::IndexOutOfRange)?;
    let children = doc.children(Some(id)).to_vec();
    in_gesture(editor, |editor| {
        for (k, c) in (at..).zip(children) {
            editor.apply(Command::PlaceNode {
                id: c,
                parent: None,
                index: k,
            })?;
        }
        editor.apply(Command::DeleteNodes { ids: vec![id] })?;
        Ok(id)
    })
    .map(drop)
}

/// The automatic outline of section `id` in its own space, from its visible
/// row blocks' and rows' present seats as the index caches them, with the
/// compiler's padding and algorithm (03 §2.3). Empty without seats; `None`
/// for a node that is not a section or a grid the compiler would refuse.
pub(crate) fn outline(doc: &Document, index: &SpatialIndex, id: u32) -> Option<Vec<P>> {
    let Node::Section(_) = doc.node(id)? else {
        return None;
    };
    let (mut seats, mut pitch) = (Vec::new(), None::<f64>);
    for &c in doc.children(Some(id)) {
        let node = doc.node(c)?;
        match node {
            _ if each_node!(node, n => n.hidden) => continue,
            Node::RowBlock(b) => pitch = Some(pitch.map_or(b.row_pitch, |p| p.max(b.row_pitch))),
            Node::Row(_) => {}
            _ => continue,
        }
        let place = each_node!(node, n => Place::of(n.origin, n.rotation));
        let present = index.seats(c).iter().filter(|s| !s.removed);
        seats.extend(present.map(|s| place.apply(s.pos)));
    }
    auto_outline(&seats, pitch.map_or(DEFAULT_PADDING, |p| p / 2.0))
}

fn centre(r: &Rect) -> P {
    [(r.min[0] + r.max[0]) / 2.0, (r.min[1] + r.max[1]) / 2.0]
}
