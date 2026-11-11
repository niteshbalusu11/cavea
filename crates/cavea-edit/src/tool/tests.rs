//! Scripted pointer and key traces (13 §9): each produces an expected
//! selection and document.

use cavea_compiler::ChartDocument;
use cavea_compiler::document::Node;

use super::draw_tests::unsnapped;
use crate::tests::{Rng, command};
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
const ALT: Modifiers = Modifiers { alt: true, ..NONE };
const CTRL: Modifiers = Modifiers { ctrl: true, ..NONE };

/// A block of 3 rows × 4 seats (10) at the origin; a section (20) holding a
/// booth (21) and a one-row block (22); a rectangle turned 30° (30); a locked
/// (40) and a hidden (41) booth; a booth on the second floor (50) drawn over
/// the block. Zoom 1, so screen pixels are chart units.
fn chart() -> ChartDocument {
    serde_json::from_value(serde_json::json!({
        "schema": 1, "name": "t", "nextOid": 200,
        "settings": { "seatSize": 40 },
        "floors": [{ "id": 1, "name": "Stalls" }, { "id": 2, "name": "Circle" }],
        "nodes": [
            { "id": 10, "type": "rowBlock", "seatPitch": 50, "rowPitch": 100, "align": "left",
              "rows": [{ "rid": 11, "seats": 4, "oids": [[100, 4]] },
                       { "rid": 12, "seats": 4, "oids": [[104, 4]] },
                       { "rid": 13, "seats": 4, "oids": [[108, 4]] }] },
            { "id": 20, "type": "section", "origin": [1000, 0], "children": [
                { "id": 21, "type": "booth", "size": [100, 100], "oid": 24 },
                { "id": 22, "type": "rowBlock", "origin": [0, 300], "seatPitch": 50,
                  "align": "left", "rows": [{ "rid": 23, "seats": 3, "oids": [[120, 3]] }] }
            ] },
            { "id": 30, "type": "shape", "kind": "rect", "size": [200, 100],
              "origin": [0, 800], "rotation": 30 },
            { "id": 40, "type": "booth", "size": [100, 100], "oid": 42, "origin": [500, 800],
              "locked": true },
            { "id": 41, "type": "booth", "size": [100, 100], "oid": 43, "origin": [700, 800],
              "hidden": true },
            { "id": 50, "type": "booth", "size": [100, 100], "oid": 51, "floor": 2 }
        ]
    }))
    .expect("chart")
}

struct Trace {
    s: Session,
    time: f64,
}

impl Trace {
    fn new() -> Self {
        Self {
            s: Session::load(chart()).map(unsnapped).expect("load"),
            time: 0.0,
        }
    }

    fn pointer(&self, x: f64, y: f64, mods: Modifiers) -> Pointer {
        Pointer {
            world: [x, y],
            screen: [x, y],
            zoom: 1.0,
            button: Button::Primary,
            kind: PointerKind::Mouse,
            mods,
            time: self.time,
        }
    }

    fn down(&mut self, x: f64, y: f64, mods: Modifiers) {
        // Far from the last click unless a double click says otherwise.
        self.time += 1_000.0;
        self.s.input(Input::PointerDown(self.pointer(x, y, mods)));
    }

    fn to(&mut self, x: f64, y: f64, mods: Modifiers) {
        self.time += 10.0;
        self.s.input(Input::PointerMove(self.pointer(x, y, mods)));
    }

    fn up(&mut self, x: f64, y: f64, mods: Modifiers) {
        self.time += 10.0;
        self.s.input(Input::PointerUp(self.pointer(x, y, mods)));
    }

    fn click(&mut self, x: f64, y: f64, mods: Modifiers) {
        self.down(x, y, mods);
        self.up(x, y, mods);
    }

    fn double(&mut self, x: f64, y: f64) {
        self.click(x, y, NONE);
        self.time -= 900.0;
        self.click(x, y, NONE);
    }

    fn drag(&mut self, from: [f64; 2], to: [f64; 2], mods: Modifiers) {
        self.down(from[0], from[1], mods);
        self.to((from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0, mods);
        self.to(to[0], to[1], mods);
        self.up(to[0], to[1], mods);
    }

    fn key(&mut self, key: Key, mods: Modifiers) {
        self.s.input(Input::KeyDown(key, mods));
        self.s.input(Input::KeyUp(key, mods));
    }

    fn sel(&self) -> (Level, Vec<u32>) {
        let s = self.s.selection();
        (s.level(), s.items().iter().copied().collect())
    }

    fn state(&self) -> Vec<u8> {
        self.s.document().snapshot().to_json().expect("json")
    }

    fn origin(&self, id: u32) -> [f64; 2] {
        match self.s.document().node(id) {
            Some(Node::RowBlock(n)) => n.origin,
            Some(Node::Booth(n)) => n.origin,
            _ => [f64::NAN; 2],
        }
    }
}

#[test]
fn click_selection() {
    let mut t = Trace::new();
    t.click(50.0, 100.0, NONE);
    assert_eq!(t.sel(), (Level::Top, vec![10]));
    t.click(0.0, 800.0, SHIFT);
    assert_eq!(t.sel(), (Level::Top, vec![10, 30]));
    // Between four seats is on the block; shift-click toggles it off.
    t.click(25.0, 50.0, SHIFT);
    assert_eq!(t.sel(), (Level::Top, vec![30]));
    t.click(3_000.0, 3_000.0, NONE);
    assert_eq!(t.sel(), (Level::Top, vec![]));
    // A press that moves less than the drag threshold is a click; the booth
    // on the other floor, drawn over the block, is not picked.
    t.down(0.0, 0.0, NONE);
    t.to(3.0, 0.0, NONE);
    t.up(3.0, 0.0, NONE);
    assert_eq!(t.sel(), (Level::Top, vec![10]));
    // A click on a selected node among several selects it alone.
    t.click(0.0, 800.0, SHIFT);
    t.click(0.0, 800.0, NONE);
    assert_eq!(t.sel(), (Level::Top, vec![30]));
    // Locked and hidden nodes are empty canvas; so is the corner of the
    // turned rectangle's bounds.
    for p in [[500.0, 800.0], [700.0, 800.0], [105.0, 715.0]] {
        t.click(30.0, 0.0, NONE);
        t.click(p[0], p[1], NONE);
        assert_eq!(t.sel(), (Level::Top, vec![]), "{p:?}");
    }
}

#[test]
fn box_selection() {
    let mut t = Trace::new();
    // Live while dragging, and drawn.
    t.down(-60.0, -60.0, NONE);
    t.to(200.0, 250.0, NONE);
    assert_eq!(t.sel(), (Level::Top, vec![10]));
    assert_eq!(
        t.s.overlay().marquee,
        Some(Rect::new([-60.0, -60.0], [200.0, 250.0]))
    );
    assert_eq!(t.s.cursor(), Cursor::Crosshair);
    t.up(200.0, 250.0, NONE);
    assert_eq!(t.s.overlay().marquee, None);
    // Contained, or with Alt touched.
    t.drag([-60.0, -60.0], [60.0, 60.0], NONE);
    assert_eq!(t.sel(), (Level::Top, vec![]));
    t.drag([-60.0, -60.0], [60.0, 60.0], ALT);
    assert_eq!(t.sel(), (Level::Top, vec![10]));
    // Shift adds.
    t.drag([-150.0, 650.0], [150.0, 950.0], SHIFT);
    assert_eq!(t.sel(), (Level::Top, vec![10, 30]));
    // Escape restores the selection from before the box.
    t.down(2_000.0, 2_000.0, NONE);
    t.to(-500.0, -500.0, NONE);
    assert_eq!(t.sel(), (Level::Top, vec![10, 20, 30]));
    t.key(Key::Escape, NONE);
    assert_eq!(t.sel(), (Level::Top, vec![10, 30]));
    t.up(-500.0, -500.0, NONE);
    assert_eq!(t.sel(), (Level::Top, vec![10, 30]));
    // Seats of an entered block.
    t.s.select(Level::Seats(10), []);
    t.drag([-30.0, 70.0], [80.0, 130.0], NONE);
    assert_eq!(t.sel(), (Level::Seats(10), vec![104, 105]));
    assert_eq!(t.s.overlay().outlines.len(), 2);
}

#[test]
fn levels() {
    let mut t = Trace::new();
    // A node in a section selects the section; a double click enters it.
    t.click(1_000.0, 0.0, NONE);
    assert_eq!(t.sel(), (Level::Top, vec![20]));
    t.double(1_000.0, 0.0);
    assert_eq!(t.sel(), (Level::Section(20), vec![21]));
    t.double(1_050.0, 300.0);
    assert_eq!(t.sel(), (Level::Rows(22), vec![23]));
    t.double(1_100.0, 300.0);
    assert_eq!(t.sel(), (Level::Seats(22), vec![122]));
    t.click(1_000.0, 300.0, SHIFT);
    assert_eq!(t.sel(), (Level::Seats(22), vec![120, 122]));
    // A gap in the block keeps seat-edit mode.
    t.click(1_025.0, 330.0, NONE);
    assert_eq!(t.sel(), (Level::Seats(22), vec![]));
    t.click(1_000.0, 300.0, NONE);
    for expected in [
        (Level::Rows(22), vec![23]),
        (Level::Section(20), vec![22]),
        (Level::Top, vec![20]),
        (Level::Top, vec![]),
    ] {
        t.key(Key::Escape, NONE);
        assert_eq!(t.sel(), expected);
    }
    // Cancelling a press that stepped out restores the level too.
    t.s.select(Level::Section(20), [21]);
    t.down(50.0, 100.0, NONE);
    assert_eq!(t.sel(), (Level::Top, vec![10]));
    t.s.input(Input::PointerCancel);
    assert_eq!(t.sel(), (Level::Section(20), vec![21]));
    // A section is hit wherever its children are: 40 above the block's seat.
    t.key(Key::Escape, NONE);
    t.click(1_050.0, 260.0, NONE);
    assert_eq!(t.sel(), (Level::Top, vec![20]));
    // A click outside the level lands where it hits.
    t.s.select(Level::Section(20), [21]);
    t.click(50.0, 100.0, NONE);
    assert_eq!(t.sel(), (Level::Top, vec![10]));
    // Keyboard: Enter steps in with everything selected; Ctrl+A selects all
    // that can be picked.
    t.s.select(Level::Top, [20]);
    t.key(Key::Enter, NONE);
    assert_eq!(t.sel(), (Level::Section(20), vec![21, 22]));
    t.key(Key::Escape, NONE);
    t.key(Key::Char('a'), CTRL);
    assert_eq!(t.sel(), (Level::Top, vec![10, 20, 30]));
    assert!(t.s.set_floor(2));
    t.key(Key::Char('a'), CTRL);
    assert_eq!(t.sel(), (Level::Top, vec![50]));
    // Deleting the entered section leaves the selection at the top.
    assert!(t.s.set_floor(1));
    t.s.select(Level::Section(20), [21]);
    t.s.apply(Command::DeleteNodes { ids: vec![20] })
        .expect("delete");
    assert_eq!(t.sel(), (Level::Top, vec![]));
    t.s.undo().expect("undo");
    assert_eq!(t.sel(), (Level::Top, vec![]));
}

#[test]
fn drag_move_and_nudge() {
    let mut t = Trace::new();
    t.click(50.0, 100.0, NONE);
    let (start, entries) = (t.state(), t.s.editor().undo_len());
    t.drag([50.0, 100.0], [80.0, 140.0], NONE);
    assert_eq!(t.origin(10), [30.0, 40.0]);
    assert_eq!(t.s.editor().undo_len(), entries + 1);
    let moved = t.state();
    t.s.undo().expect("undo");
    assert_eq!(t.state(), start);
    t.s.redo().expect("redo");
    assert_eq!(t.state(), moved);
    t.s.undo().expect("undo");
    // Escape mid-drag puts it back and leaves no entry.
    t.down(50.0, 100.0, NONE);
    t.to(90.0, 100.0, NONE);
    assert_eq!(t.s.cursor(), Cursor::Move);
    t.to(120.0, 130.0, NONE);
    assert_ne!(t.state(), start);
    t.key(Key::Escape, NONE);
    t.up(120.0, 130.0, NONE);
    assert_eq!(t.state(), start);
    assert_eq!(t.s.editor().undo_len(), entries);
    // The release is the drag's last position.
    t.down(50.0, 100.0, NONE);
    t.to(60.0, 100.0, NONE);
    t.up(100.0, 100.0, NONE);
    assert_eq!(t.origin(10), [50.0, 0.0]);
    t.s.undo().expect("undo");
    // A press on an unselected node drags it alone.
    t.drag([0.0, 800.0], [10.0, 800.0], NONE);
    assert_eq!(t.sel(), (Level::Top, vec![30]));
    assert_eq!(t.origin(10), [0.0, 0.0]);
    // Nudges: one entry each.
    t.click(50.0, 100.0, NONE);
    let entries = t.s.editor().undo_len();
    t.key(Key::ArrowRight, NONE);
    t.key(Key::ArrowDown, SHIFT);
    assert_eq!(t.origin(10), [1.0, 10.0]);
    assert_eq!(t.s.editor().undo_len(), entries + 2);
    // In a turned section, a node follows the pointer on screen.
    t.s.apply(Command::TransformNodes {
        nodes: vec![NodeTransform {
            id: 20,
            origin: [1_000.0, 0.0],
            rotation: 90.0,
        }],
    })
    .expect("turn");
    t.s.select(Level::Section(20), [21]);
    let before = t.s.index().bounds(21).expect("bounds");
    // (Off its centre, where a drag takes the pivot.)
    t.drag([1_000.0, 20.0], [1_010.0, 20.0], NONE);
    let after = t.s.index().bounds(21).expect("bounds");
    assert!((after.min[0] - before.min[0] - 10.0).abs() < 1e-9);
    assert!((after.min[1] - before.min[1]).abs() < 1e-9);
    // Locked nodes in the selection stay put.
    t.s.select(Level::Top, [10, 40]);
    t.key(Key::ArrowLeft, NONE);
    assert_eq!(t.origin(10), [0.0, 10.0]);
    assert!(matches!(t.s.document().node(40), Some(Node::Booth(b)) if b.origin == [500.0, 800.0]));
}

/// A tool switch mid-drag cancels it; a hostile trace (no press before a
/// release, second presses, non-finite numbers, keys held, commands from
/// outside) never panics, and a release or cancel always leaves no gesture
/// open.
#[test]
fn hostile_input() {
    let mut t = Trace::new();
    t.click(50.0, 100.0, NONE);
    let start = t.state();
    t.down(50.0, 100.0, NONE);
    t.to(90.0, 100.0, NONE);
    assert!(t.s.editor().gesture_open());
    t.s.set_tool(ToolId::Select);
    assert!(!t.s.editor().gesture_open());
    assert_eq!(t.state(), start);
    t.up(90.0, 100.0, NONE);
    assert_eq!(t.state(), start);

    let mut r = Rng(42);
    let (mut dragging, mut selected) = (0, 0);
    let numbers = [f64::NAN, f64::INFINITY, -f64::INFINITY, 1e308, -1e308, 0.0];
    for _ in 0..20_000 {
        let v = |r: &mut Rng| {
            if r.below(20) == 0 {
                numbers[r.below(numbers.len())]
            } else {
                r.f(1_200.0) + 500.0
            }
        };
        // Half the time near a node, so that presses hit and drags move.
        let near = [
            [50.0, 100.0],
            [1_000.0, 0.0],
            [0.0, 800.0],
            [1_050.0, 300.0],
        ];
        let world = match r.below(2) {
            0 => [v(&mut r), v(&mut r)],
            _ => near[r.below(near.len())].map(|c| c + r.f(30.0)),
        };
        let p = Pointer {
            world,
            screen: if r.below(10) == 0 {
                [v(&mut r), v(&mut r)]
            } else {
                world
            },
            zoom: if r.below(20) == 0 { v(&mut r) } else { 1.0 },
            button: [Button::Primary, Button::Primary, Button::Middle][r.below(3)],
            kind: [PointerKind::Mouse, PointerKind::Touch, PointerKind::Pen][r.below(3)],
            mods: Modifiers {
                shift: r.below(3) == 0,
                alt: r.below(3) == 0,
                ctrl: r.below(5) == 0,
            },
            time: t.time + r.f(400.0),
        };
        t.time += 50.0;
        let keys = [
            Key::Escape,
            Key::Enter,
            Key::ArrowLeft,
            Key::ArrowUp,
            Key::Char('a'),
        ];
        let input = match r.below(14) {
            0..=1 => Input::PointerDown(p),
            2..=8 => Input::PointerMove(p),
            9 => Input::PointerUp(p),
            10 => Input::PointerCancel,
            11 => Input::KeyDown(keys[r.below(keys.len())], p.mods),
            12 => Input::KeyUp(keys[r.below(keys.len())], p.mods),
            _ => {
                match r.below(6) {
                    // Removing floor 1 would leave only node 50 to pick.
                    0 => match command(&mut r, t.s.document()) {
                        Command::RemoveFloor { .. } => {}
                        c => drop(t.s.apply(c)),
                    },
                    1 => drop(t.s.undo()),
                    2 => drop(t.s.redo()),
                    3 => t.s.select(Level::Seats(10), [100, 101, 999]),
                    4 => drop(t.s.set_floor(r.below(3) as u32)),
                    _ => drop(t.s.step_in()),
                }
                continue;
            }
        };
        t.s.input(input);
        let _ = t.s.overlay();
        if matches!(input, Input::PointerUp(_) | Input::PointerCancel) {
            assert!(!t.s.editor().gesture_open(), "{input:?}");
        }
        dragging += usize::from(t.s.editor().gesture_open());
        selected += usize::from(!t.s.selection().items().is_empty());
    }
    // The trace reaches drags and selections, not just empty canvas.
    assert!(dragging > 20 && selected > 2_000, "{dragging} {selected}");
    t.s.input(Input::PointerCancel);
    assert!(!t.s.editor().gesture_open());
}

/// Selecting every row or seat of a block of 100,000 rows, then moving the
/// pointer, costs time in proportion to the block, not its square; the
/// block's seats are found although its letters run past 64 bytes.
#[test]
fn large_block_selection() {
    let rows: Vec<_> = (0..100_000u32)
        .map(|i| serde_json::json!({ "rid": 1 + 2 * i, "seats": 1, "oids": [[2 + 2 * i, 1]] }))
        .collect();
    let doc = serde_json::from_value(serde_json::json!({
        "schema": 1, "name": "big", "nextOid": 300_000,
        "nodes": [{ "id": 0, "type": "rowBlock", "rows": rows }]
    }))
    .expect("doc");
    let mut t = Trace {
        s: Session::load(doc).expect("load"),
        time: 0.0,
    };
    for level in [Level::Rows(0), Level::Seats(0)] {
        t.s.select(level, []);
        t.key(Key::Char('a'), CTRL);
        assert_eq!(t.s.selection().items().len(), 100_000);
        for i in 0..50 {
            t.to(f64::from(i), 0.0, NONE);
        }
        t.s.apply(Command::RemoveRows {
            id: 0,
            rids: vec![1, 3],
        })
        .expect("remove");
        assert_eq!(t.s.selection().items().len(), 99_998);
        t.s.undo().expect("undo");
    }
}

fn ms(t: std::time::Instant, n: u32) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0 / f64::from(n)
}

/// Prints the select tool's costs on `stadium-100k`, input to updated index
/// and selection, before any drawing: run with
/// `cargo test --release -p cavea-edit measure_ -- --ignored --nocapture`.
#[test]
#[ignore]
fn measure_tools() {
    use std::time::Instant;
    let doc = cavea_testkit::presets::find("stadium-100k")
        .expect("preset")
        .generate(1);
    let mut t = Trace {
        s: Session::load(doc).expect("load"),
        time: 0.0,
    };
    // A point on each top-level section: the first seat of its block.
    let doc = t.s.document();
    let seat_of = |id: u32| {
        let block = *doc.children(Some(id)).first()?;
        let seat = t.s.index().seats(block).first()?;
        t.s.index().to_world(block, seat.pos)
    };
    let points: Vec<(u32, [f64; 2])> = doc
        .children(None)
        .iter()
        .filter_map(|&id| seat_of(id).map(|p| (id, p)))
        .collect();
    let n = 1_000;
    let start = Instant::now();
    for i in 0..n {
        let (_, p) = points[i as usize % points.len()];
        t.click(p[0], p[1], NONE);
    }
    let click = ms(start, n);

    let ids: Vec<u32> = points.iter().take(50).map(|&(id, _)| id).collect();
    t.s.select(Level::Top, ids.iter().copied());
    let seats: usize = ids
        .iter()
        .flat_map(|&id| t.s.document().children(Some(id)).to_vec())
        .map(|b| t.s.index().seats(b).len())
        .sum();
    let p = points[0].1;
    t.down(p[0], p[1], NONE);
    t.to(p[0] + 10.0, p[1], NONE);
    let frames = 200;
    let start = Instant::now();
    for i in 0..frames {
        t.to(p[0] + 10.0 + f64::from(i), p[1] + f64::from(i % 7), NONE);
    }
    let drag = ms(start, frames);
    t.up(p[0], p[1], NONE);

    let area = (t.s.document().children(None).iter())
        .filter_map(|&id| t.s.index().bounds(id))
        .fold(None, |a, b| Rect::union(a, Some(b)))
        .expect("bounds");
    t.click(area.min[0] - 100.0, area.min[1] - 100.0, NONE);
    t.down(area.min[0] - 100.0, area.min[1] - 100.0, NONE);
    let start = Instant::now();
    for i in 0..frames {
        let f = f64::from(i) / f64::from(frames);
        let x = area.min[0] + f * (area.max[0] - area.min[0]);
        t.to(x, area.max[1], NONE);
    }
    let marquee = ms(start, frames);
    t.up(area.max[0], area.max[1], NONE);
    let chosen = t.s.selection().items().len();

    let rows = vec![serde_json::json!({ "rid": 0, "seats": 100, "oids": [] }); 50];
    let node = serde_json::from_value(serde_json::json!({
        "id": 0, "type": "rowBlock", "rows": rows
    }))
    .expect("node");
    let created =
        t.s.apply(Command::CreateNode {
            parent: None,
            index: 0,
            node,
        })
        .expect("create");
    let big = created.created[0];
    let start = Instant::now();
    t.s.apply(Command::AddRows {
        id: big,
        index: 0,
        count: 1,
        seats: 100,
    })
    .expect("add");
    let add = ms(start, 1);
    let start = Instant::now();
    t.s.undo().expect("undo");
    let undo = ms(start, 1);
    println!(
        "stadium-100k, through the session (input to index and selection up to date): \
         click {click:.4} ms; drag of 50 sections ({seats} seats) per pointer move {drag:.3} ms; \
         box selection per pointer move {marquee:.3} ms ({chosen} selected); \
         add a row to a 5,000-seat block {add:.3} ms, undo {undo:.3} ms"
    );
}

/// The image tool's node (07 §4, DSN-020): placed from a draft as one undo
/// entry and selected, refused before anything is created for a hash of
/// another form or an empty size, scaled by its corner handle about the
/// opposite corner, and once locked out of reach of a stray click, a drag
/// and a marquee, while the layers panel still selects and unlocks it.
#[test]
fn image_place_scale_and_lock() {
    use cavea_compiler::document::ImageRole;
    let mut t = Trace::new();
    let hash = "0123456789abcdef0123456789abcdef";
    let draft = |asset: &str, size: [f64; 2]| Draft::Image {
        center: [2000.0, 2000.0],
        size,
        asset: asset.into(),
        role: ImageRole::Reference,
    };
    let before = t.state();
    for (bad, field) in [
        (
            draft("0123456789ABCDEF0123456789ABCDEF", [400.0, 200.0]),
            "asset",
        ),
        (draft(&hash[1..], [400.0, 200.0]), "asset"),
        (draft(hash, [0.0, 200.0]), "size"),
        (draft(hash, [f64::INFINITY, 200.0]), "size"),
    ] {
        assert_eq!(
            t.s.insert(None, &bad),
            Err(CommandError::InvalidValue(field))
        );
        assert_eq!(t.state(), before);
    }
    let id =
        t.s.insert(None, &draft(hash, [400.0, 200.0]))
            .expect("insert");
    let image = |t: &Trace| match t.s.document().node(id) {
        Some(Node::Image(i)) => i.clone(),
        other => panic!("not an image: {other:?}"),
    };
    let placed = image(&t);
    assert_eq!(
        (placed.origin, placed.size),
        ([2000.0, 2000.0], [400.0, 200.0])
    );
    assert_eq!(
        (placed.rotation, placed.opacity, placed.role),
        (0.0, 1.0, ImageRole::Reference)
    );
    assert_eq!(placed.asset, hash);
    assert_eq!(t.sel(), (Level::Top, vec![id]));
    assert_eq!(t.s.undo(), Ok(true));
    assert_eq!(t.state(), before);
    assert_eq!(t.s.redo(), Ok(true));
    t.s.click_layer(id, LayerClick::Replace);

    // The bottom-right corner, dragged by (100, 50): the top-left stays.
    t.drag([2200.0, 2100.0], [2300.0, 2150.0], NONE);
    let scaled = image(&t);
    assert_eq!(
        (scaled.origin, scaled.size),
        ([2050.0, 2025.0], [500.0, 250.0])
    );

    t.s.set_locked(true).expect("lock");
    let locked = t.state();
    t.click(2050.0, 2025.0, NONE);
    assert_eq!(t.sel(), (Level::Top, vec![]));
    t.drag([2050.0, 2025.0], [2500.0, 2500.0], NONE);
    t.drag([1500.0, 1500.0], [2600.0, 2600.0], NONE);
    t.drag([2050.0, 2025.0], [2100.0, 2100.0], ALT);
    assert_eq!(t.sel(), (Level::Top, vec![]));
    assert_eq!(t.state(), locked);

    // The layers panel reaches it, and unlocks it.
    assert!(t.s.click_layer(id, LayerClick::Replace));
    assert_eq!(t.sel(), (Level::Top, vec![id]));
    t.s.lock_layer(id, false).expect("unlock");
    t.click(2050.0, 2025.0, NONE);
    assert_eq!(t.sel(), (Level::Top, vec![id]));
}
