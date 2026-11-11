//! The selection (`docs/07-designer.md` §5.1): a set of items at one level.
//! It is not part of the document and not on the undo stack; it is
//! reconciled with the document after every change, so it never names
//! something that is gone.

use std::collections::BTreeSet;

use cavea_compiler::document::Node;

use crate::document::{Document, each_node};
use crate::index::{P, Quad, Rect, Select, SpatialIndex};

/// What the selected items are: nodes at the top level or in a section, rows
/// of a row block, or seat positions of a row block or row (seat-edit mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Top,
    /// The section's children; the section is top-level.
    Section(u32),
    /// The block's rows, by `rid`.
    Rows(u32),
    /// The node's seat positions, by `oid`, removed ones included.
    Seats(u32),
}

impl Level {
    /// The nodes whose children are this level's items, for node levels.
    fn scope(self) -> Option<Option<u32>> {
        match self {
            Level::Top => Some(None),
            Level::Section(s) => Some(Some(s)),
            Level::Rows(_) | Level::Seats(_) => None,
        }
    }

    /// The node holding this level's rows or seats.
    fn holder(self) -> Option<u32> {
        match self {
            Level::Rows(id) | Level::Seats(id) => Some(id),
            Level::Top | Level::Section(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    level: Level,
    /// Node ids, `rid`s or `oid`s by level; sorted, so every order derived
    /// from the selection is deterministic.
    items: BTreeSet<u32>,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            level: Level::Top,
            items: BTreeSet::new(),
        }
    }
}

impl Selection {
    pub fn level(&self) -> Level {
        self.level
    }

    pub fn items(&self) -> &BTreeSet<u32> {
        &self.items
    }

    pub(crate) fn items_mut(&mut self) -> &mut BTreeSet<u32> {
        &mut self.items
    }

    /// Sets the level and items, keeping only items that exist there; a level
    /// whose holder is gone, or is not a section, row block or row as the
    /// level needs, is the top level.
    pub(crate) fn set(
        &mut self,
        doc: &Document,
        level: Level,
        items: impl IntoIterator<Item = u32>,
    ) {
        self.level = if level_valid(doc, level) {
            level
        } else {
            Level::Top
        };
        self.items = items
            .into_iter()
            .filter(|&i| item_valid(doc, self.level, i))
            .collect();
    }

    /// Drops what the document no longer holds at this level.
    pub(crate) fn reconcile(&mut self, doc: &Document) {
        if !level_valid(doc, self.level) {
            *self = Self::default();
        }
        let level = self.level;
        self.items.retain(|&i| item_valid(doc, level, i));
    }
}

fn level_valid(doc: &Document, level: Level) -> bool {
    match level {
        Level::Top => true,
        Level::Section(s) => {
            matches!(doc.node(s), Some(Node::Section(_))) && doc.parent(s).is_none()
        }
        Level::Rows(b) => matches!(doc.node(b), Some(Node::RowBlock(_))),
        Level::Seats(n) => matches!(doc.node(n), Some(Node::RowBlock(_) | Node::Row(_))),
    }
}

fn item_valid(doc: &Document, level: Level, item: u32) -> bool {
    match level {
        Level::Top => doc.node(item).is_some() && doc.parent(item).is_none(),
        Level::Section(s) => doc.parent(item) == Some(s),
        Level::Rows(b) => match doc.node(b) {
            Some(Node::RowBlock(b)) => b.rows.iter().any(|r| r.rid == item),
            _ => false,
        },
        Level::Seats(n) => {
            let runs = |runs: &[[u32; 2]]| {
                runs.iter()
                    .any(|r| item >= r[0] && u64::from(item) < u64::from(r[0]) + u64::from(r[1]))
            };
            match doc.node(n) {
                Some(Node::RowBlock(b)) => b.rows.iter().any(|r| runs(&r.oids)),
                Some(Node::Row(r)) => runs(&r.oids),
                _ => false,
            }
        }
    }
}

/// The floor a node is drawn on (03 §2.2): its section's, else its own if
/// listed, else the first floor listed (`None` for the implicit floor).
pub(crate) fn floor_of(doc: &Document, id: u32) -> Option<u32> {
    let top = doc.parent(id).unwrap_or(id);
    let own = doc.node(top).and_then(|n| each_node!(n, x => x.floor));
    let listed = |f: u32| doc.floors().iter().any(|l| l.id == f);
    own.filter(|&f| listed(f))
        .or_else(|| doc.floors().first().map(|f| f.id))
}

/// Whether a node can be picked on the canvas: it and its section are
/// neither hidden nor locked, and it is on the floor shown.
pub(crate) fn pickable(doc: &Document, floor: Option<u32>, id: u32) -> bool {
    let free = |id: u32| {
        doc.node(id)
            .is_some_and(|n| each_node!(n, x => !x.hidden && !x.locked))
    };
    free(id) && doc.parent(id).is_none_or(free) && floor_of(doc, id) == floor
}

/// What a canvas query sees: the document, the index, the floor shown and
/// the hit tolerance in chart units.
pub(crate) struct View<'a> {
    pub(crate) doc: &'a Document,
    pub(crate) index: &'a SpatialIndex,
    pub(crate) floor: Option<u32>,
    pub(crate) tol: f64,
}

impl View<'_> {
    /// Whether an item of the level may be picked: a node itself, a row or
    /// seat through the node holding it.
    pub(crate) fn pickable(&self, level: Level, item: u32) -> bool {
        let node = level.holder().unwrap_or(item);
        pickable(self.doc, self.floor, node)
    }

    /// The item of `level` at `point`, if any.
    pub(crate) fn hit(&self, level: Level, point: P) -> Option<u32> {
        if let Some(scope) = level.scope() {
            let pick = |id| self.pickable(level, id);
            return self.index.hit(self.doc, scope, point, self.tol, pick);
        }
        let holder = level.holder()?;
        if !self.pickable(level, holder) {
            return None;
        }
        let ix = self.index;
        match level {
            Level::Rows(b) => {
                let seat = ix.seat_at(b, point, ix.reach(b) + self.tol)?;
                rid_of(self.doc, b, seat.row)
            }
            _ => ix
                .seat_at(holder, point, ix.seat_pad(holder) + self.tol)
                .map(|s| s.oid),
        }
    }

    /// The items of `level` that `rect` contains or touches. A row is
    /// contained when all its present seats are; a seat when its box is.
    pub(crate) fn select(&self, level: Level, rect: Rect, mode: Select) -> Vec<u32> {
        if let Some(scope) = level.scope() {
            let pick = |id| self.pickable(level, id);
            return self.index.select(self.doc, rect, scope, mode, pick);
        }
        let Some(holder) = level.holder().filter(|&h| self.pickable(level, h)) else {
            return Vec::new();
        };
        let ix = self.index;
        let pad = ix.seat_pad(holder);
        let inside = |pos: P| {
            let Some(w) = ix.to_world(holder, pos) else {
                return false;
            };
            let b = Rect::new(w, w).grow(pad);
            match mode {
                Select::Contain => rect.contains(&b),
                Select::Touch => rect.intersects(&b),
            }
        };
        let seats = ix.seats(holder);
        if let Level::Seats(_) = level {
            return seats
                .iter()
                .filter(|s| inside(s.pos))
                .map(|s| s.oid)
                .collect();
        }
        let mut out = Vec::new();
        for row in seats.chunk_by(|a, b| a.row == b.row) {
            let mut present = row.iter().filter(|s| !s.removed).peekable();
            let hit = present.peek().is_some()
                && match mode {
                    Select::Contain => present.all(|s| inside(s.pos)),
                    Select::Touch => present.any(|s| inside(s.pos)),
                };
            if hit && let Some(rid) = row.first().and_then(|s| rid_of(self.doc, holder, s.row)) {
                out.push(rid);
            }
        }
        out
    }

    /// Every pickable item of the level.
    pub(crate) fn all(&self, level: Level) -> Vec<u32> {
        if let Some(scope) = level.scope() {
            let kids = self.doc.children(scope).iter().copied();
            return kids.filter(|&id| self.pickable(level, id)).collect();
        }
        let Some(holder) = level.holder().filter(|&h| self.pickable(level, h)) else {
            return Vec::new();
        };
        match (level, self.doc.node(holder)) {
            (Level::Rows(_), Some(Node::RowBlock(b))) => b.rows.iter().map(|r| r.rid).collect(),
            _ => self.index.seats(holder).iter().map(|s| s.oid).collect(),
        }
    }

    /// The outline of each selected item, following its node's rotation: a
    /// node's box, a row's box around its seats, a seat's box.
    pub(crate) fn outlines(&self, selection: &Selection) -> Vec<Quad> {
        let level = selection.level;
        let items = selection.items.iter().copied();
        let Some(holder) = level.holder() else {
            return items.filter_map(|id| self.index.quad(id)).collect();
        };
        let ix = self.index;
        let pad = ix.seat_pad(holder);
        let quad = |r: Rect| {
            r.grow(pad)
                .corners()
                .map(|p| ix.to_world(holder, p).unwrap_or(p))
        };
        let seats = ix.seats(holder);
        match level {
            Level::Rows(_) => {
                let rows = seats.chunk_by(|a, b| a.row == b.row);
                rows.filter(|row| {
                    row.first()
                        .and_then(|s| rid_of(self.doc, holder, s.row))
                        .is_some_and(|rid| selection.items.contains(&rid))
                })
                .filter_map(|row| Rect::around(row.iter().filter(|s| !s.removed).map(|s| s.pos)))
                .map(quad)
                .collect()
            }
            _ => seats
                .iter()
                .filter(|s| selection.items.contains(&s.oid))
                .map(|s| quad(Rect::new(s.pos, s.pos)))
                .collect(),
        }
    }
}

fn rid_of(doc: &Document, block: u32, row: u32) -> Option<u32> {
    match doc.node(block) {
        Some(Node::RowBlock(b)) => b.rows.get(row as usize).map(|r| r.rid),
        _ => None,
    }
}

/// Steps into the item `id` of the selection's level, if it holds a level of
/// its own: a section's children, a row block's rows, a row block's or row's
/// seats. The item at `point`, if one is given and there is one, is
/// selected at the new level; otherwise nothing. Returns whether it stepped.
pub(crate) fn enter(view: &View<'_>, selection: &mut Selection, id: u32, point: Option<P>) -> bool {
    let doc = view.doc;
    let next = match (selection.level, doc.node(id)) {
        (Level::Top, Some(Node::Section(_))) => Level::Section(id),
        (Level::Top | Level::Section(_), Some(Node::RowBlock(_))) => Level::Rows(id),
        (Level::Top | Level::Section(_), Some(Node::Row(_))) => Level::Seats(id),
        (Level::Rows(b), _) => Level::Seats(b),
        _ => return false,
    };
    let picked = point.and_then(|p| view.hit(next, p));
    selection.set(doc, next, picked);
    true
}

/// Steps out one level, selecting what held the items: from seats to the
/// rows holding the selected seats (or the row node), from rows to the
/// block, from a section to the section. At the top level it clears.
pub(crate) fn exit(doc: &Document, index: &SpatialIndex, selection: &mut Selection) {
    let level = selection.level;
    let parent = |id: u32| match doc.parent(id) {
        Some(s) => Level::Section(s),
        None => Level::Top,
    };
    match level {
        Level::Top => selection.items.clear(),
        Level::Section(s) => selection.set(doc, Level::Top, [s]),
        Level::Rows(b) => selection.set(doc, parent(b), [b]),
        Level::Seats(n) => match doc.node(n) {
            Some(Node::RowBlock(_)) => {
                let rows: BTreeSet<u32> = index
                    .seats(n)
                    .iter()
                    .filter(|s| selection.items.contains(&s.oid))
                    .filter_map(|s| rid_of(doc, n, s.row))
                    .collect();
                selection.set(doc, Level::Rows(n), rows);
            }
            _ => selection.set(doc, parent(n), [n]),
        },
    }
}
