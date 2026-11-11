//! The spatial index behind selection and snapping (`docs/07-designer.md`
//! §5.2): each node's bounds in world space, and each seat node's expanded
//! seats in its own space, cached and brought up to date from the document's
//! change tracking by [`SpatialIndex::sync`].
//!
//! A chart has a few hundred to a few thousand nodes (row blocks are
//! parametric), so queries scan the cached bounds of the nodes at one level in
//! document order: no tree or grid is needed to stay far inside a frame.
//! Bounds are axis-aligned boxes; a row block's come from its expanded seats.

use std::collections::BTreeMap;

use cavea_compiler::Expansion;
use cavea_compiler::document::{
    Align, AlphaOverflow, Node, Outline, RowLabels, SeatLabels, Settings, ShapeKind, TableShape,
};

use crate::document::{Document, each_node};

type P = [f64; 2];

/// An axis-aligned box in chart units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub min: P,
    pub max: P,
}

impl Rect {
    /// The box spanning two corners in any order.
    pub fn new(a: P, b: P) -> Self {
        Self {
            min: [a[0].min(b[0]), a[1].min(b[1])],
            max: [a[0].max(b[0]), a[1].max(b[1])],
        }
    }

    pub fn contains_point(&self, p: P) -> bool {
        (self.min[0]..=self.max[0]).contains(&p[0]) && (self.min[1]..=self.max[1]).contains(&p[1])
    }

    pub fn contains(&self, r: &Rect) -> bool {
        self.contains_point(r.min) && self.contains_point(r.max)
    }

    pub fn intersects(&self, r: &Rect) -> bool {
        self.min[0] <= r.max[0]
            && r.min[0] <= self.max[0]
            && self.min[1] <= r.max[1]
            && r.min[1] <= self.max[1]
    }

    fn union(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
        match (a, b) {
            (Some(a), Some(b)) => Some(Rect::new(
                [a.min[0].min(b.min[0]), a.min[1].min(b.min[1])],
                [a.max[0].max(b.max[0]), a.max[1].max(b.max[1])],
            )),
            (a, b) => a.or(b),
        }
    }

    fn grow(self, pad: f64) -> Rect {
        Rect {
            min: [self.min[0] - pad, self.min[1] - pad],
            max: [self.max[0] + pad, self.max[1] + pad],
        }
    }
}

/// Box selection: what the box fully contains, or (Alt) what it touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Select {
    Contain,
    Touch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorKind {
    /// A corner of the node's bounds.
    Corner,
    /// The centre of its bounds.
    Centre,
    /// The first or last seat of a row.
    RowEnd,
}

/// A point a snapping tool may snap to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    pub node: u32,
    pub kind: AnchorKind,
    pub point: P,
}

/// One expanded seat position, in its node's own space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CachedSeat {
    pub pos: P,
    pub oid: u32,
    /// Index of its row in the node.
    pub row: u32,
    pub removed: bool,
}

/// A rigid transform: rotation, then translation.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Place {
    cos: f64,
    sin: f64,
    t: P,
}

impl Place {
    const IDENTITY: Self = Self {
        cos: 1.0,
        sin: 0.0,
        t: [0.0, 0.0],
    };

    fn of(origin: P, rotation: f64) -> Self {
        let (sin, cos) = (rotation % 360.0).to_radians().sin_cos();
        Self {
            cos,
            sin,
            t: origin,
        }
    }

    /// With y down, turns +x towards +y: clockwise on screen, as the compiler.
    fn apply(&self, [x, y]: P) -> P {
        [
            x * self.cos - y * self.sin + self.t[0],
            x * self.sin + y * self.cos + self.t[1],
        ]
    }

    /// `self` after `inner`: the parent's place applied to a child's.
    fn compose(&self, inner: &Place) -> Place {
        Place {
            cos: self.cos * inner.cos - self.sin * inner.sin,
            sin: self.sin * inner.cos + self.cos * inner.sin,
            t: self.apply(inner.t),
        }
    }

    fn invert(&self, p: P) -> P {
        let (dx, dy) = (p[0] - self.t[0], p[1] - self.t[1]);
        [
            dx * self.cos + dy * self.sin,
            -dx * self.sin + dy * self.cos,
        ]
    }
}

#[derive(Debug)]
struct Item {
    world: Place,
    bounds: Option<Rect>,
    /// The node's content stamp when `seats` and `points` were made.
    content: u64,
    seats: Vec<CachedSeat>,
    /// Outline points in the node's own space, for nodes without seats.
    points: Vec<P>,
    pad: f64,
}

#[derive(Debug, Default)]
pub struct SpatialIndex {
    revision: u64,
    synced: bool,
    items: BTreeMap<u32, Item>,
}

impl SpatialIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Brings the index up to date with `doc`: only nodes changed since the
    /// last sync are recomputed, and only those whose content changed are
    /// expanded again. A chart-level change (seat size) recomputes all.
    pub fn sync(&mut self, doc: &Document) {
        let changes = doc.changes_since(self.revision);
        for id in &changes.removed {
            self.items.remove(id);
        }
        let full = !self.synced || changes.chart;
        if full {
            self.items.clear();
        }
        let dirty: Vec<u32> = if full {
            doc.children(None).to_vec()
        } else {
            changes.nodes
        };
        let is_dirty = |id| dirty.binary_search(&id).is_ok();
        for &id in &dirty {
            // A node below a dirty one is refreshed with it.
            let mut up = doc.parent(id);
            let mut covered = false;
            while let Some(a) = up {
                covered |= is_dirty(a);
                up = doc.parent(a);
            }
            if full || !covered {
                let parent = doc.parent(id).and_then(|p| self.items.get(&p));
                let world = parent.map_or(Place::IDENTITY, |p| p.world);
                self.refresh(doc, id, world);
                let mut up = doc.parent(id);
                while let Some(a) = up {
                    self.rebound(doc, a);
                    up = doc.parent(a);
                }
            }
        }
        self.revision = doc.revision();
        self.synced = true;
    }

    /// Recomputes a node and its subtree under the parent's world place.
    fn refresh(&mut self, doc: &Document, id: u32, parent: Place) {
        let (Some(node), Some((_, content))) = (doc.node(id), doc.stamps(id)) else {
            return;
        };
        let own = each_node!(node, n => Place::of(n.origin, n.rotation));
        let world = parent.compose(&own);
        let stale = self.items.get(&id).is_none_or(|i| i.content != content);
        if stale {
            let (seats, points, pad) = local(node, doc.settings());
            let item = Item {
                world,
                bounds: None,
                content,
                seats,
                points,
                pad,
            };
            self.items.insert(id, item);
        } else if let Some(item) = self.items.get_mut(&id) {
            item.world = world;
        }
        for &child in doc.children(Some(id)) {
            self.refresh(doc, child, world);
        }
        self.rebound(doc, id);
    }

    /// Recomputes a node's bounds from its cached geometry and, for a
    /// section, its children's bounds.
    fn rebound(&mut self, doc: &Document, id: u32) {
        let children = doc
            .children(Some(id))
            .iter()
            .filter_map(|c| self.items.get(c).and_then(|i| i.bounds))
            .fold(None, |a, b| Rect::union(a, Some(b)));
        if let Some(item) = self.items.get_mut(&id) {
            let world = item.world;
            let present = item.seats.iter().filter(|s| !s.removed).map(|s| s.pos);
            let own = present
                .chain(item.points.iter().copied())
                .map(|p| world.apply(p))
                .filter(|p| p.iter().all(|v| v.is_finite()))
                .fold(None, |a, p| Rect::union(a, Some(Rect::new(p, p))))
                .map(|r| r.grow(item.pad));
            item.bounds = Rect::union(own, children);
        }
    }

    /// The node's bounds in world space; `None` for a node with no extent.
    pub fn bounds(&self, id: u32) -> Option<Rect> {
        self.items.get(&id).and_then(|i| i.bounds)
    }

    /// The topmost visible node among `scope`'s children (the top level for
    /// `None`) whose bounds hold the point: click selection, one nesting level
    /// at a time (07 §5.1).
    pub fn hit(&self, doc: &Document, point: P, scope: Option<u32>) -> Option<u32> {
        doc.children(scope).iter().rev().copied().find(|&id| {
            visible(doc, id) && self.bounds(id).is_some_and(|b| b.contains_point(point))
        })
    }

    /// Visible nodes among `scope`'s children inside or touching `rect`, in
    /// document order: box selection.
    pub fn select(&self, doc: &Document, rect: Rect, scope: Option<u32>, mode: Select) -> Vec<u32> {
        let matches = |b: Rect| match mode {
            Select::Contain => rect.contains(&b),
            Select::Touch => rect.intersects(&b),
        };
        doc.children(scope)
            .iter()
            .copied()
            .filter(|&id| visible(doc, id) && self.bounds(id).is_some_and(matches))
            .collect()
    }

    /// Snap anchors of every node, at any depth, whose bounds touch `rect`
    /// (the region around the viewport or the dragged selection), except the
    /// nodes in `exclude` (sorted). In id order.
    pub fn anchors(&self, rect: Rect, exclude: &[u32]) -> Vec<Anchor> {
        let mut out = Vec::new();
        for (&node, item) in &self.items {
            let Some(b) = item.bounds.filter(|b| b.intersects(&rect)) else {
                continue;
            };
            if exclude.binary_search(&node).is_ok() {
                continue;
            }
            let corners = [b.min, [b.max[0], b.min[1]], b.max, [b.min[0], b.max[1]]];
            let centre = [(b.min[0] + b.max[0]) / 2.0, (b.min[1] + b.max[1]) / 2.0];
            let anchor = |kind, point| Anchor { node, kind, point };
            out.extend(corners.map(|p| anchor(AnchorKind::Corner, p)));
            out.push(anchor(AnchorKind::Centre, centre));
            let present: Vec<&CachedSeat> = item.seats.iter().filter(|s| !s.removed).collect();
            for row in present.chunk_by(|a, b| a.row == b.row) {
                if let (Some(first), Some(last)) = (row.first(), row.last()) {
                    out.push(anchor(AnchorKind::RowEnd, item.world.apply(first.pos)));
                    if row.len() > 1 {
                        out.push(anchor(AnchorKind::RowEnd, item.world.apply(last.pos)));
                    }
                }
            }
        }
        out
    }

    /// The seat position of a row block or row nearest to `point` within
    /// `radius`, removed positions included: seat-edit mode.
    pub fn seat_at(&self, id: u32, point: P, radius: f64) -> Option<u32> {
        let item = self.items.get(&id)?;
        let p = item.world.invert(point);
        let d2 = |s: &CachedSeat| (s.pos[0] - p[0]).powi(2) + (s.pos[1] - p[1]).powi(2);
        let mut best: Option<(f64, u32)> = None;
        for s in &item.seats {
            let d = d2(s);
            if d <= radius * radius && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, s.oid));
            }
        }
        best.map(|(_, oid)| oid)
    }

    /// The node's expanded seats in its own space, for the live scene.
    pub fn seats(&self, id: u32) -> &[CachedSeat] {
        self.items.get(&id).map_or(&[], |i| &i.seats)
    }
}

fn visible(doc: &Document, id: u32) -> bool {
    doc.node(id).is_some_and(|n| !each_node!(n, x => x.hidden))
}

/// A node's own geometry: expanded seats, outline points, and the padding
/// around them.
fn local(node: &Node, settings: &Settings) -> (Vec<CachedSeat>, Vec<P>, f64) {
    let seat = finite(settings.seat_size);
    let rect = |[w, h]: [f64; 2]| {
        let (x, y) = (w / 2.0, h / 2.0);
        vec![[-x, -y], [x, -y], [x, y], [-x, y]]
    };
    match node {
        Node::RowBlock(_) | Node::Row(_) => (
            expand(node, settings.alpha_overflow),
            Vec::new(),
            seat / 2.0,
        ),
        Node::Section(s) => match &s.outline {
            Outline::Points(p) => (Vec::new(), p.points.clone(), 0.0),
            Outline::Auto => Default::default(),
        },
        Node::Table(t) => match t.shape {
            TableShape::Round => (
                Vec::new(),
                vec![[0.0, 0.0]],
                finite(t.radius.unwrap_or(0.0)) + seat,
            ),
            TableShape::Rect => (Vec::new(), rect(t.size.unwrap_or_default()), seat),
        },
        Node::Booth(b) => (Vec::new(), rect(b.size), 0.0),
        Node::Area(a) => (Vec::new(), a.shape.points.clone(), 0.0),
        Node::Shape(s) => {
            let points = match s.kind {
                ShapeKind::Rect | ShapeKind::Ellipse => rect(s.size.unwrap_or_default()),
                ShapeKind::Polygon | ShapeKind::Line => s.points.clone().unwrap_or_default(),
                // SVG path data is parsed by the compiler only.
                ShapeKind::Path => Vec::new(),
            };
            (Vec::new(), points, finite(s.style.stroke_width) / 2.0)
        }
        Node::Text(t) => {
            // The label text model of 03 §2.7.
            let w = 0.6 * t.text.chars().count() as f64 * t.size;
            let h = 1.2 * t.size / 2.0;
            let (x0, x1) = match t.align {
                Align::Left => (0.0, w),
                Align::Center => (-w / 2.0, w / 2.0),
                Align::Right => (-w, 0.0),
            };
            (Vec::new(), vec![[x0, -h], [x1, -h], [x1, h], [x0, h]], 0.0)
        }
        Node::Icon(i) => (Vec::new(), rect([i.size, i.size]), 0.0),
        Node::Image(i) => (Vec::new(), rect(i.size), 0.0),
    }
}

fn finite(v: f64) -> f64 {
    if v.is_finite() { v.abs() } else { 0.0 }
}

/// A row block's or row's seats. If its labels do not expand (a user may be
/// halfway through fixing them), the seats are placed without them.
fn expand(node: &Node, overflow: AlphaOverflow) -> Vec<CachedSeat> {
    if let Some(seats) = try_expand(node, overflow) {
        return seats;
    }
    let mut plain = node.clone();
    match &mut plain {
        Node::RowBlock(b) => {
            b.row_labels = RowLabels::default();
            b.seat_labels = SeatLabels::default();
            b.rows.iter_mut().for_each(|r| r.label = None);
            b.seat_overrides.values_mut().for_each(|o| o.label = None);
        }
        Node::Row(r) => {
            r.label = None;
            r.seat_labels = SeatLabels::default();
            r.seat_overrides.values_mut().for_each(|o| o.label = None);
        }
        _ => {}
    }
    try_expand(&plain, overflow).unwrap_or_default()
}

fn try_expand(node: &Node, overflow: AlphaOverflow) -> Option<Vec<CachedSeat>> {
    let mut e = Expansion::new();
    let ok = match node {
        Node::RowBlock(b) => e.push_row_block(b, overflow).is_ok(),
        Node::Row(r) => e.push_row(r, overflow).is_ok(),
        _ => false,
    };
    ok.then(|| {
        e.seats
            .iter()
            .map(|s| CachedSeat {
                pos: s.pos,
                oid: s.oid,
                row: s.row,
                removed: s.removed,
            })
            .collect()
    })
}

#[cfg(test)]
mod tests;
