//! The live scene kept up to date with the session's document
//! (`docs/04-renderer.md` §25, `docs/07-designer.md` §2, §14).
//!
//! The floor shown is drawn as pieces, each compiled alone by the compiler
//! (`cavea_compiler::build`) and merged by `cavea_render::live`: every
//! top-level node outside a section; for each section, its *shell* (the
//! section with no children and its outline given: the fill, the label and
//! its anchor) and each child, compiled inside a copy of its section with a
//! stand-in outline, so that it is placed, coloured and labelled as in the
//! whole chart's compile. A piece remembers what it was compiled from: the
//! content revisions of its node (and section), their placement, the
//! shell's outline and the chart-level data. At each [`LiveSync::sync`]:
//!
//! - a piece whose content changed is compiled again now (one node
//!   re-expanded and its seats re-uploaded);
//! - a piece whose placement alone changed while a gesture is open is drawn
//!   with a transform from where it was compiled ([`Engine::set_transforms`]):
//!   nothing is expanded or uploaded per pointer move;
//! - once no gesture is open, moved pieces are compiled at their new place
//!   (*baked*), at most [`BAKE_SEATS`] seats a frame, the rest staying
//!   transformed until their turn;
//! - chart-level changes: a new category colour or label, or a floor's
//!   name, level or focal point, recompile nothing; anything else at chart
//!   level (settings, the categories' keys or order, the floors' ids or
//!   order) compiles every piece again;
//! - pieces are merged again only when a piece, the order of the top-level
//!   nodes or the floor shown changed.
//!
//! While the numbering panel previews (07 §6), its target nodes are compiled
//! as the numbering would leave them, so the canvas shows the labels it
//! would apply.

use std::collections::{BTreeMap, BTreeSet};

use cavea_compiler::document::{AlphaOverflow, Category, Floor, Node, Outline, Points, Settings};
use cavea_compiler::{ChartDocument, Detail};
use cavea_edit::Session;
use cavea_format::Chart;
use cavea_render::{
    Engine,
    backend::GpuBackend,
    engine::Affine,
    live::{LiveScene, Piece, Role},
};

/// Seats compiled per frame at most to bake moved pieces after a gesture:
/// about 10 ms of compiling (0.5 µs a seat, measured natively).
pub const BAKE_SEATS: usize = 20_000;

/// Which piece: a node (outside a section, or a section's child) or a
/// section's shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Node(u32),
    Shell(u32),
}

/// What a piece was compiled from, apart from its placement.
#[derive(Debug, Clone, PartialEq)]
struct Content {
    /// Content revisions of the node and of its section, if any.
    stamps: [u64; 2],
    section: Option<u32>,
    /// A shell's outline in the section's own space.
    outline: Option<Vec<[f64; 2]>>,
    /// The numbering preview it shows, if any.
    preview: Option<u64>,
    chart: u64,
}

#[derive(Debug)]
struct Entry {
    /// Unique per compile: the engine keeps a piece's uploads by it.
    id: u64,
    piece: Option<Piece>,
    content: Content,
    /// The world placement it was compiled at.
    placed: Affine,
}

/// The chart-level data pieces are compiled with.
#[derive(Debug, Clone, PartialEq)]
struct ChartBasis {
    settings: Settings,
    categories: Vec<Category>,
    floors: Vec<Floor>,
}

impl ChartBasis {
    /// What compiled pieces depend on: the settings, the categories' keys
    /// in order, the floors' ids in order.
    fn shape(&self) -> (Settings, Vec<&str>, Vec<u32>) {
        (
            self.settings.clone(),
            self.categories.iter().map(|c| c.key.as_str()).collect(),
            self.floors.iter().map(|f| f.id).collect(),
        )
    }
}

/// The live scene for one session and its engine.
#[derive(Debug, Default)]
pub struct LiveSync {
    live: LiveScene,
    entries: BTreeMap<Key, Entry>,
    /// What the engine shows: the pieces' keys in order, and the floor.
    shown: Vec<Key>,
    floor: Option<u32>,
    /// The document revision and preview last synced, and whether a gesture
    /// was open.
    revision: Option<u64>,
    preview: Option<u64>,
    gesture: bool,
    /// The chart-level data, its generation, and its compile alone.
    basis: Option<ChartBasis>,
    chart: u64,
    base: Option<Chart>,
    next_id: u64,
    /// Pieces waiting to be baked.
    pending: bool,
    /// Counters for tests and measurements: pieces compiled and seats in
    /// them since made, and merges.
    pub compiled: u64,
    pub compiled_seats: u64,
    pub merges: u64,
}

/// Origin and rotation of a node (07 §2.1): its place in its parent.
fn place_of(node: &Node) -> ([f64; 2], f64) {
    match node {
        Node::Section(n) => (n.origin, n.rotation),
        Node::RowBlock(n) => (n.origin, n.rotation),
        Node::Row(n) => (n.origin, n.rotation),
        Node::Table(n) => (n.origin, n.rotation),
        Node::Booth(n) => (n.origin, n.rotation),
        Node::Area(n) => (n.origin, n.rotation),
        Node::Shape(n) => (n.origin, n.rotation),
        Node::Text(n) => (n.origin, n.rotation),
        Node::Icon(n) => (n.origin, n.rotation),
        Node::Image(n) => (n.origin, n.rotation),
    }
}

fn floor_of(node: &Node) -> Option<u32> {
    match node {
        Node::Section(n) => n.floor,
        Node::RowBlock(n) => n.floor,
        Node::Row(n) => n.floor,
        Node::Table(n) => n.floor,
        Node::Booth(n) => n.floor,
        Node::Area(n) => n.floor,
        Node::Shape(n) => n.floor,
        Node::Text(n) => n.floor,
        Node::Icon(n) => n.floor,
        Node::Image(n) => n.floor,
    }
}

/// Rotation then translation, as the compiler places a node (y down: +x
/// turns towards +y).
fn affine((origin, rotation): ([f64; 2], f64)) -> Affine {
    let (sin, cos) = (rotation % 360.0).to_radians().sin_cos();
    [[cos, sin], [-sin, cos], origin]
}

/// `outer` after `inner`.
fn compose(outer: &Affine, inner: &Affine) -> Affine {
    let m = |c: [f64; 2]| {
        [
            outer[0][0] * c[0] + outer[1][0] * c[1],
            outer[0][1] * c[0] + outer[1][1] * c[1],
        ]
    };
    let t = m(inner[2]);
    [
        m(inner[0]),
        m(inner[1]),
        [t[0] + outer[2][0], t[1] + outer[2][1]],
    ]
}

/// The inverse of a rigid transform.
fn invert(a: &Affine) -> Affine {
    let [[c, s], _, [x, y]] = *a;
    // The transpose of the rotation, and the translation turned back.
    [[c, -s], [s, c], [-(c * x + s * y), -(-s * x + c * y)]]
}

fn same(a: &Affine, b: &Affine) -> bool {
    a.iter()
        .flatten()
        .zip(b.iter().flatten())
        .all(|(x, y)| x == y)
}

/// A stand-in outline for a section's child compiled alone: the child's
/// pieces take their section record from the shell, so this outline is
/// never drawn; a small triangle the compiler accepts.
fn stand_in() -> Outline {
    Outline::Points(Points {
        points: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
    })
}

/// A row block's or row's labels made plain, as the spatial index places
/// a node whose labels do not expand (07 §5.2): so it can still be drawn.
fn plain(node: &Node) -> Node {
    use cavea_compiler::document::{LabelScheme, RowLabels, SeatLabels};
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
    plain
}

impl LiveSync {
    pub fn new() -> Self {
        Self::default()
    }

    /// Brings the engine's live scene up to date with the session. Returns
    /// whether moved pieces are still waiting to be baked, so the host
    /// should draw another frame.
    pub fn sync<B: GpuBackend>(&mut self, session: &Session, engine: &mut Engine<B>) -> bool {
        let doc = session.document();
        let revision = doc.revision();
        let gesture = session.editor().gesture_open();
        let preview = session.numbering_nodes().map(|r| r.preview);
        if self.revision == Some(revision)
            && self.floor == session.floor()
            && self.preview == preview
            && self.gesture == gesture
            && !self.pending
        {
            return false;
        }
        self.revision = Some(revision);
        self.preview = preview;
        self.gesture = gesture;

        // Chart-level data: a new generation when what pieces depend on
        // changed; a new base compile whenever any of it changed.
        let basis = ChartBasis {
            settings: doc.settings().clone(),
            categories: doc.categories().to_vec(),
            floors: doc.floors().to_vec(),
        };
        let mut remerge = self.floor != session.floor();
        if self.basis.as_ref() != Some(&basis) {
            let deep = self
                .basis
                .as_ref()
                .is_none_or(|b| b.shape() != basis.shape());
            if deep {
                self.chart += 1;
            }
            let (base, _) = cavea_compiler::build(&self.document(session, Vec::new(), None));
            self.base = base.filter(|c| c.check().is_ok());
            self.basis = Some(basis);
            remerge = true;
        }
        self.floor = session.floor();

        let previewed: BTreeMap<u32, &Node> = session
            .numbering_nodes()
            .map(|r| r.nodes.iter().map(|(id, n)| (*id, n)).collect())
            .unwrap_or_default();
        let overflow = session.numbering_nodes().map(|r| r.overflow);

        // The pieces of the floor shown, in draw order, with what they are
        // compiled from and where they are now.
        let shown_floor = session.floor();
        let first_floor = doc.floors().first().map(|f| f.id);
        let floor_index = doc
            .floors()
            .iter()
            .position(|f| Some(f.id) == shown_floor)
            .unwrap_or(0) as u32;
        let mut wanted: Vec<(Key, Content, Affine, Option<u32>)> = Vec::new();
        for &root in doc.children(None) {
            let Some(node) = doc.node(root) else { continue };
            if floor_of(node).or(first_floor) != shown_floor {
                continue;
            }
            let content_of = |id: u32| doc.stamps(id).map_or(0, |(_, c)| c);
            let preview_of = |id: u32| previewed.contains_key(&id).then_some(preview).flatten();
            let place = affine(place_of(node));
            match node {
                Node::Section(sec) => {
                    let outline = match &sec.outline {
                        Outline::Points(_) => None,
                        Outline::Auto => Some(session.section_outline(root).unwrap_or_default()),
                    };
                    let content = Content {
                        stamps: [content_of(root), 0],
                        section: None,
                        outline,
                        preview: None,
                        chart: self.chart,
                    };
                    wanted.push((Key::Shell(root), content, place, None));
                    for &child in doc.children(Some(root)) {
                        let Some(c) = doc.node(child) else { continue };
                        let content = Content {
                            stamps: [content_of(child), content_of(root)],
                            section: Some(root),
                            outline: None,
                            preview: preview_of(child),
                            chart: self.chart,
                        };
                        let at = compose(&place, &affine(place_of(c)));
                        wanted.push((Key::Node(child), content, at, Some(root)));
                    }
                }
                _ => {
                    let content = Content {
                        stamps: [content_of(root), 0],
                        section: None,
                        outline: None,
                        preview: preview_of(root),
                        chart: self.chart,
                    };
                    wanted.push((Key::Node(root), content, place, None));
                }
            }
        }

        // Compile what changed; move or bake what was only placed.
        let mut budget = BAKE_SEATS;
        let mut pending = false;
        let mut moves: Vec<(Key, Affine)> = Vec::new();
        for (key, content, at, section) in &wanted {
            let compile = match self.entries.get(key) {
                None => true,
                Some(e) => {
                    // Only placed elsewhere, or a shell whose outline its
                    // children moved: drawn moved during a gesture, baked
                    // after it. Anything else is compiled now.
                    let mut c = content.clone();
                    if matches!(key, Key::Shell(_)) {
                        c.outline.clone_from(&e.content.outline);
                    }
                    let placed = e.content == c;
                    let still = same(&e.placed, at) && e.content == *content;
                    let delta = compose(at, &invert(&e.placed));
                    let seats = e.piece.as_ref().map_or(0, Piece::seats);
                    if still {
                        false
                    } else if !placed {
                        true
                    } else if gesture {
                        moves.push((*key, delta));
                        false
                    } else if seats > budget && budget < BAKE_SEATS {
                        pending = true;
                        moves.push((*key, delta));
                        false
                    } else {
                        budget = budget.saturating_sub(seats);
                        true
                    }
                }
            };
            if compile {
                let source = match key {
                    Key::Node(id) => previewed.get(id).map(|&n| n.clone()),
                    Key::Shell(_) => None,
                };
                let piece = self.compile(session, *key, *section, source, overflow, floor_index);
                self.next_id += 1;
                let entry = Entry {
                    id: self.next_id,
                    piece,
                    content: content.clone(),
                    placed: *at,
                };
                self.entries.insert(*key, entry);
                remerge = true;
            }
        }
        self.pending = pending;

        // Forget pieces whose nodes are gone; keep those of other floors.
        let before = self.entries.len();
        self.entries.retain(|k, _| match k {
            Key::Node(id) | Key::Shell(id) => doc.node(*id).is_some(),
        });
        let keys: Vec<Key> = wanted.iter().map(|(k, ..)| *k).collect();
        remerge |= keys != self.shown || self.entries.len() != before;

        if remerge {
            self.merge(engine, keys, floor_index);
        }
        let index: BTreeMap<Key, usize> = self
            .shown
            .iter()
            .enumerate()
            .map(|(i, k)| (*k, i))
            .collect();
        let moves: Vec<(usize, Affine)> = moves
            .iter()
            .filter_map(|(k, a)| index.get(k).map(|&i| (i, *a)))
            .collect();
        engine.set_transforms(&moves);
        pending
    }

    fn merge<B: GpuBackend>(&mut self, engine: &mut Engine<B>, keys: Vec<Key>, floor: u32) {
        let Some(base) = &self.base else {
            return;
        };
        let pieces: Vec<(&Piece, u64)> = keys
            .iter()
            .filter_map(|k| self.entries.get(k))
            .filter_map(|e| e.piece.as_ref().map(|p| (p, e.id)))
            .collect();
        // Keys of pieces that compiled to nothing are left out of the
        // order, so span indices follow `shown`.
        let shown: Vec<Key> = keys
            .iter()
            .copied()
            .filter(|k| self.entries.get(k).is_some_and(|e| e.piece.is_some()))
            .collect();
        let refs: Vec<&Piece> = pieces.iter().map(|(p, _)| *p).collect();
        let (scene, spans) = self.live.merge(base, floor, &refs);
        let ids = pieces.iter().map(|(_, id)| *id).zip(spans).collect();
        engine.set_live(scene, ids);
        self.shown = shown;
        self.merges += 1;
    }

    /// The chart-level data with `nodes` only.
    fn document(
        &self,
        session: &Session,
        nodes: Vec<Node>,
        overflow: Option<AlphaOverflow>,
    ) -> ChartDocument {
        let doc = session.document();
        let mut settings = doc.settings().clone();
        if let Some(o) = overflow {
            settings.alpha_overflow = o;
        }
        ChartDocument {
            schema: cavea_compiler::SCHEMA,
            name: doc.name().to_owned(),
            settings,
            next_oid: doc.next_oid(),
            categories: doc.categories().to_vec(),
            floors: doc.floors().to_vec(),
            nodes,
        }
    }

    /// Compiles one piece; `None` when it draws nothing.
    fn compile(
        &mut self,
        session: &Session,
        key: Key,
        section: Option<u32>,
        source: Option<Node>,
        overflow: Option<AlphaOverflow>,
        floor: u32,
    ) -> Option<Piece> {
        let doc = session.document();
        let preview = source.is_some();
        let (node, role) = match key {
            Key::Node(id) => {
                let node = source.or_else(|| doc.tree(id))?;
                match section.and_then(|s| doc.node(s)) {
                    Some(Node::Section(sec)) => {
                        let mut wrapper = sec.clone();
                        wrapper.outline = stand_in();
                        wrapper.children = vec![node];
                        (Node::Section(wrapper), Role::Child(sec.id))
                    }
                    _ => (node, Role::Node),
                }
            }
            Key::Shell(id) => {
                let Some(Node::Section(sec)) = doc.node(id) else {
                    return None;
                };
                let mut shell = sec.clone();
                shell.children = Vec::new();
                if shell.outline == Outline::Auto {
                    let points = session.section_outline(id).unwrap_or_default();
                    if !points.is_empty() {
                        shell.outline = Outline::Points(Points { points });
                    }
                }
                (Node::Section(shell), Role::Shell(id))
            }
        };
        let overflow = overflow.filter(|_| preview);
        let build = |node: Node| {
            let (chart, findings) =
                cavea_compiler::build(&self.document(session, vec![node], overflow));
            let expand = findings
                .iter()
                .any(|f| matches!(f.detail, Detail::Expand(_)));
            (chart.filter(|c| c.check().is_ok()), expand)
        };
        let (mut chart, expand) = build(node.clone());
        if expand || chart.is_none() {
            // Labels that do not expand: drawn with plain numbers, as the
            // spatial index places them.
            let plain = match node {
                Node::Section(mut s) => {
                    s.children = s.children.iter().map(plain).collect();
                    Node::Section(s)
                }
                other => plain(&other),
            };
            chart = build(plain).0.or(chart);
        }
        let chart = chart?;
        self.compiled += 1;
        self.compiled_seats += chart.seat_pos.len() as u64;
        Some(self.live.piece(chart, floor, role))
    }

    /// Whether moved pieces wait to be baked.
    pub fn pending(&self) -> bool {
        self.pending
    }

    /// The pieces shown, in order, for tests.
    pub fn pieces(&self) -> Vec<&Piece> {
        self.shown
            .iter()
            .filter_map(|k| self.entries.get(k))
            .filter_map(|e| e.piece.as_ref())
            .collect()
    }

    /// Nodes with a piece compiled, for tests.
    pub fn nodes(&self) -> BTreeSet<u32> {
        self.entries
            .keys()
            .map(|k| match k {
                Key::Node(id) | Key::Shell(id) => *id,
            })
            .collect()
    }
}
