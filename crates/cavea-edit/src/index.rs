//! The spatial index behind selection and snapping (`docs/07-designer.md`
//! §5.2): each node's bounds in world space and in its own space, and each
//! seat node's expanded seats in its own space, cached and brought up to date
//! from the document's change tracking by [`SpatialIndex::sync`].
//!
//! A chart has a few hundred to a few thousand nodes (row blocks are
//! parametric), so queries scan the cached bounds of the nodes at one level in
//! document order: no tree or grid is needed to stay far inside a frame.

use std::collections::BTreeMap;

use cavea_compiler::Expansion;
use cavea_compiler::document::{
    Align, AlphaOverflow, LabelScheme, Node, Outline, RowLabels, SeatLabels, Settings, ShapeKind,
    TableShape,
};

use crate::document::{Document, each_node};

pub(crate) type P = [f64; 2];

/// Four corners in chart coordinates: a box in some node's space, carried
/// into the world, so it follows the node's rotation.
pub type Quad = [P; 4];

/// An axis-aligned box in chart units.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
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

    pub(crate) fn union(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
        match (a, b) {
            (Some(a), Some(b)) => Some(Rect::new(
                [a.min[0].min(b.min[0]), a.min[1].min(b.min[1])],
                [a.max[0].max(b.max[0]), a.max[1].max(b.max[1])],
            )),
            (a, b) => a.or(b),
        }
    }

    pub(crate) fn grow(self, pad: f64) -> Rect {
        Rect {
            min: [self.min[0] - pad, self.min[1] - pad],
            max: [self.max[0] + pad, self.max[1] + pad],
        }
    }

    /// Clockwise from the top left, with y down.
    pub(crate) fn corners(&self) -> Quad {
        [
            self.min,
            [self.max[0], self.min[1]],
            self.max,
            [self.min[0], self.max[1]],
        ]
    }

    /// The bounds of finite points, if any.
    pub(crate) fn around(points: impl Iterator<Item = P>) -> Option<Rect> {
        points
            .filter(|p| p.iter().all(|v| v.is_finite()))
            .fold(None, |a, p| Rect::union(a, Some(Rect::new(p, p))))
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
pub(crate) struct Place {
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

    pub(crate) fn of(origin: P, rotation: f64) -> Self {
        let (sin, cos) = (rotation % 360.0).to_radians().sin_cos();
        Self {
            cos,
            sin,
            t: origin,
        }
    }

    /// With y down, turns +x towards +y: clockwise on screen, as the compiler.
    pub(crate) fn apply(&self, p: P) -> P {
        let [x, y] = self.turn(p);
        [x + self.t[0], y + self.t[1]]
    }

    /// The rotation alone, for vectors.
    pub(crate) fn turn(&self, [x, y]: P) -> P {
        [x * self.cos - y * self.sin, x * self.sin + y * self.cos]
    }

    /// `self` after `inner`: the parent's place applied to a child's.
    fn compose(&self, inner: &Place) -> Place {
        Place {
            cos: self.cos * inner.cos - self.sin * inner.sin,
            sin: self.sin * inner.cos + self.cos * inner.sin,
            t: self.apply(inner.t),
        }
    }

    pub(crate) fn invert(&self, p: P) -> P {
        self.unturn([p[0] - self.t[0], p[1] - self.t[1]])
    }

    fn unturn(&self, [x, y]: P) -> P {
        [x * self.cos + y * self.sin, -x * self.sin + y * self.cos]
    }
}

#[derive(Debug)]
struct Item {
    world: Place,
    /// World bounds, children included.
    bounds: Option<Rect>,
    /// Bounds in the node's own space: its own geometry with its padding.
    own: Option<Rect>,
    /// `own` with the children's boxes, for a section.
    local: Option<Rect>,
    /// The node's content stamp when `seats` and `points` were made.
    content: u64,
    seats: Vec<CachedSeat>,
    /// Outline points in the node's own space, for nodes without seats.
    points: Vec<P>,
    pad: f64,
    /// For seat nodes: a point this near a seat is on the node.
    reach: f64,
}

impl Item {
    fn quad(&self) -> Option<Quad> {
        self.local.map(|r| r.corners().map(|p| self.world.apply(p)))
    }
}

#[derive(Debug, Default)]
pub struct SpatialIndex {
    revision: u64,
    /// The settings the cached geometry was made with: none before the first
    /// sync.
    settings: Option<(f64, AlphaOverflow)>,
    items: BTreeMap<u32, Item>,
}

impl SpatialIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Brings the index up to date with `doc`: only nodes changed since the
    /// last sync are recomputed, and only those whose content changed are
    /// expanded again. A change of seat size or alphabet overflow recomputes
    /// all; other chart-level changes recompute nothing.
    pub fn sync(&mut self, doc: &Document) {
        let changes = doc.changes_since(self.revision);
        for id in &changes.removed {
            self.items.remove(id);
        }
        let key = (doc.settings().seat_size, doc.settings().alpha_overflow);
        let full = self
            .settings
            .is_none_or(|k| k.0.to_bits() != key.0.to_bits() || k.1 != key.1);
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
        self.settings = Some(key);
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
            let reach = reach(node, doc.settings()).max(pad);
            let item = Item {
                world,
                bounds: None,
                own: None,
                local: None,
                content,
                seats,
                points,
                pad,
                reach,
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
        let Some(world) = self.items.get(&id).map(|i| i.world) else {
            return;
        };
        let (mut kids, mut kids_local) = (None, None);
        for c in doc.children(Some(id)) {
            if let Some(child) = self.items.get(c) {
                kids = Rect::union(kids, child.bounds);
                let quad = child.quad().into_iter().flatten();
                kids_local = Rect::union(kids_local, Rect::around(quad.map(|p| world.invert(p))));
            }
        }
        if let Some(item) = self.items.get_mut(&id) {
            let present = item.seats.iter().filter(|s| !s.removed).map(|s| s.pos);
            let points = || present.clone().chain(item.points.iter().copied());
            item.own = Rect::around(points()).map(|r| r.grow(item.pad));
            item.local = Rect::union(item.own, kids_local);
            let own = Rect::around(points().map(|p| world.apply(p))).map(|r| r.grow(item.pad));
            item.bounds = Rect::union(own, kids);
        }
    }

    /// The node's bounds in world space; `None` for a node with no extent.
    pub fn bounds(&self, id: u32) -> Option<Rect> {
        self.items.get(&id).and_then(|i| i.bounds)
    }

    /// The node's box in its own space, carried into the world: its
    /// selection outline.
    pub fn quad(&self, id: u32) -> Option<Quad> {
        self.items.get(&id).and_then(Item::quad)
    }

    /// A point in the node's own space, in the world.
    pub fn to_world(&self, id: u32, p: P) -> Option<P> {
        self.items.get(&id).map(|i| i.world.apply(p))
    }

    /// The node's own space as placed in the world now.
    pub(crate) fn place(&self, id: u32) -> Option<Place> {
        self.items.get(&id).map(|i| i.world)
    }

    /// A world point in the space of `parent`'s children (the world for
    /// `None`).
    pub(crate) fn to_local(&self, parent: Option<u32>, p: P) -> P {
        match parent.and_then(|s| self.items.get(&s)) {
            Some(item) => item.world.invert(p),
            None => p,
        }
    }

    /// A world vector in the space of `parent`'s children (the world for
    /// `None`): what a node's origin moves by to move by `v` on screen.
    pub fn to_parent(&self, parent: Option<u32>, v: P) -> P {
        match parent.and_then(|p| self.items.get(&p)) {
            Some(item) => item.world.unturn(v),
            None => v,
        }
    }

    /// Whether `point` lies on the node, within `tol`: within `reach` of a
    /// present seat for a row block or row; on a child, or inside an explicit
    /// outline's box, for a section; inside the node's box in its own space
    /// for anything else.
    pub fn on_node(&self, doc: &Document, id: u32, point: P, tol: f64) -> bool {
        let Some(item) = self.items.get(&id) else {
            return false;
        };
        // A section's children test their own bounds, grown by their reach.
        let kids = doc.children(Some(id));
        let near = |b: Rect| b.grow(item.reach + tol).contains_point(point);
        if kids.is_empty() && !item.bounds.is_some_and(near) {
            return false;
        }
        let p = item.world.invert(point);
        if !item.seats.is_empty() {
            let r2 = (item.reach + tol).powi(2);
            return item
                .seats
                .iter()
                .any(|s| !s.removed && dist2(s.pos, p) <= r2);
        }
        let own = item.own.is_some_and(|r| r.grow(tol).contains_point(p));
        own || kids.iter().any(|&c| self.on_node(doc, c, point, tol))
    }

    /// The topmost node among `scope`'s children (the top level for `None`)
    /// that `pick` accepts and `point` is on: click selection, one nesting
    /// level at a time (07 §5.1).
    pub fn hit(
        &self,
        doc: &Document,
        scope: Option<u32>,
        point: P,
        tol: f64,
        pick: impl Fn(u32) -> bool,
    ) -> Option<u32> {
        doc.children(scope)
            .iter()
            .rev()
            .copied()
            .find(|&id| pick(id) && self.on_node(doc, id, point, tol))
    }

    /// Nodes among `scope`'s children that `pick` accepts and whose bounds
    /// `rect` contains, or that it touches, in document order: box selection.
    /// A row block or row is touched through one of its seats, a section
    /// through a child or its explicit outline, anything else through its
    /// world bounds.
    pub fn select(
        &self,
        doc: &Document,
        rect: Rect,
        scope: Option<u32>,
        mode: Select,
        pick: impl Fn(u32) -> bool,
    ) -> Vec<u32> {
        doc.children(scope)
            .iter()
            .copied()
            .filter(|&id| {
                pick(id)
                    && match mode {
                        Select::Contain => self.bounds(id).is_some_and(|b| rect.contains(&b)),
                        Select::Touch => self.touches(doc, id, &rect),
                    }
            })
            .collect()
    }

    fn touches(&self, doc: &Document, id: u32, rect: &Rect) -> bool {
        let Some(item) = self.items.get(&id) else {
            return false;
        };
        if !item.bounds.is_some_and(|b| b.intersects(rect)) {
            return false;
        }
        if !item.seats.is_empty() {
            let grown = rect.grow(item.pad);
            return item
                .seats
                .iter()
                .any(|s| !s.removed && grown.contains_point(item.world.apply(s.pos)));
        }
        if !doc.children(Some(id)).is_empty() {
            let own = Rect::around(item.points.iter().map(|&p| item.world.apply(p)));
            return own.is_some_and(|b| b.intersects(rect))
                || doc
                    .children(Some(id))
                    .iter()
                    .any(|&c| self.touches(doc, c, rect));
        }
        true
    }

    /// Every node, at any depth, whose world bounds touch `rect`, with them,
    /// in id order: snapping's neighbours.
    pub(crate) fn near(&self, rect: Rect) -> impl Iterator<Item = (u32, Rect)> + '_ {
        let touching = move |b: &Rect| b.intersects(&rect);
        self.items
            .iter()
            .filter_map(move |(&id, i)| i.bounds.filter(touching).map(|b| (id, b)))
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
            let centre = [(b.min[0] + b.max[0]) / 2.0, (b.min[1] + b.max[1]) / 2.0];
            let anchor = |kind, point| Anchor { node, kind, point };
            out.extend(b.corners().map(|p| anchor(AnchorKind::Corner, p)));
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
    /// `radius`, removed positions included: rows and seat-edit mode. Of two
    /// at the same distance, the first in the node's order.
    pub fn seat_at(&self, id: u32, point: P, radius: f64) -> Option<&CachedSeat> {
        let item = self.items.get(&id)?;
        let p = item.world.invert(point);
        let mut best: Option<(f64, &CachedSeat)> = None;
        for s in &item.seats {
            let d = dist2(s.pos, p);
            if d <= radius * radius && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, s));
            }
        }
        best.map(|(_, s)| s)
    }

    /// The node's expanded seats in its own space, for the live scene.
    pub fn seats(&self, id: u32) -> &[CachedSeat] {
        self.items.get(&id).map_or(&[], |i| &i.seats)
    }

    /// Half the seat size of a seat node: the padding of a seat's box.
    pub fn seat_pad(&self, id: u32) -> f64 {
        self.items.get(&id).map_or(0.0, |i| i.pad)
    }

    /// How near a seat of the node a point must be to be on it.
    pub fn reach(&self, id: u32) -> f64 {
        self.items.get(&id).map_or(0.0, |i| i.reach)
    }
}

fn dist2(a: P, b: P) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

/// How near a seat of a row block or row a point must be to be on the node:
/// half the diagonal between neighbouring seats in neighbouring rows, so the
/// space between a block's seats and rows is on it while an aisle is not.
fn reach(node: &Node, settings: &Settings) -> f64 {
    let half = finite(settings.seat_size) / 2.0;
    match node {
        Node::RowBlock(b) => half.max(finite(b.seat_pitch).hypot(finite(b.row_pitch)) / 2.0),
        Node::Row(r) => half.max(finite(r.seat_pitch) / 2.0),
        _ => 0.0,
    }
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
                ShapeKind::Path => path_points(s.d.as_deref().unwrap_or_default()),
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

/// The end and control points of SVG path data (03 §2.7: `M L H V C S Q T Z`,
/// absolute and relative, implicit repeats). A Bézier curve lies inside the
/// hull of its control points, so their box holds the flattened path the
/// compiler draws. Reading stops at the first thing it cannot read (the
/// compiler refuses such data, E005); the points before it are kept so the
/// shape can still be selected and fixed.
fn path_points(d: &str) -> Vec<P> {
    let mut out = Vec::new();
    let mut tokens = PathTokens(d.as_bytes());
    let (mut cur, mut start) = ([0.0; 2], [0.0; 2]);
    // The last command's second control point, for `S` and `T`.
    let mut last: Option<(u8, P)> = None;
    let Some(mut cmd) = tokens.command() else {
        return out;
    };
    loop {
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { cur } else { [0.0; 2] };
        let read = |t: &mut PathTokens<'_>, n: usize, out: &mut Vec<P>| -> Option<P> {
            let mut p = None;
            for _ in 0..n {
                let (x, y) = (t.number()?, t.number()?);
                out.push([base[0] + x, base[1] + y]);
                p = out.last().copied();
            }
            p
        };
        let reflect = |kind: u8| match last {
            Some((k, c)) if k == kind => [2.0 * cur[0] - c[0], 2.0 * cur[1] - c[1]],
            _ => cur,
        };
        let upper = cmd.to_ascii_uppercase();
        let mut ctrl = None;
        let step = match upper {
            b'M' | b'L' => read(&mut tokens, 1, &mut out),
            b'C' | b'Q' => {
                let n = if upper == b'C' { 3 } else { 2 };
                let end = read(&mut tokens, n, &mut out);
                // The control point that `S` or `T` reflects next.
                ctrl = out.get(out.len().saturating_sub(2)).map(|&c| (upper, c));
                end
            }
            b'S' => {
                out.push(reflect(b'C'));
                let end = read(&mut tokens, 2, &mut out);
                ctrl = out.get(out.len().saturating_sub(2)).map(|&c| (b'C', c));
                end
            }
            b'T' => {
                let c = reflect(b'Q');
                out.push(c);
                ctrl = Some((b'Q', c));
                read(&mut tokens, 1, &mut out)
            }
            b'H' => tokens.number().map(|x| [base[0] + x, cur[1]]),
            b'V' => tokens.number().map(|y| [cur[0], base[1] + y]),
            b'Z' => Some(start),
            _ => None,
        };
        let Some(next) = step else {
            return out;
        };
        if matches!(upper, b'H' | b'V') {
            out.push(next);
        }
        last = ctrl;
        if upper == b'M' {
            start = next;
            cmd = if rel { b'l' } else { b'L' };
        }
        cur = next;
        if upper == b'Z' || !tokens.at_number() {
            match tokens.command() {
                Some(c) => cmd = c,
                None => return out,
            }
        }
    }
}

struct PathTokens<'a>(&'a [u8]);

impl PathTokens<'_> {
    fn skip(&mut self) {
        while let [b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b',', rest @ ..] = self.0 {
            self.0 = rest;
        }
    }

    fn command(&mut self) -> Option<u8> {
        self.skip();
        let (&c, rest) = self.0.split_first()?;
        c.is_ascii_alphabetic().then(|| {
            self.0 = rest;
            c
        })
    }

    fn at_number(&mut self) -> bool {
        self.skip();
        matches!(self.0.first(), Some(b'0'..=b'9' | b'+' | b'-' | b'.'))
    }

    /// A number as SVG writes it: sign, digits, fraction, exponent.
    fn number(&mut self) -> Option<f64> {
        self.skip();
        let s = self.0;
        let digits = |i: &mut usize| {
            let from = *i;
            while s.get(*i).is_some_and(u8::is_ascii_digit) {
                *i += 1;
            }
            *i > from
        };
        let mut i = usize::from(matches!(s.first(), Some(b'+' | b'-')));
        let mut any = digits(&mut i);
        if s.get(i) == Some(&b'.') {
            i += 1;
            any |= digits(&mut i);
        }
        if any && matches!(s.get(i), Some(b'e' | b'E')) {
            let mark = i;
            i += 1 + usize::from(matches!(s.get(i + 1), Some(b'+' | b'-')));
            if !digits(&mut i) {
                i = mark;
            }
        }
        let (text, rest) = s.split_at_checked(i)?;
        let v: f64 = std::str::from_utf8(text).ok()?.parse().ok()?;
        self.0 = rest;
        (any && v.is_finite()).then_some(v)
    }
}

/// A row block's or row's seats. If its labels do not expand (a user may be
/// halfway through fixing them, or a block has more rows than letters can
/// label in 64 bytes), the seats are placed with plain numbers instead.
fn expand(node: &Node, overflow: AlphaOverflow) -> Vec<CachedSeat> {
    if let Some(seats) = try_expand(node, overflow) {
        return seats;
    }
    let mut plain = node.clone();
    match &mut plain {
        Node::RowBlock(b) => {
            b.row_labels = RowLabels {
                scheme: LabelScheme::Numeric,
                ..RowLabels::default()
            };
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
