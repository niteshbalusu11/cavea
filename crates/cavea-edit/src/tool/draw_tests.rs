//! Scripted traces for the drawing tools and the select tool's handles
//! (13 §9): each produces an expected document; generated traces always
//! leave a document that parses, entries that undo byte for byte, fresh
//! ids and no open gesture.

use std::collections::BTreeMap;

use cavea_compiler::document::{Curve, Node, Outline, Path, Points, ShapeKind};
use cavea_compiler::{ChartDocument, compile};

use crate::tests::Rng;
use crate::*;

const NONE: Modifiers = Modifiers {
    shift: false,
    alt: false,
    ctrl: false,
};
const SHIFT: Modifiers = Modifiers {
    shift: true,
    ..NONE
};
const CTRL: Modifiers = Modifiers { ctrl: true, ..NONE };

/// An empty chart with one category, ids from 1000. Zoom 1.
fn empty() -> ChartDocument {
    serde_json::from_value(serde_json::json!({
        "schema": 1, "name": "t", "nextOid": 1000,
        "categories": [{ "key": "std", "label": "Standard", "color": "#2563EB" }],
        "floors": [{ "id": 1, "name": "Stalls", "focalPoint": [0, -1000] }],
        "nodes": []
    }))
    .expect("chart")
}

/// The traces of the slices before snapping: with it off.
pub(crate) fn unsnapped(mut s: Session) -> Session {
    s.set_snap(SnapSettings {
        grid: false,
        objects: false,
        spacing: false,
        rows: false,
        angles: false,
        ..SnapSettings::default()
    });
    s
}

struct T {
    s: Session,
    time: f64,
    zoom: f64,
}

impl T {
    fn new(doc: ChartDocument) -> Self {
        Self {
            s: Session::load(doc).map(unsnapped).expect("load"),
            time: 0.0,
            zoom: 1.0,
        }
    }

    fn p(&self, at: [f64; 2], mods: Modifiers) -> Pointer {
        Pointer {
            world: at,
            screen: [at[0] * self.zoom, at[1] * self.zoom],
            zoom: self.zoom,
            button: Button::Primary,
            kind: PointerKind::Mouse,
            mods,
            time: self.time,
        }
    }

    fn down(&mut self, at: [f64; 2], mods: Modifiers) {
        self.time += 1_000.0;
        self.s.input(Input::PointerDown(self.p(at, mods)));
    }

    fn to(&mut self, at: [f64; 2], mods: Modifiers) {
        self.time += 10.0;
        self.s.input(Input::PointerMove(self.p(at, mods)));
    }

    fn up(&mut self, at: [f64; 2], mods: Modifiers) {
        self.time += 10.0;
        self.s.input(Input::PointerUp(self.p(at, mods)));
    }

    fn click(&mut self, at: [f64; 2]) {
        self.down(at, NONE);
        self.up(at, NONE);
    }

    fn drag(&mut self, from: [f64; 2], to: [f64; 2], mods: Modifiers) {
        self.down(from, mods);
        self.to([(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0], mods);
        self.to(to, mods);
        self.up(to, mods);
    }

    fn key(&mut self, key: Key) {
        self.s.input(Input::KeyDown(key, NONE));
        self.s.input(Input::KeyUp(key, NONE));
    }

    fn typed(&mut self, text: &str) {
        text.chars().for_each(|c| self.key(Key::Char(c)));
    }

    fn state(&self) -> Vec<u8> {
        self.s.document().snapshot().to_json().expect("json")
    }

    fn node(&self, id: u32) -> Node {
        self.s.document().node(id).cloned().expect("node")
    }

    /// The node selected, which a tool leaves selected after creating it.
    fn selected(&self) -> u32 {
        let items = self.s.selection().items();
        assert_eq!(items.len(), 1, "one node selected");
        items.first().copied().unwrap_or_default()
    }

    /// Where a handle of the selection is drawn.
    fn handle(&self, kind: HandleKind) -> [f64; 2] {
        let h = self
            .s
            .overlay()
            .handles
            .into_iter()
            .find(|h| h.kind == kind);
        let h = h.expect("handle");
        [h.at[0] + h.offset[0], h.at[1] + h.offset[1]]
    }

    /// Drags a handle by `d`.
    fn by(&mut self, kind: HandleKind, d: [f64; 2], mods: Modifiers) {
        let from = self.handle(kind);
        self.drag(from, [from[0] + d[0], from[1] + d[1]], mods);
    }

    /// Drags the rotate handle `degrees` about `pivot`.
    fn turn_by(&mut self, pivot: [f64; 2], degrees: f64, mods: Modifiers) {
        let from = self.handle(HandleKind::Rotate);
        self.drag(from, turn(from, pivot, degrees), mods);
    }

    /// World seat positions of a row block or row, by oid.
    fn seats(&self, id: u32) -> BTreeMap<u32, [f64; 2]> {
        let ix = self.s.index();
        ix.seats(id)
            .iter()
            .map(|s| (s.oid, ix.to_world(id, s.pos).expect("world")))
            .collect()
    }
}

fn close(a: [f64; 2], b: [f64; 2], tol: f64) -> bool {
    (a[0] - b[0]).abs() <= tol && (a[1] - b[1]).abs() <= tol
}

fn block(n: &Node) -> &cavea_compiler::document::RowBlock {
    match n {
        Node::RowBlock(b) => b,
        _ => panic!("not a row block"),
    }
}

/// Every seat kept in `after` where it was in `before`.
fn kept(before: &BTreeMap<u32, [f64; 2]>, after: &BTreeMap<u32, [f64; 2]>, tol: f64) -> usize {
    let mut n = 0;
    for (oid, p) in after {
        if let Some(q) = before.get(oid) {
            assert!(close(*p, *q, tol), "seat {oid} moved: {q:?} → {p:?}");
            n += 1;
        }
    }
    n
}

fn compiles(doc: &ChartDocument) {
    if let Err(e) = compile(doc) {
        panic!("{e}");
    }
}

#[test]
fn row_block_tool() {
    let mut t = T::new(empty());
    t.s.set_tool(ToolId::RowBlock);
    let start = t.state();
    // 5.4 seat pitches by 2.6 row pitches: 3 rows of 5.
    t.down([100.0, 200.0], NONE);
    t.to([250.0, 300.0], NONE);
    assert!(matches!(
        t.s.overlay().readout,
        Some(Readout::Block {
            rows: 1,
            seats: 3,
            ..
        })
    ));
    t.to([397.0, 447.0], NONE);
    t.up([397.0, 447.0], NONE);
    let id = t.selected();
    assert_eq!(id, 1000);
    let b = block(&t.node(id)).clone();
    assert_eq!(b.origin, [237.5, 247.5]);
    assert_eq!(
        (b.seat_pitch, b.row_pitch, b.curve),
        (55.0, 95.0, Curve::None {})
    );
    assert_eq!(b.category.as_deref(), Some("std"));
    let rows: Vec<_> = b
        .rows
        .iter()
        .map(|r| (r.rid, r.seats, r.oids.clone()))
        .collect();
    assert_eq!(
        rows,
        vec![
            (1001, 5, vec![[1002, 5]]),
            (1007, 5, vec![[1008, 5]]),
            (1013, 5, vec![[1014, 5]])
        ]
    );
    assert_eq!(t.s.editor().undo_len(), 1);
    let drawn = t.state();
    t.s.undo().expect("undo");
    assert_eq!(t.state(), start);
    t.s.redo().expect("redo");
    assert_eq!(t.state(), drawn);
    // Up and left of the press: the same counts, the cells running back.
    t.drag([400.0, 450.0], [103.0, 203.0], NONE);
    let b = block(&t.node(t.selected())).clone();
    assert_eq!(b.origin, [262.5, 212.5]);
    assert_eq!((b.rows.len(), b.rows[0].seats), (3, 5));
    // Shift squares the drag: 300 × 300.
    t.drag([0.0, 2_000.0], [300.0, 2_100.0], SHIFT);
    let b = block(&t.node(t.selected())).clone();
    assert_eq!((b.rows.len(), b.rows[0].seats), (3, 5));
    // A click leaves nothing.
    let before = t.state();
    t.click([0.0, 3_000.0]);
    assert_eq!(t.state(), before);
}

#[test]
fn row_block_numeric_entry_and_escape() {
    let mut t = T::new(empty());
    t.s.set_tool(ToolId::RowBlock);
    let start = t.state();
    let saved = t.s.document().save().next_oid;
    t.down([0.0, 0.0], NONE);
    t.to([200.0, 200.0], NONE);
    t.key(Key::Escape);
    t.up([200.0, 200.0], NONE);
    // Nothing left. The preview kept the same ids however often it changed
    // (2 rows of 4: 11 ids), and they stay taken (07 §9).
    assert_eq!(t.state(), start);
    assert_eq!(t.s.document().save().next_oid, saved + 11);
    assert_eq!(t.s.editor().undo_len(), 0);
    // Typed rows × seats replace the dragged ones; a bare `x6` keeps rows.
    t.down([0.0, 0.0], NONE);
    t.to([200.0, 200.0], NONE);
    t.typed("4x6");
    assert_eq!(t.s.overlay().entry, "4x6");
    t.key(Key::Enter);
    t.to([900.0, 900.0], NONE);
    t.up([900.0, 900.0], NONE);
    let b = block(&t.node(t.selected())).clone();
    assert_eq!(b.id, 1011, "the escaped block's ids are not reused");
    assert_eq!((b.rows.len(), b.rows[0].seats), (4, 6));
    assert_eq!(b.origin, [165.0, 47.5]);
    t.down([0.0, 1_000.0], NONE);
    t.to([200.0, 1_200.0], NONE);
    t.typed("x77");
    t.key(Key::Backspace);
    t.key(Key::Enter);
    t.up([200.0, 1_200.0], NONE);
    let b = block(&t.node(t.selected())).clone();
    assert_eq!((b.rows.len(), b.rows[0].seats), (2, 7));
    // Absurd numbers stay within the limits (02 §14).
    t.down([0.0, 5_000.0], NONE);
    t.to([200.0, 5_200.0], NONE);
    t.typed("99999999999999999999x99999999");
    t.key(Key::Enter);
    t.up([200.0, 5_200.0], NONE);
    let b = block(&t.node(t.selected())).clone();
    assert_eq!(b.rows[0].seats, 4_095);
    assert_eq!(t.s.editor().undo_len(), 3);
    assert!(!t.s.editor().gesture_open());
}

#[test]
fn row_block_in_a_section() {
    let mut t = T::new(empty());
    let section: Box<Node> = Box::new(
        serde_json::from_value(serde_json::json!({
            "id": 0, "type": "section", "label": "Stalls", "origin": [1000, 0], "rotation": 90
        }))
        .expect("section"),
    );
    let s =
        t.s.apply(Command::CreateNode {
            parent: None,
            index: 0,
            node: section,
        })
        .expect("create")
        .created[0];
    t.s.select(Level::Section(s), []);
    t.s.set_tool(ToolId::RowBlock);
    // Turned 90°: world (0, 165) is local (165, 0); world (−190, 0) is (0, 190).
    t.drag([1_000.0, 0.0], [810.0, 165.0], NONE);
    let id = t.selected();
    assert_eq!(t.s.selection().level(), Level::Section(s));
    assert_eq!(t.s.document().parent(id), Some(s));
    let b = block(&t.node(id)).clone();
    assert_eq!((b.rows.len(), b.rows[0].seats), (2, 3));
    assert!(close(b.origin, [82.5, 47.5], 1e-9));
    // Pressed outside the section (it now has a child): the top level.
    t.drag([-3_000.0, 0.0], [-2_800.0, 100.0], NONE);
    assert_eq!(t.s.document().parent(t.selected()), None);
}

/// Clicks the centre, drags the front row from `a` along its circle through
/// `path`, then adds rows out to `out` and clicks.
fn arc(t: &mut T, c: [f64; 2], a: [f64; 2], path: &[[f64; 2]], out: [f64; 2]) -> u32 {
    t.click(c);
    t.down(a, NONE);
    for &p in path {
        t.to(p, NONE);
    }
    let last = path.last().copied().unwrap_or(a);
    t.up(last, NONE);
    t.to(out, NONE);
    t.click(out);
    t.selected()
}

fn on_circle(c: [f64; 2], r: f64, degrees: f64) -> [f64; 2] {
    let (s, k) = degrees.to_radians().sin_cos();
    [c[0] + r * k, c[1] + r * s]
}

#[test]
fn arc_block_tool() {
    let mut t = T::new(empty());
    t.s.set_tool(ToolId::ArcBlock);
    let c = [0.0, 0.0];
    let a = on_circle(c, 1_000.0, 90.0);
    let b = on_circle(c, 1_000.0, 60.0);
    let path: Vec<_> = (1..=6)
        .map(|i| on_circle(c, 1_000.0, 90.0 - 5.0 * f64::from(i)))
        .collect();
    let id = arc(&mut t, c, a, &path, [0.0, 1_190.0]);
    let blk = block(&t.node(id)).clone();
    assert!(matches!(blk.curve, Curve::Arc { radius } if (radius - 1_000.0).abs() < 1e-9));
    // 30° of a 1,000 radius holds 10 seats at 55; each row behind fills it.
    let seats: Vec<u32> = blk.rows.iter().map(|r| r.seats).collect();
    assert_eq!(seats, vec![10, 10, 11]);
    assert!((blk.rotation - -15.0).abs() < 1e-9);
    assert_eq!(t.s.editor().undo_len(), 1);
    // The front row passes through the points given, within a pitch, and
    // rows grow outward.
    let ix = t.s.index();
    let world: Vec<([f64; 2], u32)> = ix
        .seats(id)
        .iter()
        .map(|s| (ix.to_world(id, s.pos).expect("world"), s.row))
        .collect();
    for (p, row) in &world {
        let r = p[0].hypot(p[1]);
        assert!((r - (1_000.0 + 95.0 * f64::from(*row))).abs() < 1e-6, "{r}");
    }
    let front: Vec<[f64; 2]> = world.iter().filter(|s| s.1 == 0).map(|s| s.0).collect();
    for target in [a, b] {
        let near = front
            .iter()
            .map(|p| (p[0] - target[0]).hypot(p[1] - target[1]));
        assert!(near.fold(f64::MAX, f64::min) < 55.0);
    }
    compiles(&t.s.document().save());

    // Swept nearly all the way round: clamped short of a full turn, so the
    // compiler takes it.
    let c = [5_000.0, 0.0];
    let a = on_circle(c, 200.0, 0.0);
    let path: Vec<_> = (1..=40)
        .map(|i| on_circle(c, 200.0, 11.0 * f64::from(i)))
        .collect();
    let id = arc(&mut t, c, a, &path, on_circle(c, 210.0, 0.0));
    assert_eq!(block(&t.node(id)).rows[0].seats, 22);
    compiles(&ChartDocument {
        nodes: vec![t.node(id)],
        ..empty()
    });

    // Typed radius × seats × rows.
    t.click([0.0, 9_000.0]);
    t.down([0.0, 9_500.0], NONE);
    t.to([100.0, 9_500.0], NONE);
    t.typed("800x12x4");
    t.key(Key::Enter);
    t.up([100.0, 9_500.0], NONE);
    let blk = block(&t.node(t.selected())).clone();
    assert!(matches!(blk.curve, Curve::Arc { radius } if radius == 800.0));
    assert_eq!((blk.rows.len(), blk.rows[0].seats), (4, 12));
    // Escape between steps leaves nothing.
    let before = t.state();
    t.click([0.0, -9_000.0]);
    t.down([0.0, -8_000.0], NONE);
    t.to([300.0, -8_100.0], NONE);
    t.up([300.0, -8_100.0], NONE);
    t.key(Key::Escape);
    assert_eq!(t.state(), before);
}

fn shape(n: &Node) -> &cavea_compiler::document::Shape {
    match n {
        Node::Shape(s) => s,
        _ => panic!("not a shape"),
    }
}

/// Creates a node from JSON at the top level and selects it.
fn add(s: &mut Session, node: serde_json::Value) -> u32 {
    let node = Box::new(serde_json::from_value(node).expect("node"));
    let index = s.document().children(None).len() as u32;
    let parent = None;
    let created = s.apply(Command::CreateNode {
        parent,
        index,
        node,
    });
    let id = created.expect("create").created[0];
    s.select(Level::Top, [id]);
    id
}

/// A straight block whose front row is centred on the origin.
fn straight(rows: u32, seats: u32) -> Draft {
    Draft::RowBlock {
        origin: [0.0, 0.0],
        rows,
        seats,
        seat_pitch: 55.0,
        row_pitch: 95.0,
    }
}

/// An arc block opening downward from `center`.
fn fan(center: [f64; 2], radius: f64, rows: u32, seats: u32) -> Draft {
    Draft::ArcBlock {
        center,
        radius,
        direction: 90.0,
        rows,
        seats,
        seat_pitch: 55.0,
        row_pitch: 95.0,
    }
}

fn rect(origin: [f64; 2], size: [f64; 2]) -> serde_json::Value {
    serde_json::json!({ "id": 0, "type": "shape", "kind": "rect", "origin": origin, "size": size })
}

#[test]
fn single_row_tool() {
    let mut t = T::new(empty());
    t.s.set_tool(ToolId::Row);
    // Two clicks, then a click on the end: straight, 11 seats over 550.
    t.click([0.0, 0.0]);
    t.click([550.0, 0.0]);
    t.click([553.0, 4.0]);
    let row = match t.node(t.selected()) {
        Node::Row(r) => r,
        _ => panic!("row"),
    };
    let Path::Polyline { points } = &row.path;
    assert_eq!(
        (row.origin, points.clone()),
        ([0.0, 0.0], vec![[0.0, 0.0], [550.0, 0.0]])
    );
    assert_eq!((row.seats, row.label.as_deref()), (11, Some("A")));
    assert_eq!(row.category.as_deref(), Some("std"));
    // Dragged, then Enter: straight.
    t.drag([0.0, 500.0], [300.0, 500.0], NONE);
    t.key(Key::Enter);
    assert!(matches!(t.node(t.selected()), Node::Row(r) if r.seats == 6));
    // A third click bends it through that point.
    t.click([0.0, 1_000.0]);
    t.click([1_000.0, 1_000.0]);
    t.click([500.0, 1_200.0]);
    let row = match t.node(t.selected()) {
        Node::Row(r) => r,
        _ => panic!("row"),
    };
    let Path::Polyline { points } = &row.path;
    // A sag of 200 needs 20 segments for 0.5 cu (sag / n²).
    assert_eq!(points.len(), 21);
    assert!(close(points[10], [500.0, 200.0], 1e-9));
    let length: f64 = points
        .windows(2)
        .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
        .sum();
    assert_eq!(row.seats, (length / 55.0).floor() as u32 + 1);
    assert!(row.seats > 19);
    assert_eq!(t.s.editor().undo_len(), 3);
    compiles(&ChartDocument {
        nodes: vec![Node::Row(row)],
        ..empty()
    });
    // Escape after the second click leaves nothing.
    let before = t.state();
    t.click([0.0, 2_000.0]);
    t.click([500.0, 2_000.0]);
    t.key(Key::Escape);
    assert_eq!(t.state(), before);
}

#[test]
fn shape_tools() {
    let mut t = T::new(empty());
    t.s.set_tool(ToolId::Rect);
    t.drag([0.0, 0.0], [200.0, -100.0], NONE);
    let s = shape(&t.node(t.selected())).clone();
    assert_eq!(
        (s.kind, s.origin, s.size),
        (ShapeKind::Rect, [100.0, -50.0], Some([200.0, 100.0]))
    );
    assert_eq!(s.style.fill.as_deref(), Some(draft_fill()));
    t.drag([0.0, 0.0], [200.0, 100.0], SHIFT);
    let s = shape(&t.node(t.selected())).clone();
    assert_eq!((s.origin, s.size), ([100.0, 100.0], Some([200.0, 200.0])));
    t.click([0.0, 500.0]);
    assert_eq!(t.s.document().ids().count(), 2, "a click draws nothing");
    t.s.set_tool(ToolId::Ellipse);
    t.drag([0.0, 0.0], [-50.0, 80.0], NONE);
    let s = shape(&t.node(t.selected())).clone();
    assert_eq!(
        (s.kind, s.origin, s.size),
        (ShapeKind::Ellipse, [-25.0, 40.0], Some([50.0, 80.0]))
    );
    // Lines: dragged with Shift (15° steps), or two clicks.
    t.s.set_tool(ToolId::Line);
    t.drag([0.0, 0.0], [100.0, 30.0], SHIFT);
    let s = shape(&t.node(t.selected())).clone();
    let end = s
        .points
        .as_deref()
        .and_then(|p| p.last().copied())
        .expect("end");
    let r = 100f64.hypot(30.0);
    assert!(close(
        end,
        [r * 15f64.to_radians().cos(), r * 15f64.to_radians().sin()],
        1e-9
    ));
    assert!(s.style.fill.is_none() && s.style.stroke_width > 0.0);
    t.click([0.0, 0.0]);
    t.click([0.0, 100.0]);
    let s = shape(&t.node(t.selected())).clone();
    assert_eq!(s.points, Some(vec![[0.0, 0.0], [0.0, 100.0]]));
    // A polygon closed on its first corner.
    t.s.set_tool(ToolId::Polygon);
    for p in [
        [0.0, 0.0],
        [100.0, 0.0],
        [100.0, 100.0],
        [0.0, 100.0],
        [1.0, 1.0],
    ] {
        t.click(p);
    }
    let s = shape(&t.node(t.selected())).clone();
    assert_eq!(s.kind, ShapeKind::Polygon);
    assert_eq!(s.points.as_ref().map(Vec::len), Some(4));
    let entries = t.s.editor().undo_len();
    // A corner whose edge crosses the outline is refused; Enter closes.
    for p in [[0.0, 0.0], [100.0, 100.0], [100.0, 0.0], [0.0, 100.0]] {
        t.click(p);
    }
    assert!(matches!(t.s.overlay().readout, Some(Readout::Points(3))));
    t.key(Key::Enter);
    assert_eq!(
        shape(&t.node(t.selected())).points.as_ref().map(Vec::len),
        Some(3)
    );
    // A closing edge that would cross the outline is refused.
    let before = t.state();
    let spiral = [
        [0.0, 0.0],
        [100.0, 0.0],
        [100.0, 100.0],
        [10.0, 100.0],
        [10.0, 30.0],
        [50.0, 30.0],
        [50.0, 70.0],
    ];
    spiral.iter().for_each(|&p| t.click(p));
    t.key(Key::Enter);
    t.click([0.0, 0.0]);
    assert_eq!(t.state(), before);
    t.key(Key::Escape);
    assert_eq!(t.s.overlay().preview, Vec::<[f64; 2]>::new());
    assert_eq!(t.s.editor().undo_len(), entries + 1);
    for id in t.s.document().ids().collect::<Vec<_>>() {
        compiles(&ChartDocument {
            nodes: vec![t.node(id)],
            ..empty()
        });
    }
}

fn draft_fill() -> &'static str {
    "#D1D5DB"
}

#[test]
fn text_tool() {
    let mut t = T::new(empty());
    t.s.set_tool(ToolId::Text);
    t.click([0.0, -500.0]);
    let field = t.s.overlay().text.expect("field");
    assert_eq!(
        (field.node, field.at, field.text.as_str()),
        (None, [0.0, -500.0], "")
    );
    t.s.set_text("ST");
    t.s.set_text("STAGE");
    t.key(Key::Enter);
    let id = t.selected();
    assert!(matches!(t.node(id), Node::Text(x) if x.text == "STAGE" && x.origin == [0.0, -500.0]));
    assert_eq!(t.s.editor().undo_len(), 1);
    assert!(t.s.overlay().text.is_none());
    // Empty on commit, or Escape: nothing.
    let before = t.state();
    t.click([0.0, 500.0]);
    t.s.set_text("x");
    t.s.set_text("");
    t.key(Key::Enter);
    t.click([0.0, 500.0]);
    t.s.set_text("Hi");
    t.key(Key::Escape);
    assert_eq!(t.state(), before);
    // A click away commits.
    t.click([0.0, 900.0]);
    t.s.set_text("Bar");
    t.click([500.0, 900.0]);
    assert_eq!(t.s.editor().undo_len(), 2);
    // A double click with the select tool edits the string.
    t.s.set_tool(ToolId::Select);
    t.click([0.0, -500.0]);
    t.time -= 900.0;
    t.click([0.0, -500.0]);
    assert_eq!(t.s.overlay().text.and_then(|f| f.node), Some(id));
    t.s.set_text("STAGE LEFT");
    t.key(Key::Enter);
    assert!(matches!(t.node(id), Node::Text(x) if x.text == "STAGE LEFT"));
    assert_eq!(t.s.editor().undo_len(), 3);
}

/// `p` turned by `degrees` about `c` (clockwise, y down).
fn turn(p: [f64; 2], c: [f64; 2], degrees: f64) -> [f64; 2] {
    let (s, k) = degrees.to_radians().sin_cos();
    let (x, y) = (p[0] - c[0], p[1] - c[1]);
    [c[0] + x * k - y * s, c[1] + x * s + y * k]
}

fn origin(n: &Node) -> ([f64; 2], f64) {
    match n {
        Node::RowBlock(b) => (b.origin, b.rotation),
        Node::Shape(s) => (s.origin, s.rotation),
        _ => panic!("node"),
    }
}

#[test]
fn rotate() {
    let mut t = T::new(empty());
    let start = t.state();
    let a = t.s.insert(None, &straight(2, 4)).expect("block");
    let pivot = t.handle(HandleKind::Pivot);
    // A quarter turn with Shift: exactly 90°, the origin on its circle.
    t.turn_by(pivot, 88.0, SHIFT);
    let (o, r) = origin(&t.node(a));
    assert_eq!(r, 90.0);
    assert!(close(o, turn([0.0, 0.0], pivot, 90.0), 1e-9));
    assert_eq!(t.s.editor().undo_len(), 2);
    // And back: the rotation exactly, the origin within 1e-9 cu.
    t.turn_by(pivot, -91.0, SHIFT);
    let (o, r) = origin(&t.node(a));
    assert_eq!(r, 0.0);
    assert!(close(o, [0.0, 0.0], 1e-9));
    // Escape mid-turn restores.
    let before = t.state();
    let h = t.handle(HandleKind::Rotate);
    t.down(h, NONE);
    t.to(turn(h, pivot, 40.0), NONE);
    assert!(
        matches!(t.s.overlay().readout, Some(Readout::Rotate { angle }) if (angle - 40.0).abs() < 1e-6)
    );
    t.key(Key::Escape);
    t.up(turn(h, pivot, 40.0), NONE);
    assert_eq!(t.state(), before);
    // Several nodes turn about their shared frame's centre.
    let b = add(&mut t.s, rect([1_000.0, 0.0], [100.0, 50.0]));
    t.s.select(Level::Top, [a, b]);
    let pivot = t.handle(HandleKind::Pivot);
    let handles = t.s.overlay().handles;
    assert_eq!(handles.len(), 2, "several nodes: rotate and pivot only");
    let (oa, ob) = (origin(&t.node(a)).0, origin(&t.node(b)).0);
    t.turn_by(pivot, 180.0, SHIFT);
    assert!(close(origin(&t.node(a)).0, turn(oa, pivot, 180.0), 1e-9));
    assert!(close(origin(&t.node(b)).0, turn(ob, pivot, 180.0), 1e-9));
    assert_eq!((origin(&t.node(a)).1, origin(&t.node(b)).1), (180.0, 180.0));
    // A moved pivot.
    t.s.select(Level::Top, [b]);
    let from = t.handle(HandleKind::Pivot);
    t.drag(from, [2_000.0, 0.0], NONE);
    assert_eq!(t.handle(HandleKind::Pivot), [2_000.0, 0.0]);
    let ob = origin(&t.node(b)).0;
    t.turn_by([2_000.0, 0.0], 90.0, SHIFT);
    assert!(close(
        origin(&t.node(b)).0,
        turn(ob, [2_000.0, 0.0], 90.0),
        1e-9
    ));
    assert_eq!(origin(&t.node(b)).1, -90.0);
    while t.s.undo().expect("undo") {}
    assert_eq!(t.state(), start);
}

#[test]
fn resize_row_blocks() {
    let mut t = T::new(empty());
    let id = t.s.insert(None, &straight(3, 4)).expect("block");
    let rids = |t: &T| {
        block(&t.node(id))
            .rows
            .iter()
            .map(|r| r.rid)
            .collect::<Vec<_>>()
    };
    let counts = |t: &T| {
        let b = block(&t.node(id)).clone();
        (
            b.rows.len(),
            b.rows.iter().map(|r| r.seats).collect::<Vec<_>>(),
        )
    };
    let first = rids(&t);
    // The back: two rows more, every seat kept in place.
    let before = t.seats(id);
    t.by(HandleKind::Side(Side::Bottom), [0.0, 190.0], NONE);
    assert_eq!(counts(&t), (5, vec![4; 5]));
    assert_eq!(rids(&t)[..3], first[..]);
    assert_eq!(kept(&before, &t.seats(id), 0.0), 12);
    // The front: one more, the others still in place.
    let before = t.seats(id);
    t.by(HandleKind::Side(Side::Top), [0.0, -100.0], NONE);
    assert_eq!(counts(&t).0, 6);
    assert_eq!(kept(&before, &t.seats(id), 1e-9), 20);
    // Two seats at the left of every row, one off the right.
    let before = t.seats(id);
    t.by(HandleKind::Side(Side::Left), [-110.0, 0.0], NONE);
    assert_eq!(counts(&t), (6, vec![6; 6]));
    assert_eq!(kept(&before, &t.seats(id), 1e-9), 24);
    t.by(HandleKind::Side(Side::Right), [-50.0, 0.0], NONE);
    assert_eq!(counts(&t), (6, vec![5; 6]));
    // At the minimum: one row, one seat.
    t.by(HandleKind::Side(Side::Bottom), [0.0, -9_000.0], NONE);
    t.by(HandleKind::Side(Side::Right), [-9_000.0, 0.0], NONE);
    assert_eq!(counts(&t), (1, vec![1]));
    assert_eq!(t.s.editor().undo_len(), 7);
    // Ctrl: the row pitch, the front row staying.
    t.by(HandleKind::Side(Side::Bottom), [0.0, 190.0], NONE);
    let front = t.seats(id);
    t.by(HandleKind::Side(Side::Bottom), [0.0, 20.0], CTRL);
    assert_eq!(block(&t.node(id)).row_pitch, 105.0);
    let now = t.seats(id);
    let row0 = t.s.index().seats(id).iter().filter(|s| s.row == 0).count();
    assert_eq!(
        front
            .iter()
            .take(row0)
            .filter(|(o, p)| now.get(o) == Some(p))
            .count(),
        row0
    );
    // Undo walks back through each drag, byte for byte to the start.
    while t.s.editor().undo_len() > 1 {
        t.s.undo().expect("undo");
    }
    assert_eq!(counts(&t), (3, vec![4; 3]));
}

/// A staggered block's front takes rows in pairs, so the kept rows keep
/// their parity and their seats stay put.
#[test]
fn resize_staggered_front() {
    let mut t = T::new(empty());
    let id = t.s.insert(None, &straight(2, 3)).expect("block");
    let stagger = Property::Stagger(0.5);
    t.s.apply(Command::SetProperty {
        id,
        property: stagger,
    })
    .expect("stagger");
    t.s.select(Level::Top, [id]);
    let before = t.state();
    t.by(HandleKind::Side(Side::Top), [0.0, -95.0], NONE);
    assert_eq!(t.state(), before);
    let seats = t.seats(id);
    t.by(HandleKind::Side(Side::Top), [0.0, -190.0], NONE);
    assert_eq!(block(&t.node(id)).rows.len(), 4);
    assert_eq!(kept(&seats, &t.seats(id), 1e-9), 6);
}

/// A skewed block's kept seats stay put when rows are added at its front,
/// or the row pitch is changed from there.
#[test]
fn resize_skewed_front() {
    let mut t = T::new(empty());
    let id = t.s.insert(None, &straight(2, 4)).expect("block");
    let skew = Property::Skew(30.0);
    t.s.apply(Command::SetProperty { id, property: skew })
        .expect("skew");
    t.s.select(Level::Top, [id]);
    let seats = t.seats(id);
    t.by(HandleKind::Side(Side::Top), [0.0, -95.0], NONE);
    assert_eq!(block(&t.node(id)).rows.len(), 3);
    assert_eq!(kept(&seats, &t.seats(id), 1e-9), 8);
    let back: Vec<u32> =
        t.s.index()
            .seats(id)
            .iter()
            .filter(|s| s.row == 2)
            .map(|s| s.oid)
            .collect();
    let seats = t.seats(id);
    t.by(HandleKind::Side(Side::Top), [0.0, -20.0], CTRL);
    assert_eq!(block(&t.node(id)).row_pitch, 105.0);
    let now = t.seats(id);
    for oid in back {
        assert!(close(seats[&oid], now[&oid], 1e-9), "back seat {oid} moved");
    }
}

/// A cancelled gesture's ids are never handed out again, even to a node
/// made right after it; and a draft whose numbers would overflow a count
/// is refused, not a panic.
#[test]
fn cancelled_ids_and_overflowing_drafts() {
    let mut e = Editor::load(empty()).expect("load");
    let create = || Command::CreateNode {
        parent: None,
        index: 0,
        node: Box::new(straight(1, 2).node(None, None).expect("node")),
    };
    e.begin_gesture().expect("begin");
    let first = e.apply(create()).expect("create").created;
    e.cancel_gesture().expect("cancel");
    let second = e.apply(create()).expect("create").created;
    assert_ne!(first, second);
    let huge = Draft::ArcBlock {
        center: [0.0, 0.0],
        radius: 55.0,
        direction: 0.0,
        rows: 2,
        seats: 2,
        seat_pitch: 55.0,
        row_pitch: 1e300,
    };
    assert!(huge.node(None, None).is_err());
}

#[test]
fn resize_turned_and_curved_blocks() {
    let mut t = T::new(empty());
    // Turned 30°: the left handle runs along the block's own −x.
    let id = t.s.insert(None, &straight(2, 4)).expect("block");
    t.s.apply(Command::TransformNodes {
        nodes: vec![NodeTransform {
            id,
            origin: [0.0, 0.0],
            rotation: 30.0,
        }],
    })
    .expect("turn");
    t.s.select(Level::Top, [id]);
    let before = t.seats(id);
    let (s, k) = 30f64.to_radians().sin_cos();
    t.by(HandleKind::Side(Side::Left), [-110.0 * k, -110.0 * s], NONE);
    assert_eq!(block(&t.node(id)).rows[0].seats, 6);
    assert_eq!(kept(&before, &t.seats(id), 1e-9), 8);
    // An arc: rows added at the front keep the centre and every seat.
    let arc =
        t.s.insert(None, &fan([0.0, 2_000.0], 1_000.0, 2, 9))
            .expect("arc");
    let before = t.seats(arc);
    t.by(HandleKind::Side(Side::Top), [0.0, -95.0], NONE);
    let b = block(&t.node(arc)).clone();
    assert_eq!(b.rows.len(), 3);
    assert!(matches!(b.curve, Curve::Arc { radius } if (radius - 905.0).abs() < 1e-9));
    assert_eq!(kept(&before, &t.seats(arc), 1e-6), 9 + 9);
    let before = t.seats(arc);
    t.by(HandleKind::Side(Side::Right), [55.0, 0.0], NONE);
    assert_eq!(kept(&before, &t.seats(arc), 1e-6), before.len());
    // The front cannot pass the centre: the arc stops at its last row.
    t.by(HandleKind::Side(Side::Top), [0.0, -50_000.0], NONE);
    let b = block(&t.node(arc)).clone();
    assert!(matches!(b.curve, Curve::Arc { radius } if radius > 0.0));
    compiles(&ChartDocument {
        nodes: vec![Node::RowBlock(b)],
        ..empty()
    });
}

#[test]
fn scale_shapes() {
    let mut t = T::new(empty());
    let id = add(&mut t.s, rect([0.0, 0.0], [200.0, 100.0]));
    let start = t.state();
    let size = |t: &T| (shape(&t.node(id)).origin, shape(&t.node(id)).size);
    t.by(HandleKind::Corner(Corner::BottomRight), [100.0, 10.0], NONE);
    assert_eq!(size(&t), ([50.0, 5.0], Some([300.0, 110.0])));
    t.s.undo().expect("undo");
    assert_eq!(t.state(), start);
    t.by(
        HandleKind::Corner(Corner::BottomRight),
        [100.0, 10.0],
        SHIFT,
    );
    assert_eq!(size(&t), ([50.0, 25.0], Some([300.0, 150.0])));
    t.by(HandleKind::Side(Side::Left), [1_000.0, 0.0], NONE);
    assert_eq!(size(&t).1, Some([1.0, 150.0]), "the minimum size");
    // A polygon scales as a whole about the opposite side.
    let points = [[0, 0], [100, 0], [0, 100]];
    let node = serde_json::json!({ "id": 0, "type": "shape", "kind": "polygon", "points": points });
    let poly = add(&mut t.s, node);
    t.by(HandleKind::Side(Side::Right), [100.0, 0.0], NONE);
    assert_eq!(
        shape(&t.node(poly)).points,
        Some(vec![[0.0, 0.0], [200.0, 0.0], [0.0, 100.0]])
    );
    // A line's end follows the pointer.
    let node = serde_json::json!({ "id": 0, "type": "shape", "kind": "line", "origin": [0, 500],
        "points": [[0, 0], [100, 0]], "style": { "stroke": "#000000", "strokeWidth": 4 } });
    let line = add(&mut t.s, node);
    t.by(HandleKind::End(1), [0.0, 100.0], NONE);
    assert_eq!(
        shape(&t.node(line)).points,
        Some(vec![[0.0, 0.0], [100.0, 100.0]])
    );
}

/// A small theatre drawn from an empty document: a straight block in the
/// stalls and a curved block in the circle, around a stage and its label
/// (from commands until their tools come); then the stalls block is turned
/// and given a row. It compiles with no errors.
#[test]
fn draw_a_theatre() {
    let mut t = T::new(empty());
    let mut sections = Vec::new();
    for (i, label) in ["Stalls", "Circle"].into_iter().enumerate() {
        let node = serde_json::from_value(serde_json::json!({
            "id": 0, "type": "section", "label": label
        }))
        .expect("section");
        let applied = t.s.apply(Command::CreateNode {
            parent: None,
            index: i as u32,
            node: Box::new(node),
        });
        sections.push(applied.expect("section").created[0]);
    }
    add(&mut t.s, rect([0.0, -1_050.0], [1_200.0, 300.0]));
    let label = serde_json::json!({ "id": 0, "type": "text", "text": "STAGE", "origin": [0, -1050],
        "size": 120, "color": "#F9FAFB" });
    add(&mut t.s, label);
    t.s.select(Level::Section(sections[0]), []);
    t.s.set_tool(ToolId::RowBlock);
    t.drag([-300.0, -700.0], [300.0, -100.0], NONE);
    let stalls = t.selected();
    t.s.select(Level::Section(sections[1]), []);
    t.s.set_tool(ToolId::ArcBlock);
    let c = [0.0, -1_050.0];
    let path: Vec<_> = (1..=12)
        .map(|i| on_circle(c, 1_500.0, 120.0 - 5.0 * f64::from(i)))
        .collect();
    let circle = arc(
        &mut t,
        c,
        on_circle(c, 1_500.0, 120.0),
        &path,
        on_circle(c, 1_785.0, 90.0),
    );
    t.s.set_tool(ToolId::Select);
    t.s.select(Level::Section(sections[0]), [stalls]);
    let pivot = t.handle(HandleKind::Pivot);
    t.turn_by(pivot, 14.0, SHIFT);
    t.by(HandleKind::Side(Side::Bottom), [0.0, 95.0], NONE);
    let b = block(&t.node(stalls)).clone();
    assert_eq!((b.rows.len(), b.rows[0].seats, b.rotation), (7, 11, 15.0));
    assert_eq!(block(&t.node(circle)).rows.len(), 4);
    let doc = t.s.document().save();
    let compiled = compile(&doc).unwrap_or_else(|e| panic!("{e}"));
    assert!(compiled.stats.seats > 200, "{}", compiled.stats.seats);
}

fn tools() -> [ToolId; 10] {
    [
        ToolId::Select,
        ToolId::RowBlock,
        ToolId::ArcBlock,
        ToolId::Row,
        ToolId::Rect,
        ToolId::Ellipse,
        ToolId::Polygon,
        ToolId::Line,
        ToolId::Text,
        ToolId::Section,
    ]
}

/// Every id, `rid` and `oid` of a document.
fn ids(doc: &ChartDocument) -> std::collections::BTreeSet<u32> {
    let mut out = std::collections::BTreeSet::new();
    let text = serde_json::to_value(&doc.nodes).expect("json");
    fn walk(v: &serde_json::Value, out: &mut std::collections::BTreeSet<u32>) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, v) in m {
                    match (k.as_str(), v) {
                        ("id" | "rid", serde_json::Value::Number(n)) => {
                            out.extend(n.as_u64().map(|n| n as u32));
                        }
                        ("oids", serde_json::Value::Array(runs)) => {
                            for r in runs {
                                let (a, n) =
                                    (r[0].as_u64().unwrap_or(0), r[1].as_u64().unwrap_or(0));
                                out.extend((a..a + n).map(|v| v as u32));
                            }
                        }
                        _ => walk(v, out),
                    }
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    walk(&text, &mut out);
    out
}

/// One random step of a generated trace: mostly gestures (drags, clicks,
/// handle drags, typed numbers and text), some lone events.
fn event(t: &mut T, r: &mut Rng) {
    let mut at = || [r.f(1_500.0), r.f(1_500.0)];
    let (a, b) = (at(), at());
    let mods = Modifiers {
        shift: r.below(4) == 0,
        alt: r.below(8) == 0,
        ctrl: r.below(5) == 0,
    };
    match r.below(20) {
        0..=4 => {
            t.down(a, mods);
            for k in 1..=r.below(4) + 1 {
                let f = k as f64 / 4.0;
                t.to([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f], mods);
            }
            t.up(b, mods);
        }
        5..=7 => t.click(a),
        8 => t.to(a, mods),
        9 => {
            let keys = [
                Key::Escape,
                Key::Enter,
                Key::Backspace,
                Key::Char('x'),
                Key::Char('3'),
            ];
            for _ in 0..r.below(4) + 1 {
                t.key(r.pick(&keys).unwrap_or(Key::Enter));
            }
        }
        10 => t.s.set_text(["", "A", "STAGE", "Ünïcødé"][r.below(4)]),
        11 => t.s.set_tool(r.pick(&tools()).unwrap_or(ToolId::Select)),
        12 => drop(t.s.undo()),
        13 => {
            let ids: Vec<u32> = t.s.document().children(None).to_vec();
            let picked: Vec<u32> = (0..r.below(3) + 1).filter_map(|_| r.pick(&ids)).collect();
            t.s.select(Level::Top, picked);
        }
        14..=16 => {
            let handles = t.s.overlay().handles;
            if let Some(h) = r.pick(&handles) {
                let from = [h.at[0] + h.offset[0], h.at[1] + h.offset[1]];
                t.down(from, mods);
                t.to([(from[0] + b[0]) / 2.0, (from[1] + b[1]) / 2.0], mods);
                t.to(b, mods);
                t.up(b, mods);
            }
        }
        17 => t.down(a, mods),
        18 => t.up(a, mods),
        _ => t.key(Key::Enter),
    }
}

/// Generated traces with every tool, half with snapping: after each event
/// that leaves no gesture open, the document parses again; every undo entry the tools
/// make undoes and redoes byte for byte; ids are never reused; each node
/// compiles alone (no seat-less block, zero-size shape or bad arc can be
/// drawn); and switching tools never leaves a gesture open.
#[test]
fn generated_traces() {
    // Entries committed with each tool active, so every tool is exercised.
    let mut made = BTreeMap::<String, u32>::new();
    for seed in 0..90 {
        let mut r = Rng(seed + 100);
        let mut t = T::new(empty());
        let drafts = [straight(3, 5), fan([0.0, -1_200.0], 900.0, 2, 8)];
        for d in drafts.iter().take(seed as usize % 3) {
            t.s.insert(None, d).expect("insert");
        }
        let shapes = [
            rect([800.0, 0.0], [200.0, 100.0]),
            serde_json::json!({ "id": 0, "type": "shape", "kind": "line", "origin": [-800, 0],
                "points": [[0, 0], [200, 200]], "style": { "stroke": "#000000", "strokeWidth": 4 } }),
            serde_json::json!({ "id": 0, "type": "shape", "kind": "polygon", "origin": [0, 800],
                "points": [[0, 0], [300, 0], [0, 200]] }),
        ];
        for node in shapes.into_iter().take(seed as usize / 3 % 4) {
            add(&mut t.s, node);
        }
        t.s.set_tool(tools()[(seed as usize) % 10]);
        if seed % 2 == 1 {
            t.s.set_snap(SnapSettings::default());
        }
        let mut committed = t.state();
        let mut ever = ids(&t.s.document().snapshot());
        for _ in 0..60 {
            let entries = t.s.editor().undo_len();
            event(&mut t, &mut r);
            if t.s.editor().gesture_open() {
                continue;
            }
            let now = t.state();
            let doc = ChartDocument::from_json(&now).expect("parses");
            if t.s.editor().undo_len() > entries {
                let saved = t.s.document().save().next_oid;
                t.s.undo().expect("undo");
                assert_eq!(t.state(), committed, "seed {seed}: undo");
                t.s.redo().expect("redo");
                assert_eq!(t.state(), now, "seed {seed}: redo");
                assert_eq!(t.s.document().save().next_oid, saved);
                let old = ids(&ChartDocument::from_json(&committed).expect("old"));
                let new = ids(&doc);
                for id in new.difference(&old) {
                    assert!(!ever.contains(id), "seed {seed}: id {id} reused");
                }
                ever.extend(new);
                *made.entry(format!("{:?}", t.s.tool())).or_default() += 1;
                for node in &doc.nodes {
                    let one = ChartDocument {
                        nodes: vec![node.clone()],
                        ..empty()
                    };
                    // A section may hold two blocks with the same default
                    // labels (E001, as two blocks outside sections would).
                    let labels = |e: &cavea_compiler::CompileError| match e {
                        cavea_compiler::CompileError::Invalid(f) => {
                            f.iter().all(|f| f.code == cavea_compiler::Code::E001)
                        }
                        _ => false,
                    };
                    if let Err(e) = compile(&one)
                        && !labels(&e)
                    {
                        panic!(
                            "seed {seed}: {e}\n{}",
                            serde_json::to_string(node).unwrap_or_default()
                        );
                    }
                }
            }
            committed = now;
        }
        t.s.set_tool(ToolId::Select);
        assert!(!t.s.editor().gesture_open(), "seed {seed}");
    }
    assert_eq!(made.len(), tools().len(), "{made:?}");
}

/// Hostile input never panics and stays bounded: a second pointer
/// mid-drag, a tool switch between the arc tool's steps, Enter with nothing
/// typed, non-finite and huge coordinates, thousands of clicks.
#[test]
fn hostile_drawing_input() {
    let mut t = T::new(empty());
    t.s.set_tool(ToolId::RowBlock);
    t.down([0.0, 0.0], NONE);
    t.to([300.0, 300.0], NONE);
    t.down([10.0, 10.0], NONE);
    t.up([10.0, 10.0], NONE);
    t.up([10.0, 10.0], NONE);
    assert_eq!(
        t.s.document().ids().count(),
        0,
        "a second press abandons the drag"
    );
    // A drag across a huge canvas previews at most the live cap.
    t.down([0.0, 0.0], NONE);
    t.to([1e12, 1e12], NONE);
    let seats: u32 =
        t.s.document()
            .ids()
            .filter_map(|id| match t.s.document().node(id) {
                Some(Node::RowBlock(b)) => Some(b.rows.iter().map(|r| r.seats).sum::<u32>()),
                _ => None,
            })
            .sum();
    assert!(seats <= tuning::LIVE_SEATS);
    t.to([f64::NAN, 0.0], NONE);
    t.up([f64::INFINITY, 0.0], NONE);
    assert!(!t.s.editor().gesture_open());
    t.s.set_tool(ToolId::ArcBlock);
    t.click([0.0, 0.0]);
    t.key(Key::Enter);
    t.down([0.0, 1e300], NONE);
    t.to([1e300, 1e300], NONE);
    t.s.set_tool(ToolId::RowBlock);
    assert!(!t.s.editor().gesture_open());
    t.key(Key::Enter);
    t.key(Key::Backspace);
    // Thousands of clicks between the arc tool's steps.
    t.s.set_tool(ToolId::ArcBlock);
    let mut r = Rng(9);
    for _ in 0..3_000 {
        t.click([r.f(5_000.0), r.f(5_000.0)]);
    }
    t.s.set_tool(ToolId::Polygon);
    for _ in 0..10_000 {
        t.click([r.f(5_000.0), r.f(5_000.0)]);
    }
    assert!(t.s.overlay().preview.len() <= 4_097);
    t.key(Key::Enter);
    // The section tool's polygon, then its box over everything and moves,
    // snapping.
    t.s.set_tool(ToolId::Section);
    t.click([0.0, 0.0]);
    for _ in 0..5_000 {
        t.click([r.f(5_000.0), r.f(5_000.0)]);
    }
    assert!(t.s.overlay().preview.len() <= 4_097);
    t.key(Key::Enter);
    t.key(Key::Escape);
    t.s.set_snap(SnapSettings::default());
    t.drag([-1e300, -1e300], [1e300, 1e300], NONE);
    t.drag([0.0, 0.0], [f64::MAX, 1.0], NONE);
    t.s.set_tool(ToolId::Select);
    t.drag([0.0, 0.0], [1e300, -1e300], NONE);
    t.s.select_all();
    t.drag([0.0, 0.0], [f64::MAX, f64::MAX], NONE);
    t.s.set_tool(ToolId::Text);
    t.click([0.0, 0.0]);
    t.s.set_text(&"x".repeat(100_000));
    t.key(Key::Enter);
    t.s.set_tool(ToolId::Select);
    assert!(!t.s.editor().gesture_open());
    ChartDocument::from_json(&t.state()).expect("parses");
}

fn ms(t: std::time::Instant, n: u32) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0 / f64::from(n)
}

/// Prints the tools' per-event timings: run with
/// `cargo test --release -p cavea-edit measure_ -- --ignored --nocapture`.
#[test]
#[ignore]
fn measure_tools() {
    use std::time::Instant;
    let stadium = cavea_testkit::presets::find("stadium-100k")
        .expect("preset")
        .generate(1);
    // Dragging out a 50 × 100 block: each move changes its size.
    let mut t = T::new(stadium.clone());
    t.s.set_tool(ToolId::RowBlock);
    t.down([0.0, 0.0], NONE);
    let frames = 200;
    let clock = Instant::now();
    for i in 0..frames {
        let seats = if i % 2 == 0 { 100.0 } else { 99.0 };
        t.to([seats * 55.0, 50.0 * 95.0], NONE);
    }
    let drag = ms(clock, frames);
    t.up([100.0 * 55.0, 50.0 * 95.0], NONE);
    let big = t.selected();
    assert_eq!(block(&t.node(big)).rows.len(), 50);
    // A resize step adding a row to that 5,000-seat block.
    t.s.set_tool(ToolId::Select);
    t.s.select(Level::Top, [big]);
    let h = t.handle(HandleKind::Side(Side::Bottom));
    t.down(h, NONE);
    t.to([h[0], h[1] + 95.0], NONE);
    let clock = Instant::now();
    for i in 0..frames {
        let rows = if i % 2 == 0 { 1.0 } else { 2.0 };
        t.to([h[0], h[1] + rows * 95.0], NONE);
    }
    let resize = ms(clock, frames);
    t.up([h[0], h[1] + 95.0], NONE);
    // A seat more, and none, at the end of each of 20,000 rows: one pass.
    let tall = t.s.insert(None, &straight(20_000, 2)).expect("tall");
    let h = t.handle(HandleKind::Side(Side::Right));
    t.down(h, NONE);
    t.to([h[0] + 55.0, h[1]], NONE);
    let clock = Instant::now();
    for i in 0..20 {
        let seats = if i % 2 == 0 { 0.0 } else { 1.0 };
        t.to([h[0] + seats * 55.0, h[1]], NONE);
    }
    let wide = ms(clock, 20);
    t.up(h, NONE);
    assert_eq!(block(&t.node(tall)).rows.len(), 20_000);
    // Rotating 50 of the stadium's sections, a block each.
    let doc = t.s.document();
    let sections: Vec<u32> = doc
        .children(None)
        .iter()
        .copied()
        .filter(|&id| matches!(doc.node(id), Some(Node::Section(_))))
        .take(50)
        .collect();
    let seats: usize = sections
        .iter()
        .flat_map(|&id| doc.children(Some(id)).to_vec())
        .map(|b| t.s.index().seats(b).len())
        .sum();
    let count = sections.len();
    t.s.select(Level::Top, sections);
    let pivot = t.handle(HandleKind::Pivot);
    let h = t.handle(HandleKind::Rotate);
    t.down(h, NONE);
    t.to(turn(h, pivot, 1.0), NONE);
    let clock = Instant::now();
    for i in 0..frames {
        t.to(turn(h, pivot, f64::from(i % 90)), NONE);
    }
    let rotate = ms(clock, frames);
    t.up(h, NONE);
    println!(
        "stadium-100k: row block 50 × 100 drag move {drag:.3} ms; \
         resize step (row added to 5,000 seats) {resize:.3} ms; \
         seats added to 20,000 rows {wide:.3} ms; \
         rotate step ({count} sections, {seats} seats) {rotate:.3} ms"
    );
}

fn section(n: &Node) -> &cavea_compiler::document::Section {
    match n {
        Node::Section(s) => s,
        _ => panic!("not a section"),
    }
}

/// Sets a node's origin and rotation.
fn place(s: &mut Session, id: u32, origin: [f64; 2], rotation: f64) {
    let nodes = vec![NodeTransform {
        id,
        origin,
        rotation,
    }];
    s.apply(Command::TransformNodes { nodes })
        .expect("transform");
}

/// Row labels with a prefix, so a second block in a section repeats none.
fn distinct(s: &mut Session, id: u32) {
    let labels = cavea_compiler::document::RowLabels {
        prefix: "X".into(),
        ..Default::default()
    };
    let property = Property::RowLabels(Box::new(labels));
    s.apply(Command::SetProperty { id, property })
        .expect("labels");
}

/// Asserts every seat of `before` (by node, by oid) is where it was.
fn in_place(t: &T, before: &[(u32, BTreeMap<u32, [f64; 2]>)]) {
    for (id, seats) in before {
        assert_eq!(kept(seats, &t.seats(*id), 1e-9), seats.len(), "node {id}");
    }
}

fn world_seats(t: &T, ids: &[u32]) -> Vec<(u32, BTreeMap<u32, [f64; 2]>)> {
    ids.iter().map(|&id| (id, t.seats(id))).collect()
}

/// The section tool's box adopts the top-level nodes it contains, and
/// reparenting (the tool, `placeNode` both ways, dissolving) never moves a
/// seat, rotated nodes and a rotated section included; each is one undo
/// entry that undoes byte for byte.
#[test]
fn sections_keep_seats_in_place() {
    let mut t = T::new(empty());
    let a = t.s.insert(None, &straight(3, 4)).expect("a");
    let b =
        t.s.insert(None, &fan([1_000.0, -600.0], 700.0, 2, 6))
            .expect("b");
    distinct(&mut t.s, b);
    place(&mut t.s, a, [-400.0, 200.0], 33.0);
    let stage = add(&mut t.s, rect([0.0, -1_500.0], [600.0, 100.0]));
    let before = world_seats(&t, &[a, b]);
    let start = t.state();
    t.s.set_tool(ToolId::Section);
    t.drag([-3_000.0, -3_000.0], [3_000.0, 3_000.0], NONE);
    let s = t.selected();
    let sec = section(&t.node(s)).clone();
    assert_eq!(sec.label.as_deref(), Some("Section 1"));
    assert_eq!(
        (sec.outline.clone(), sec.category.as_deref()),
        (Outline::Auto, Some("std"))
    );
    assert_eq!(t.s.document().children(Some(s)), &[a, b, stage]);
    assert_eq!(t.s.document().children(None), &[s]);
    in_place(&t, &before);
    assert_eq!(t.s.overlay().sections.len(), 1);
    assert!(t.s.overlay().sections[0].len() >= 3);
    let grouped = t.state();
    t.s.undo().expect("undo");
    assert_eq!(t.state(), start);
    t.s.redo().expect("redo");
    assert_eq!(t.state(), grouped);
    // Turned with its children, then a turned block out and back in.
    place(&mut t.s, s, [100.0, 50.0], 40.0);
    let before = world_seats(&t, &[a, b]);
    for parent in [None, Some(s)] {
        let undone = t.state();
        t.s.apply(Command::PlaceNode {
            id: a,
            parent,
            index: 0,
        })
        .expect("place");
        assert_eq!(t.s.document().parent(a), parent);
        in_place(&t, &before);
        let done = t.state();
        t.s.undo().expect("undo");
        assert_eq!(t.state(), undone);
        t.s.redo().expect("redo");
        assert_eq!(t.state(), done);
    }
    // Dissolved: the children where the section was, in order, in place.
    let undone = t.state();
    t.s.dissolve(s).expect("dissolve");
    assert_eq!(t.s.document().children(None), &[a, b, stage]);
    in_place(&t, &before);
    t.s.undo().expect("undo");
    assert_eq!(t.state(), undone);
    // A box holding nothing it may take (only a section): an empty
    // section, its outline the box.
    t.drag([-3_000.0, -3_000.0], [3_000.0, 3_000.0], NONE);
    let empty = section(&t.node(t.selected())).clone();
    assert_eq!(empty.label.as_deref(), Some("Section 2"));
    assert!(empty.children.is_empty() && t.s.document().children(Some(t.selected())).is_empty());
    let corners = vec![[-3e3, -3e3], [3e3, -3e3], [3e3, 3e3], [-3e3, 3e3]];
    assert_eq!(empty.outline, Outline::Points(Points { points: corners }));
    // A click without a drag starts a polygon, not a section.
    t.click([5_000.0, 5_000.0]);
    t.key(Key::Escape);
    let doc = t.s.document().save();
    let compiled = compile(&doc).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(compiled.stats.sections, 2);
}

/// The section tool's polygon: its outline is the points clicked, in the
/// section's space; it adopts the top-level nodes inside it; a closing
/// click that would make the outline cross itself is refused.
#[test]
fn section_by_points() {
    let mut t = T::new(empty());
    let a = t.s.insert(None, &straight(2, 3)).expect("a");
    let far = add(&mut t.s, rect([2_000.0, 0.0], [100.0, 100.0]));
    let small = vec![[200.0, 200.0], [250.0, 200.0], [250.0, 250.0]];
    let inner = t.s.group(&[], Some(small)).expect("inner");
    t.s.set_tool(ToolId::Section);
    let before = t.state();
    let spiral = [
        [0.0, 0.0],
        [100.0, 0.0],
        [100.0, 100.0],
        [10.0, 100.0],
        [10.0, 30.0],
        [50.0, 30.0],
        [50.0, 70.0],
    ];
    spiral.iter().for_each(|&p| t.click(p));
    t.click([0.0, 0.0]);
    t.key(Key::Enter);
    assert_eq!(t.state(), before);
    t.key(Key::Escape);
    let square = [
        [-300.0, -300.0],
        [300.0, -300.0],
        [300.0, 400.0],
        [-300.0, 400.0],
    ];
    square.iter().for_each(|&p| t.click(p));
    t.click([-300.0, -299.0]);
    let s = t.selected();
    let sec = section(&t.node(s)).clone();
    assert_eq!(sec.origin, [0.0, 50.0]);
    let local: Vec<_> = square.iter().map(|p| [p[0], p[1] - 50.0]).collect();
    assert_eq!(sec.outline, Outline::Points(Points { points: local }));
    assert_eq!(t.s.document().children(Some(s)), &[a]);
    assert_eq!(t.s.document().parent(far), None);
    assert_eq!(t.s.document().children(None), &[s, far, inner]);
    assert_eq!(t.s.editor().undo_len(), 4);
    let compiled = compile(&t.s.document().save()).unwrap_or_else(|e| panic!("{e}"));
    let codes: Vec<_> = compiled.findings.iter().map(|f| f.code).collect();
    assert!(!codes.contains(&cavea_compiler::Code::W104), "{codes:?}");
}

/// Convert to polygon: the explicit points are the compiler's automatic
/// outline, so the compiled chart is the same, byte for byte; undo restores
/// `"auto"`; an explicit outline is not converted again.
#[test]
fn convert_to_polygon() {
    let mut t = T::new(empty());
    let a = t.s.insert(None, &straight(4, 6)).expect("a");
    let b =
        t.s.insert(None, &fan([0.0, -400.0], 900.0, 3, 9))
            .expect("b");
    distinct(&mut t.s, b);
    place(&mut t.s, a, [100.0, 300.0], 20.0);
    let s = t.s.group(&[a, b], None).expect("group");
    let auto = compile(&t.s.document().save()).expect("compiles");
    let shown = t.s.overlay().sections;
    let before = t.state();
    t.s.convert_outline(s).expect("convert");
    let sec = section(&t.node(s)).clone();
    let Outline::Points(p) = &sec.outline else {
        panic!("explicit");
    };
    let origin = sec.origin;
    let world: Vec<_> = p
        .points
        .iter()
        .map(|q| [q[0] + origin[0], q[1] + origin[1]])
        .collect();
    assert_eq!(vec![world], shown);
    let explicit = compile(&t.s.document().save()).expect("compiles");
    assert_eq!(explicit.public, auto.public);
    assert!(t.s.convert_outline(s).is_err());
    t.s.undo().expect("undo");
    assert_eq!(t.state(), before);
}

const ALT: Modifiers = Modifiers { alt: true, ..NONE };

fn snapped(doc: ChartDocument) -> T {
    let mut t = T::new(doc);
    t.s.set_snap(SnapSettings::default());
    t
}

/// Drags node `id` (selected first) from `from` by `d`, looking at the
/// overlay before the release; returns the guides shown then.
fn move_by(t: &mut T, id: u32, from: [f64; 2], d: [f64; 2], mods: Modifiers) -> Vec<Guide> {
    t.s.set_tool(ToolId::Select);
    t.s.select(Level::Top, [id]);
    let to = [from[0] + d[0], from[1] + d[1]];
    t.down(from, mods);
    t.to([from[0] + d[0] / 2.0, from[1] + d[1] / 2.0], mods);
    t.to(to, mods);
    let guides = t.s.overlay().guides;
    t.up(to, mods);
    assert!(t.s.overlay().guides.is_empty(), "guides end with the drag");
    guides
}

fn shape_origin(t: &T, id: u32) -> [f64; 2] {
    shape(&t.node(id)).origin
}

/// Each kind of snap: the moved node lands exactly on the target, with its
/// guide; edges and centres win over a nearer grid line; the target held
/// stays while it is in reach, so a jittering pointer does not flicker; Alt
/// and each setting turn it off.
#[test]
fn snapping() {
    let mut t = snapped(empty());
    // The grid: a rectangle drawn from (2, 3) to (202, 104).
    t.s.set_tool(ToolId::Rect);
    t.drag([2.0, 3.0], [202.0, 104.0], NONE);
    let a = t.selected();
    assert_eq!(shape(&t.node(a)).size, Some([200.0, 100.0]));
    assert_eq!(shape_origin(&t, a), [100.0, 55.0]);
    // Edges: B's left edge 4 from A's right edge lands on it, though the
    // grid (205) is nearer; y goes to the grid (13 → 15).
    let b = add(&mut t.s, rect([400.0, 300.0], [100.0, 100.0]));
    let guides = move_by(&mut t, b, [380.0, 280.0], [-146.0, -237.0], NONE);
    assert_eq!(shape_origin(&t, b), [250.0, 65.0]);
    let line = Guide::Line {
        from: [200.0, 5.0],
        to: [200.0, 115.0],
    };
    assert_eq!(guides, vec![line]);
    t.s.undo().expect("undo");
    // Alt: no snapping; objects off: the grid.
    move_by(&mut t, b, [380.0, 280.0], [-146.0, -237.0], ALT);
    assert_eq!(shape_origin(&t, b), [254.0, 63.0]);
    t.s.undo().expect("undo");
    let all = SnapSettings::default();
    t.s.set_snap(SnapSettings {
        objects: false,
        ..all
    });
    move_by(&mut t, b, [380.0, 280.0], [-146.0, -237.0], NONE);
    assert_eq!(shape_origin(&t, b), [255.0, 65.0]);
    t.s.undo().expect("undo");
    t.s.set_snap(all);
    // No flicker: C's left edge (203) and A's right edge (200) both in
    // reach; the one held first stays while it is.
    add(&mut t.s, rect([253.0, -500.0], [100.0, 100.0]));
    t.s.select(Level::Top, [b]);
    t.down([380.0, 280.0], NONE);
    let mut lefts = Vec::new();
    for x in [-148.6, -148.4, -148.6, -148.4, -142.0] {
        t.to([380.0 + x, 0.0], NONE);
        lefts.push(shape(&t.node(b)).origin[0] - 50.0);
    }
    t.up([258.0, 0.0], NONE);
    assert_eq!(lefts, [200.0, 200.0, 200.0, 200.0, 203.0]);
    t.s.undo().expect("undo");
}

/// Equal spacing: dragged beside two nodes 100 apart, a third keeps the
/// same gap, with two markers; off, it goes to the grid.
#[test]
fn equal_spacing() {
    let mut t = snapped(empty());
    for x in [50.0, 250.0] {
        add(&mut t.s, rect([x, 1_050.0], [100.0, 100.0]));
    }
    let r = add(&mut t.s, rect([1_000.0, 1_050.0], [100.0, 100.0]));
    let guides = move_by(&mut t, r, [980.0, 1_030.0], [-547.0, 0.0], NONE);
    assert_eq!(shape_origin(&t, r), [450.0, 1_050.0]);
    let gap = |from: [f64; 2], to: [f64; 2]| Guide::Spacing {
        from,
        to,
        distance: 100.0,
    };
    let markers: Vec<_> = guides
        .into_iter()
        .filter(|g| matches!(g, Guide::Spacing { .. }))
        .collect();
    let y = 1_050.0;
    assert_eq!(
        markers,
        [gap([100.0, y], [200.0, y]), gap([300.0, y], [400.0, y])]
    );
    t.s.undo().expect("undo");
    let all = SnapSettings::default();
    t.s.set_snap(SnapSettings {
        spacing: false,
        ..all
    });
    move_by(&mut t, r, [980.0, 1_030.0], [-547.0, 0.0], NONE);
    assert_eq!(shape_origin(&t, r), [455.0, 1_050.0]);
}

/// Row alignment: a block dragged near a neighbour turned the same way
/// lands with its front row on one of the neighbour's row lines, along the
/// neighbour's own axis, with a dashed extension; off, it does not.
#[test]
fn row_alignment() {
    let mut t = snapped(empty());
    let n = t.s.insert(None, &straight(6, 10)).expect("n");
    let m = t.s.insert(None, &straight(4, 6)).expect("m");
    let axes = |deg: f64| {
        let (s, c) = f64::to_radians(deg).sin_cos();
        ([c, s], [-s, c])
    };
    let (u, nrm) = axes(30.0);
    let at = |x: f64, y: f64| [x * u[0] + y * nrm[0], x * u[1] + y * nrm[1]];
    place(&mut t.s, n, [0.0, 0.0], 30.0);
    // 230 along the normal: 40 past row line 2.
    place(&mut t.s, m, at(900.0, 230.0), 30.0);
    let from = t.s.index().to_world(m, [0.0, 0.0]).expect("origin");
    let guides = move_by(&mut t, m, from, at(0.0, -37.0), NONE);
    let o = block(&t.node(m)).origin;
    let y = o[0] * nrm[0] + o[1] * nrm[1];
    assert!((y - 190.0).abs() < 1e-9, "{y}");
    let x = o[0] * u[0] + o[1] * u[1];
    assert!((x - 900.0).abs() < 1e-9, "{x}");
    assert!(guides.iter().any(|g| matches!(g, Guide::Row { .. })));
    t.s.undo().expect("undo");
    let all = SnapSettings::default();
    t.s.set_snap(SnapSettings { rows: false, ..all });
    move_by(&mut t, m, from, at(0.0, -37.0), NONE);
    let o = block(&t.node(m)).origin;
    assert!(((o[0] * nrm[0] + o[1] * nrm[1]) - 190.0).abs() > 1.0);
}

/// Angles: a turn snaps to 15° steps, and a block to the angle facing the
/// floor's focal point first; a line's end to 15° directions.
#[test]
fn angle_snapping() {
    let mut t = snapped(empty());
    let b = t.s.insert(None, &straight(2, 4)).expect("b");
    place(&mut t.s, b, [1_000.0, 0.0], 0.0);
    t.s.select(Level::Top, [b]);
    let pivot = t.handle(HandleKind::Pivot);
    t.turn_by(pivot, 13.0, NONE);
    assert_eq!(block(&t.node(b)).rotation, 15.0);
    t.s.undo().expect("undo");
    // Facing (0, −1000) from the pivot: its front (−y) points there.
    let (dx, dy) = (0.0 - pivot[0], -1_000.0 - pivot[1]);
    let facing = dx.atan2(-dy).to_degrees();
    let h = t.handle(HandleKind::Rotate);
    t.down(h, NONE);
    t.to(turn(h, pivot, facing / 2.0), NONE);
    t.to(turn(h, pivot, facing - 1.0), NONE);
    let guides = t.s.overlay().guides;
    t.up(turn(h, pivot, facing - 1.0), NONE);
    assert!((block(&t.node(b)).rotation - facing).abs() < 1e-9);
    assert!(matches!(guides[..], [Guide::Angle { .. }]));
    // A line at 14.6° ends at 15° (in reach: 6 cu at 500 is 0.69°).
    t.s.set_tool(ToolId::Line);
    let r = 500.0;
    let (s, c) = 14.6f64.to_radians().sin_cos();
    t.drag([0.0, 5_000.0], [r * c, 5_000.0 + r * s], NONE);
    let end = shape(&t.node(t.selected())).points.clone().expect("points")[1];
    let (s, c) = 15f64.to_radians().sin_cos();
    assert!(close(end, [r * c, r * s], 1e-9), "{end:?}");
}

/// A small theatre drawn with snapping on: a stage, its label centred on it
/// by snapping, two blocks whose rows line up by snapping, each grouped
/// into a section by the section tool. It compiles with no errors and no
/// unlabelled section.
#[test]
fn draw_a_theatre_with_sections() {
    let mut t = snapped(empty());
    t.s.set_tool(ToolId::Rect);
    t.drag([-302.0, -1_101.0], [298.0, -1_002.0], NONE);
    let stage = t.selected();
    assert_eq!(shape_origin(&t, stage), [0.0, -1_050.0]);
    t.s.set_tool(ToolId::Text);
    t.click([3.0, -1_047.0]);
    t.s.set_text("STAGE");
    t.key(Key::Enter);
    assert_eq!(text_origin(&t.node(t.selected())), [0.0, -1_050.0]);
    t.s.set_tool(ToolId::RowBlock);
    t.drag([-700.0, -700.0], [-100.0, -300.0], NONE);
    let left = t.selected();
    t.drag([100.0, -650.0], [700.0, -250.0], NONE);
    let right = t.selected();
    // Moved up by 47: its front row onto the left block's (3 cu away).
    let from = block(&t.node(right)).origin;
    move_by(&mut t, right, from, [0.0, -47.0], NONE);
    assert_eq!(
        block(&t.node(right)).origin[1],
        block(&t.node(left)).origin[1]
    );
    t.s.set_tool(ToolId::Section);
    t.drag([-800.0, -800.0], [-50.0, 0.0], NONE);
    t.drag([50.0, -800.0], [800.0, 0.0], NONE);
    let doc = t.s.document().save();
    let compiled = compile(&doc).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((compiled.stats.sections, compiled.stats.seats), (2, 88));
    let codes: Vec<_> = compiled.findings.iter().map(|f| f.code).collect();
    assert!(!codes.contains(&cavea_compiler::Code::W109), "{codes:?}");
}

fn text_origin(n: &Node) -> [f64; 2] {
    match n {
        Node::Text(x) => x.origin,
        _ => panic!("text"),
    }
}

/// Prints snapping and section timings on `stadium-100k`: run with
/// `cargo test --release -p cavea-edit measure_ -- --ignored --nocapture`.
#[test]
#[ignore]
fn measure_snapping_and_sections() {
    use std::time::Instant;
    let stadium = cavea_testkit::presets::find("stadium-100k")
        .expect("preset")
        .generate(1);
    let mut t = snapped(stadium);
    let doc = t.s.document();
    let sections: Vec<u32> = doc
        .children(None)
        .iter()
        .copied()
        .filter(|&id| matches!(doc.node(id), Some(Node::Section(_))))
        .take(50)
        .collect();
    let first = doc.children(Some(sections[0]))[0];
    let seat = t.s.index().seats(first)[0].pos;
    let at = t.s.index().to_world(first, seat).expect("seat");
    let frames = 200;
    let drag = |t: &mut T| {
        t.down(at, NONE);
        t.to([at[0] + 20.0 / t.zoom, at[1]], NONE);
        let clock = Instant::now();
        for i in 0..frames {
            let d = f64::from(i % 40) / t.zoom;
            t.to([at[0] + d, at[1] + d / 2.0], NONE);
        }
        let per = ms(clock, frames);
        assert!(t.s.editor().gesture_open(), "dragging");
        t.key(Key::Escape);
        t.up(at, NONE);
        per
    };
    let mut lines = Vec::new();
    // Zoom 1 (snapping looks 1,000 cu around), and the whole stadium on a
    // 1,000 px canvas (it looks at everything).
    for zoom in [1.0, 0.01] {
        t.zoom = zoom;
        t.s.select(Level::Section(sections[0]), [first]);
        let one = drag(&mut t);
        t.s.select(Level::Top, sections.clone());
        let fifty = drag(&mut t);
        lines.push(format!(
            "zoom {zoom}: snapped move dragging 1 block {one:.3} ms, 50 sections {fifty:.3} ms"
        ));
    }
    // 50 blocks out of their sections, then a section around them.
    let blocks: Vec<u32> = sections
        .iter()
        .map(|&s| t.s.document().children(Some(s))[0])
        .collect();
    for &s in &sections {
        t.s.dissolve(s).expect("dissolve");
    }
    let clock = Instant::now();
    t.s.group(&blocks, None).expect("group");
    let group = ms(clock, 1);
    let clock = Instant::now();
    t.s.undo().expect("undo");
    let undo = ms(clock, 1);
    // A 5,000-seat section: its automatic outline shown, then converted.
    let big = t.s.insert(None, &straight(50, 100)).expect("big");
    let s = t.s.group(&[big], None).expect("group");
    let clock = Instant::now();
    let shown = t.s.overlay().sections;
    let overlay = ms(clock, 1);
    let clock = Instant::now();
    t.s.convert_outline(s).expect("convert");
    let convert = ms(clock, 1);
    println!(
        "stadium-100k: {}; section around 50 blocks {group:.3} ms, undo {undo:.3} ms; \
         5,000-seat section: overlay with its outline ({} points) {overlay:.3} ms, \
         convert to polygon {convert:.3} ms",
        lines.join("; "),
        shown[0].len()
    );
}
