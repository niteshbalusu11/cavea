//! Row, seat and arrange edits, the category brush, the focal point tool and
//! the clipboard (07 §4, §5.1, §5.3): each operation's effect on seat
//! identities, scripted traces through the tools, the identity property
//! under random edit sequences, hostile input, and a theatre edited end to
//! end.

use std::collections::{BTreeMap, BTreeSet};

use cavea_compiler::document::{Align, Node, RowBlock, SeatDirection, SeatFlag};
use cavea_compiler::{ChartDocument, Code, compile};

use crate::tests::Rng;
use crate::tool::draw_tests::unsnapped;
use crate::*;

const NONE: Modifiers = Modifiers {
    shift: false,
    alt: false,
    ctrl: false,
};
const CTRL: Modifiers = Modifiers { ctrl: true, ..NONE };

/// A centre-aligned block (10) of 3 rows × 6 seats, pitches 50 and 100, at
/// the origin (seats at x −125 … 125, rows at y 0, 100, 200); a section (30,
/// category `vip`) holding a 2 × 4 block (31) without a category; a
/// rectangle (40), a text (41), a polygon (42) and a free-form row (50) of
/// 7 seats. Zoom 1.
fn chart() -> ChartDocument {
    serde_json::from_value(serde_json::json!({
        "schema": 1, "name": "e", "nextOid": 500, "settings": { "seatSize": 40 },
        "categories": [{ "key": "std", "label": "Standard", "color": "#2563EB" },
                       { "key": "vip", "label": "VIP", "color": "#B45309" },
                       { "key": "access", "label": "Access", "color": "#059669",
                         "accessible": true }],
        "floors": [{ "id": 1, "name": "Stalls", "focalPoint": [0, -500] }],
        "nodes": [
            { "id": 10, "type": "rowBlock", "seatPitch": 50, "rowPitch": 100, "category": "std",
              "rows": [{ "rid": 11, "seats": 6, "oids": [[100, 6]] },
                       { "rid": 12, "seats": 6, "oids": [[110, 6]] },
                       { "rid": 13, "seats": 6, "oids": [[120, 6]] }] },
            { "id": 30, "type": "section", "label": "Box", "category": "vip", "origin": [1000, 0],
              "children": [{ "id": 31, "type": "rowBlock", "seatPitch": 50, "rowPitch": 100,
                "rows": [{ "rid": 32, "seats": 4, "oids": [[130, 4]] },
                         { "rid": 33, "seats": 4, "oids": [[140, 4]] }] }] },
            { "id": 40, "type": "shape", "kind": "rect", "size": [200, 100], "origin": [0, 600] },
            { "id": 41, "type": "text", "text": "STAGE", "size": 60, "color": "#111827",
              "origin": [400, 600], "align": "left" },
            { "id": 42, "type": "shape", "kind": "polygon", "origin": [-500, 600],
              "points": [[0, 0], [100, 0], [0, 80]] },
            { "id": 50, "type": "row", "origin": [-600, -400], "seatPitch": 50, "seats": 7,
              "path": { "kind": "polyline", "points": [[0, 0], [150, 0], [300, 60]] },
              "oids": [[150, 7]], "category": "std" }
        ]
    }))
    .expect("chart")
}

struct T {
    s: Session,
    time: f64,
}

impl T {
    fn new(doc: ChartDocument) -> Self {
        Self {
            s: Session::load(doc).map(unsnapped).expect("load"),
            time: 0.0,
        }
    }

    fn input(&mut self, kind: fn(Pointer) -> Input, at: [f64; 2], mods: Modifiers) {
        self.time += 1_000.0;
        let p = Pointer {
            world: at,
            screen: at,
            zoom: 1.0,
            button: Button::Primary,
            kind: PointerKind::Mouse,
            mods,
            time: self.time,
        };
        self.s.input(kind(p));
    }

    fn drag(&mut self, path: &[[f64; 2]]) {
        let (Some(&first), Some(&last)) = (path.first(), path.last()) else {
            return;
        };
        self.input(Input::PointerDown, first, NONE);
        for &p in path {
            self.input(Input::PointerMove, p, NONE);
        }
        self.input(Input::PointerUp, last, NONE);
    }

    fn key(&mut self, key: Key, mods: Modifiers) {
        self.s.input(Input::KeyDown(key, mods));
        self.s.input(Input::KeyUp(key, mods));
    }

    fn block(&self, id: u32) -> RowBlock {
        match self.s.document().node(id) {
            Some(Node::RowBlock(b)) => b.clone(),
            _ => panic!("no block {id}"),
        }
    }

    fn node(&self, id: u32) -> Node {
        self.s.document().node(id).cloned().expect("node")
    }

    /// World positions of a node's seat positions, by oid.
    fn seats(&self, id: u32) -> BTreeMap<u32, [f64; 2]> {
        let ix = self.s.index();
        let world = |p| ix.to_world(id, p).expect("world");
        ix.seats(id).iter().map(|s| (s.oid, world(s.pos))).collect()
    }

    fn removed(&self, id: u32) -> BTreeSet<u32> {
        let ix = self.s.index().seats(id).iter();
        ix.filter(|s| s.removed).map(|s| s.oid).collect()
    }

    fn entries(&self) -> usize {
        self.s.editor().undo_len()
    }

    fn state(&self) -> Vec<u8> {
        self.s.document().snapshot().to_json().expect("json")
    }

    fn selected(&self) -> Vec<u32> {
        self.s.selection().items().iter().copied().collect()
    }
}

fn close(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
}

/// Every seat of `before` that is in `after` is where it was.
fn kept(before: &BTreeMap<u32, [f64; 2]>, after: &BTreeMap<u32, [f64; 2]>) -> usize {
    let same = before
        .iter()
        .filter(|(o, p)| after.get(o).is_some_and(|q| close(**p, *q)));
    same.count()
}

fn oids(b: &RowBlock, row: usize) -> Vec<u32> {
    b.rows[row]
        .oids
        .iter()
        .flat_map(|r| r[0]..r[0] + r[1])
        .collect()
}

#[test]
fn row_level_editing() {
    let mut t = T::new(chart());
    let start = t.seats(10);
    t.s.select(Level::Rows(10), [11, 12]);
    // Add two at the end of the two rows, remove one at the start of each.
    t.s.row_seats(End::End, 2).expect("add");
    t.s.row_seats(End::Start, -1).expect("remove");
    let b = t.block(10);
    assert_eq!(oids(&b, 0), [101, 102, 103, 104, 105, 500, 501]);
    assert_eq!(oids(&b, 1), [111, 112, 113, 114, 115, 502, 503]);
    assert_eq!(oids(&b, 2), (120..126).collect::<Vec<_>>());
    assert_eq!(kept(&start, &t.seats(10)), 16, "kept seats stay put");
    assert_eq!(t.entries(), 2);
    // The right handle at the rows level adds seats to the selected rows only.
    let right = t.s.overlay().handles[0];
    assert_eq!(right.kind, HandleKind::Side(Side::Right));
    t.drag(&[right.at, [right.at[0] + 100.0, right.at[1]]]);
    let b = t.block(10);
    assert_eq!(
        (b.rows[0].seats, b.rows[1].seats, b.rows[2].seats),
        (9, 9, 6)
    );
    assert_eq!(kept(&start, &t.seats(10)), 16);

    // Shift: half a pitch with an arrow key, a whole one with Shift; ids stay.
    let before = t.seats(10);
    t.key(Key::ArrowRight, NONE);
    t.key(
        Key::ArrowLeft,
        Modifiers {
            shift: true,
            ..NONE
        },
    );
    let after = t.seats(10);
    for (oid, p) in &before {
        let row0 = oids(&t.block(10), 0).contains(oid) || oids(&t.block(10), 1).contains(oid);
        let dx = if row0 { -25.0 } else { 0.0 };
        assert!(close(after[oid], [p[0] + dx, p[1]]), "seat {oid}");
    }
    assert_eq!(t.block(10).rows[0].offset, t.block(10).rows[1].offset);

    // Labels: an override keeps the row's place; clearing removes it.
    t.s.set_row_label(Some("AA".into())).expect("label");
    assert_eq!(t.block(10).rows[0].label.as_deref(), Some("AA"));
    let long = Some("x".repeat(65));
    assert_eq!(
        t.s.set_row_label(long),
        Err(CommandError::Limit("label bytes"))
    );
    t.s.set_row_label(None).expect("clear");
    assert!(t.block(10).rows.iter().all(|r| r.label.is_none()));

    // Cut an aisle through all three rows at the column of seat 103: the
    // nearest position of each row is removed and keeps its oid and number.
    t.s.select(Level::Rows(10), [11, 12, 13]);
    let at = t.seats(10)[&103];
    let numbers = |b: &RowBlock| (0..3).map(|r| oids(b, r)).collect::<Vec<_>>();
    let before = numbers(&t.block(10));
    t.s.cut_aisle(at).expect("cut");
    assert_eq!(numbers(&t.block(10)), before, "no seat renumbered");
    let gone = t.removed(10);
    assert!(gone.contains(&103) && gone.len() == 3, "{gone:?}");
    let cols: Vec<f64> = gone.iter().map(|o| t.seats(10)[o][0]).collect();
    assert!(cols.iter().all(|x| (x - cols[0]).abs() <= 25.0), "{cols:?}");

    // A row of one position is never cut.
    let mut lone = T::new(chart());
    lone.s
        .apply(Command::RemoveSeats {
            id: 10,
            rid: 13,
            end: End::End,
            count: 5,
        })
        .expect("short");
    lone.s.select(Level::Rows(10), [13]);
    lone.s.cut_aisle([10_000.0, 200.0]).expect("cut");
    assert!(lone.removed(10).is_empty());

    // On a skewed arc block the aisle is radial: the same position of
    // every row of equal length.
    let mut arc = T::new(chart());
    let set = |t: &mut T, property| {
        t.s.apply(Command::SetProperty { id: 10, property })
            .expect("set");
    };
    set(
        &mut arc,
        Property::Curve(cavea_compiler::document::Curve::Arc { radius: 1_000.0 }),
    );
    set(&mut arc, Property::Skew(45.0));
    arc.s.select(Level::Rows(10), [11, 12, 13]);
    let at = arc.seats(10)[&102];
    arc.s.cut_aisle(at).expect("cut");
    assert_eq!(arc.removed(10), BTreeSet::from([102, 112, 122]));

    // Open an aisle one position wide between the 2nd and 3rd seats of the
    // back row: new removed position, later seats one pitch on, oids kept.
    t.s.select(Level::Rows(10), [13]);
    let seats = t.seats(10);
    let at = [(seats[&121][0] + seats[&122][0]) / 2.0, seats[&121][1]];
    let next = t.s.document().next_oid();
    t.s.open_aisle(at, 1).expect("open");
    let b = t.block(10);
    assert_eq!(&oids(&b, 2)[..4], [120, 121, next, 122]);
    assert!(b.rows[2].removed.contains(&2));
    let now = t.seats(10);
    assert!(close(now[&120], seats[&120]) && close(now[&121], seats[&121]));
    assert!(close(now[&122], [seats[&122][0] + 50.0, seats[&122][1]]));

    // Delete: a middle row is emptied in place (the rows behind would move
    // and be renamed); the next row is selected. A back row is removed.
    let seats = t.seats(10);
    t.s.select(Level::Rows(10), [12]);
    t.key(Key::Delete, NONE);
    let b = t.block(10);
    assert_eq!(
        (b.rows.len(), b.rows[1].removed.len() as u32),
        (3, b.rows[1].seats)
    );
    assert_eq!(kept(&seats, &t.seats(10)), seats.len());
    assert_eq!(t.s.selection().level(), Level::Rows(10));
    assert_eq!(t.selected(), [13]);
    t.key(Key::Backspace, NONE);
    assert_eq!(t.block(10).rows.len(), 2);
    assert_eq!(t.selected(), [12]);
    // All rows: the block goes.
    t.s.select(Level::Rows(10), [11, 12]);
    t.s.delete().expect("delete");
    assert!(t.s.document().node(10).is_none());
    assert_eq!(t.s.selection().level(), Level::Top);
}

#[test]
fn seat_editing() {
    let mut t = T::new(chart());
    t.s.select(Level::Seats(10), [100, 101, 102]);
    let flag = |flag, on| SeatEdit::Flag { flag, on };
    t.s.edit_seats(flag(SeatFlag::Companion, true))
        .expect("flag");
    t.s.edit_seats(flag(SeatFlag::Accessible, true))
        .expect("flag");
    let b = t.block(10);
    assert_eq!(b.seat_overrides.len(), 3);
    let flags = &b.seat_overrides[&101].flags;
    assert_eq!(
        flags,
        &[SeatFlag::Accessible, SeatFlag::Companion],
        "schema order"
    );
    t.s.edit_seats(flag(SeatFlag::Accessible, false))
        .expect("flag");
    t.s.edit_seats(flag(SeatFlag::Companion, false))
        .expect("flag");
    assert!(
        t.block(10).seat_overrides.is_empty(),
        "empty overrides are removed"
    );
    assert_eq!(t.entries(), 4);
    // Unchanged seats make no entry.
    t.s.edit_seats(flag(SeatFlag::Companion, false))
        .expect("flag");
    assert_eq!(t.entries(), 4);

    // Category: an override, then the block's own category clears it; in a
    // section, the section's category is the one inherited.
    let cat = |c: &str| SeatEdit::Category {
        category: Some(c.into()),
    };
    t.s.edit_seats(cat("vip")).expect("vip");
    assert_eq!(
        t.block(10).seat_overrides[&100].category.as_deref(),
        Some("vip")
    );
    t.s.edit_seats(cat("std")).expect("std");
    assert!(t.block(10).seat_overrides.is_empty());
    let state = t.state();
    assert_eq!(
        t.s.edit_seats(cat("nope")),
        Err(CommandError::MissingCategory("nope".into()))
    );
    assert_eq!(t.state(), state);
    t.s.select(Level::Seats(31), [130]);
    t.s.edit_seats(cat("vip")).expect("inherited");
    assert!(t.block(31).seat_overrides.is_empty());

    // Label and nudge, by key in a turned section's block, and by dragging.
    t.s.edit_seats(SeatEdit::Label {
        label: Some("W1".into()),
    })
    .expect("label");
    t.s.apply(Command::TransformNodes {
        nodes: vec![NodeTransform {
            id: 30,
            origin: [1000.0, 0.0],
            rotation: 90.0,
        }],
    })
    .expect("turn");
    let before = t.seats(31)[&130];
    t.key(Key::ArrowRight, NONE);
    let after = t.seats(31)[&130];
    assert!(close(after, [before[0] + 1.0, before[1]]), "{after:?}");
    let o = &t.block(31).seat_overrides[&130];
    assert_eq!(o.label.as_deref(), Some("W1"));
    assert!(close(o.nudge.expect("nudge"), [0.0, -1.0]));

    t.s.select(Level::Seats(10), [104, 105]);
    let (p, q) = (t.seats(10)[&104], t.seats(10)[&105]);
    let entries = t.entries();
    t.drag(&[p, [p[0] + 3.0, p[1] + 7.0], [p[0] + 6.0, p[1] + 9.0]]);
    assert_eq!(t.entries(), entries + 1, "one undo entry");
    assert!(close(t.seats(10)[&105], [q[0] + 6.0, q[1] + 9.0]));
    // Escape mid-drag restores the start; dragging back to it leaves no nudge.
    let state = t.state();
    t.input(Input::PointerDown, t.seats(10)[&104], NONE);
    t.input(Input::PointerMove, [0.0, 0.0], NONE);
    t.key(Key::Escape, NONE);
    assert_eq!(t.state(), state);
    t.drag(&[t.seats(10)[&104], p]);
    assert!(t.block(10).seat_overrides.is_empty());

    // Remove (Delete) and restore: the oid stays, and the seat stays
    // selected so it can be restored.
    t.key(Key::Backspace, NONE);
    assert_eq!(t.block(10).rows[0].removed, [4, 5]);
    assert_eq!(t.selected(), [104, 105]);
    t.s.edit_seats(SeatEdit::Restore).expect("restore");
    assert!(t.block(10).rows[0].removed.is_empty());
    assert_eq!(t.block(10).rows[0].oids, [[100, 6]]);

    // A free-form row: the same edits, through its own `removed`.
    let row = |t: &T| match t.node(50) {
        Node::Row(r) => r,
        _ => unreachable!(),
    };
    t.s.select(Level::Seats(50), [151, 152]);
    t.key(Key::Delete, NONE);
    t.s.edit_seats(cat("vip")).expect("vip");
    assert_eq!(row(&t).removed, [1, 2]);
    assert_eq!(row(&t).seat_overrides.len(), 2);
    t.s.edit_seats(SeatEdit::Restore).expect("restore");
    t.s.edit_seats(cat("std")).expect("std");
    let r = row(&t);
    assert!(r.removed.is_empty() && r.seat_overrides.is_empty());
}

#[test]
fn category_brush() {
    let mut t = T::new(chart());
    t.s.set_tool(ToolId::Brush);
    t.s.set_category(Some("vip".into()));
    // Top level: a block, and a section (reached as a click reaches it).
    t.drag(&[[-125.0, 0.0], [0.0, 100.0], [1_000.0, 0.0]]);
    assert_eq!(t.block(10).category.as_deref(), Some("vip"));
    assert_eq!(t.entries(), 1, "one entry per stroke");
    t.s.set_category(Some("std".into()));
    t.drag(&[[1_000.0, 0.0]]);
    match t.node(30) {
        Node::Section(s) => assert_eq!(s.category.as_deref(), Some("std")),
        _ => unreachable!(),
    }
    t.s.undo().expect("undo");
    t.s.undo().expect("undo");
    // Rows: every seat of each row passed over.
    t.s.select(Level::Rows(10), []);
    t.s.set_category(Some("access".into()));
    t.drag(&[[-125.0, 0.0], [-125.0, 100.0]]);
    let b = t.block(10);
    assert_eq!(b.seat_overrides.len(), 12);
    assert!(b.seat_overrides.keys().all(|o| (100..116).contains(o)));
    // Painting back to the block's category clears the overrides.
    t.s.set_category(Some("std".into()));
    t.drag(&[[125.0, 0.0], [125.0, 100.0]]);
    assert!(t.block(10).seat_overrides.is_empty());
    // Seats: those under the brush only; Escape cancels the stroke.
    t.s.select(Level::Seats(10), []);
    t.s.set_category(Some("vip".into()));
    t.drag(&[[-125.0, 200.0], [-75.0, 200.0]]);
    let keys: Vec<u32> = t.block(10).seat_overrides.keys().copied().collect();
    assert_eq!(keys, [120, 121]);
    let state = t.state();
    t.input(Input::PointerDown, [125.0, 200.0], NONE);
    t.key(Key::Escape, NONE);
    assert_eq!(t.state(), state);
    // A locked block is not painted.
    t.s.apply(Command::SetProperty {
        id: 10,
        property: Property::Locked(true),
    })
    .expect("lock");
    t.drag(&[[125.0, 200.0]]);
    assert_eq!(t.block(10).seat_overrides.len(), 2);
}

#[test]
fn focal_point_tool() {
    let mut t = T::new(chart());
    let nodes = |t: &T| serde_json::to_string(&t.s.document().snapshot().nodes).expect("json");
    let before = nodes(&t);
    assert_eq!(t.s.overlay().focal, Some([0.0, -500.0]));
    t.s.set_tool(ToolId::Focal);
    t.drag(&[[10.0, -700.0], [40.0, -800.0], [50.0, -900.0]]);
    assert_eq!(t.s.document().floors()[0].focal_point, Some([50.0, -900.0]));
    assert_eq!(t.s.overlay().focal, Some([50.0, -900.0]));
    assert_eq!(t.entries(), 1);
    assert_eq!(nodes(&t), before, "only the floor changes");
    let state = t.state();
    t.input(Input::PointerDown, [0.0, 0.0], NONE);
    t.key(Key::Escape, NONE);
    assert_eq!(t.state(), state);
    t.s.set_focal_point(None).expect("clear");
    assert_eq!(t.s.overlay().focal, None);
}

#[test]
fn align_distribute_order_lock() {
    let mut t = T::new(chart());
    let ids = [10, 40, 41, 42];
    let bounds = |t: &T| ids.map(|id| t.s.index().bounds(id).expect("bounds"));
    t.s.select(Level::Top, ids);
    let all = Rect::around(bounds(&t).iter().flat_map(|r| [r.min, r.max])).expect("all");
    t.s.align(AlignTo::Left).expect("left");
    assert!(
        bounds(&t)
            .iter()
            .all(|r| (r.min[0] - all.min[0]).abs() < 1e-9)
    );
    t.s.align(AlignTo::Bottom).expect("bottom");
    assert!(
        bounds(&t)
            .iter()
            .all(|r| (r.max[1] - all.max[1]).abs() < 1e-9)
    );
    t.s.align(AlignTo::Middle).expect("middle");
    let mid = |r: &Rect, i: usize| (r.min[i] + r.max[i]) / 2.0;
    let m = mid(&bounds(&t)[0], 1);
    assert!(bounds(&t).iter().all(|r| (mid(r, 1) - m).abs() < 1e-9));
    t.s.distribute(Axis::Horizontal).expect("distribute");
    let mut b = bounds(&t).to_vec();
    b.sort_by(|x, y| x.min[0].total_cmp(&y.min[0]));
    let gaps: Vec<f64> = b.windows(2).map(|w| w[1].min[0] - w[0].max[0]).collect();
    assert!(gaps.iter().all(|g| (g - gaps[0]).abs() < 1e-9), "{gaps:?}");
    assert_eq!(t.entries(), 4);
    // In a turned section, alignment is still along the world's axes.
    t.s.apply(Command::CreateNode {
        parent: Some(30),
        index: 1,
        node: Box::new(t.node(40)),
    })
    .expect("child");
    t.s.apply(Command::TransformNodes {
        nodes: vec![NodeTransform {
            id: 30,
            origin: [1000.0, 0.0],
            rotation: 30.0,
        }],
    })
    .expect("turn");
    let kids = t.s.document().children(Some(30)).to_vec();
    t.s.select(Level::Section(30), kids.clone());
    t.s.align(AlignTo::Top).expect("top");
    let tops: Vec<f64> = kids
        .iter()
        .map(|&k| t.s.index().bounds(k).expect("b").min[1])
        .collect();
    assert!((tops[0] - tops[1]).abs() < 1e-6, "{tops:?}");

    // Order among siblings.
    let roots = |t: &T| t.s.document().children(None).to_vec();
    t.s.select(Level::Top, [40, 41]);
    t.s.order(Order::Front).expect("front");
    assert_eq!(roots(&t), [10, 30, 42, 50, 40, 41]);
    t.s.order(Order::Backward).expect("backward");
    assert_eq!(roots(&t), [10, 30, 42, 40, 41, 50]);
    t.s.order(Order::Back).expect("back");
    assert_eq!(roots(&t), [40, 41, 10, 30, 42, 50]);
    t.s.select(Level::Top, [40]);
    t.s.order(Order::Forward).expect("forward");
    assert_eq!(roots(&t), [41, 40, 10, 30, 42, 50]);

    // Lock and hide toggle (Ctrl+L, Ctrl+H); a locked node can be unlocked.
    let flags = |t: &T, id| match t.node(id) {
        Node::Shape(s) => (s.locked, s.hidden),
        _ => unreachable!(),
    };
    t.s.select(Level::Top, [40, 42]);
    t.key(Key::Char('l'), CTRL);
    assert_eq!(
        (flags(&t, 40), flags(&t, 42)),
        ((true, false), (true, false))
    );
    t.key(Key::Char('l'), CTRL);
    t.key(Key::Char('h'), CTRL);
    assert_eq!(flags(&t, 40), (false, true));
    t.s.set_hidden(false).expect("show");
    assert_eq!(flags(&t, 42), (false, false));
}

/// A flip is a mirror image; a row block's seats each keep their oid,
/// overrides and label on their own mirrored seat.
#[test]
fn flips() {
    let mut t = T::new(chart());
    let set = |t: &mut T, id, property| {
        t.s.apply(Command::SetProperty { id, property })
            .expect("set");
    };
    set(&mut t, 10, Property::Align(Align::Left));
    set(&mut t, 10, Property::Stagger(0.5));
    set(&mut t, 10, Property::Skew(10.0));
    t.s.apply(Command::TransformNodes {
        nodes: vec![NodeTransform {
            id: 10,
            origin: [30.0, 40.0],
            rotation: 20.0,
        }],
    })
    .expect("turn");
    t.s.apply(Command::AddSeats {
        id: 10,
        rid: 12,
        end: End::End,
        count: 3,
    })
    .expect("seats");
    t.s.select(Level::Seats(10), [102]);
    t.s.edit_seats(SeatEdit::Remove).expect("remove");
    t.s.select(Level::Seats(10), [113]);
    t.s.edit_seats(SeatEdit::Nudge { by: [7.0, 3.0] })
        .expect("nudge");
    t.s.edit_seats(SeatEdit::Flag {
        flag: SeatFlag::Aisle,
        on: true,
    })
    .expect("flag");
    for axis in [Axis::Horizontal, Axis::Vertical] {
        t.s.select(Level::Top, [10]);
        let r = t.s.index().bounds(10).expect("bounds");
        let c = [(r.min[0] + r.max[0]) / 2.0, (r.min[1] + r.max[1]) / 2.0];
        let (before, gone, b0) = (t.seats(10), t.removed(10), t.block(10));
        t.s.flip(axis).expect("flip");
        let b = t.block(10);
        for (oid, p) in &before {
            let want = match axis {
                Axis::Horizontal => [2.0 * c[0] - p[0], p[1]],
                Axis::Vertical => [p[0], 2.0 * c[1] - p[1]],
            };
            assert!(close(t.seats(10)[oid], want), "{axis:?} seat {oid}");
        }
        assert_eq!(t.removed(10), gone);
        assert_eq!(b.seat_overrides.keys().collect::<Vec<_>>(), [&113]);
        assert_eq!(b.seat_overrides[&113].flags, [SeatFlag::Aisle]);
        // Labels count from the other end now: each oid keeps its number.
        assert_eq!(b.seat_labels.direction, SeatDirection::Rtl);
        let row = |b: &RowBlock, i| oids(b, i).into_iter().rev().collect::<Vec<_>>();
        assert_eq!(row(&b, 1), oids(&b0, 1));
        assert!(
            b.rows[0]
                .removed
                .iter()
                .all(|&p| oids(&b, 0)[p as usize] == 102)
        );
        t.s.undo().expect("undo");
    }
    // Flipping twice restores every position and the oid runs; a free-form
    // row's path is mirrored, its seats keeping their oids.
    let (seats, b0) = (t.seats(10), t.block(10));
    t.s.select(Level::Top, [10, 30, 41, 42, 50]);
    let r = Rect::around([10, 30, 41, 42, 50].iter().flat_map(|&id| {
        let b = t.s.index().bounds(id).expect("bounds");
        [b.min, b.max]
    }))
    .expect("bounds");
    let cx = (r.min[0] + r.max[0]) / 2.0;
    let path = t.seats(50);
    t.s.flip(Axis::Horizontal).expect("flip");
    for (oid, p) in &path {
        assert!(
            close(t.seats(50)[oid], [2.0 * cx - p[0], p[1]]),
            "row seat {oid}"
        );
    }
    match t.node(41) {
        Node::Text(x) => assert_eq!(x.align, Align::Right),
        _ => unreachable!(),
    }
    t.s.flip(Axis::Horizontal).expect("flip");
    assert_eq!(kept(&seats, &t.seats(10)), seats.len());
    assert_eq!(t.block(10).rows, b0.rows);
}

#[test]
fn duplicate_and_array() {
    let mut t = T::new(chart());
    t.s.select(Level::Seats(10), [101]);
    t.s.edit_seats(SeatEdit::Flag {
        flag: SeatFlag::Accessible,
        on: true,
    })
    .expect("flag");
    t.s.select(Level::Top, [10]);
    let entries = t.entries();
    let next = t.s.document().next_oid();
    t.key(Key::Char('d'), CTRL);
    assert_eq!(t.entries(), entries + 1);
    let [copy] = t.selected()[..] else {
        panic!("one copy selected")
    };
    let (a, b) = (t.block(10), t.block(copy));
    assert!(copy >= next);
    assert_eq!(b.origin, [DUPLICATE_OFFSET, DUPLICATE_OFFSET]);
    assert_eq!(b.rows.len(), 3);
    assert!(b.rows.iter().all(|r| r.rid >= next && r.oids[0][0] >= next));
    // The override follows its seat to the new oid.
    let moved = oids(&b, 0)[1];
    assert_eq!(b.seat_overrides.keys().collect::<Vec<_>>(), [&moved]);
    assert_eq!((a.row_labels, a.seat_labels), (b.row_labels, b.seat_labels));

    // Array: three copies a step apart, then four turned about a point.
    t.s.select(Level::Top, [40]);
    t.s.array(3, ArrayStep::Offset([0.0, 300.0]))
        .expect("array");
    let made = t.selected();
    let origins: Vec<[f64; 2]> = made
        .iter()
        .map(|&id| match t.node(id) {
            Node::Shape(s) => s.origin,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(origins, [[0.0, 900.0], [0.0, 1200.0], [0.0, 1500.0]]);
    t.s.array(
        4,
        ArrayStep::Around {
            centre: [0.0, 0.0],
            angle: 90.0,
        },
    )
    .expect("around");
    assert_eq!(t.selected().len(), 12);
    // Limits, checked before anything is made or any id taken.
    t.s.apply(Command::AddRows {
        id: 10,
        index: 3,
        count: 200,
        seats: 6,
    })
    .expect("rows");
    t.s.select(Level::Top, [10]);
    let (state, saved) = (t.state(), t.s.document().save().next_oid);
    let step = ArrayStep::Offset([1.0, 0.0]);
    assert_eq!(
        t.s.array(MAX_COPIES + 1, step),
        Err(CommandError::Limit("copies"))
    );
    assert_eq!(t.s.array(900, step), Err(CommandError::Limit("objects")));
    assert_eq!(t.s.array(0, step), Err(CommandError::InvalidValue("count")));
    assert_eq!((t.state(), t.s.document().save().next_oid), (state, saved));
}

#[test]
fn copy_and_paste() {
    let mut t = T::new(chart());
    t.s.select(Level::Seats(31), [130]);
    t.s.edit_seats(SeatEdit::Category {
        category: Some("access".into()),
    })
    .expect("access");
    t.s.select(Level::Top, [10, 30, 41]);
    let text = t.s.copy().expect("copy");
    assert_eq!(t.s.copy().as_deref(), Some(text.as_str()), "canonical");
    assert!(text.starts_with(r#"{"format":"cavea-clipboard","version":1,"#));
    // Within the chart, centred on a point: fresh ids, overrides re-keyed.
    let before: BTreeSet<u32> = t.s.document().ids().collect();
    t.s.paste(&text, Some([5_000.0, 5_000.0])).expect("paste");
    let made = t.selected();
    assert_eq!(made.len(), 3);
    assert!(made.iter().all(|id| !before.contains(id)));
    let all = Rect::around(made.iter().flat_map(|&id| {
        let r = t.s.index().bounds(id).expect("bounds");
        [r.min, r.max]
    }))
    .expect("bounds");
    let c = [
        (all.min[0] + all.max[0]) / 2.0,
        (all.min[1] + all.max[1]) / 2.0,
    ];
    assert!(close(c, [5_000.0, 5_000.0]), "{c:?}");
    assert_eq!(t.entries(), 2, "one entry per paste");

    // Into another chart: a missing category is added, a clashing key keeps
    // the target's definition; pasted nodes go on the floor shown.
    let other: ChartDocument = serde_json::from_value(serde_json::json!({
        "schema": 1, "name": "o", "nextOid": 9000,
        "categories": [{ "key": "vip", "label": "Gold", "color": "#FFD700" }],
        "floors": [{ "id": 4, "name": "Ground" }, { "id": 7, "name": "Upper" }],
        "nodes": []
    }))
    .expect("other");
    let mut o = T::new(other);
    o.s.set_floor(7);
    o.s.paste(&text, None).expect("paste");
    let keys: Vec<(String, String)> =
        o.s.document()
            .categories()
            .iter()
            .map(|c| (c.key.clone(), c.label.clone()))
            .collect();
    let want = [("vip", "Gold"), ("std", "Standard"), ("access", "Access")];
    assert_eq!(keys, want.map(|(k, l)| (k.to_string(), l.to_string())));
    let made = o.selected();
    let floors: Vec<Option<u32>> = made
        .iter()
        .map(|&id| match o.node(id) {
            Node::RowBlock(b) => b.floor,
            Node::Section(s) => s.floor,
            Node::Text(x) => x.floor,
            _ => None,
        })
        .collect();
    assert_eq!(floors, [Some(7); 3]);
    assert!(made.iter().all(|&id| id >= 9000));
    let doc = o.s.document().save();
    assert!(compile(&doc).is_ok(), "{:?}", compile(&doc).err());

    // A section cannot be pasted into a section; a block can.
    o.s.select(Level::Section(made[1]), []);
    assert_eq!(o.s.paste(&text, None), Err(CommandError::Placement));
    t.s.select(Level::Top, [10]);
    let block = t.s.copy().expect("copy");
    o.s.paste(&block, Some([0.0, 0.0])).expect("into section");
    assert_eq!(o.s.selection().level(), Level::Section(made[1]));

    // Malformed, oversized, other versions, too many categories: refused,
    // nothing changed.
    let state = o.state();
    let bad = [
        ("", CommandError::InvalidValue("clipboard")),
        ("{\"nodes\":[]}", CommandError::InvalidValue("clipboard")),
        (
            &text.replace("\"version\":1", "\"version\":2"),
            CommandError::InvalidValue("clipboard version"),
        ),
        (
            &" ".repeat(MAX_CLIPBOARD + 1),
            CommandError::Limit("clipboard bytes"),
        ),
        (
            &text[..text.len() / 2],
            CommandError::InvalidValue("clipboard"),
        ),
    ];
    for (input, e) in bad {
        assert_eq!(o.s.paste(input, None), Err(e));
    }
    let many: Vec<serde_json::Value> = (0..5_000)
        .map(|i| serde_json::json!({ "key": format!("k{i}"), "label": "K", "color": "#000000" }))
        .collect();
    let flood = serde_json::json!({ "format": "cavea-clipboard", "version": 1,
        "centre": [0, 0], "categories": many, "nodes": [] });
    assert_eq!(
        o.s.paste(&flood.to_string(), None),
        Err(CommandError::Limit("categories"))
    );
    assert_eq!(o.state(), state);

    // A section's child copied alone keeps the category it inherited.
    t.s.select(Level::Section(30), [31]);
    let child = t.s.copy().expect("copy");
    let mut o = T::new(chart());
    o.s.apply(Command::RemoveCategory {
        key: "vip".into(),
        replacement: None,
    })
    .expect("remove");
    o.s.paste(&child, None).expect("paste");
    let [id] = o.selected()[..] else {
        panic!("one node pasted")
    };
    assert_eq!(o.block(id).category.as_deref(), Some("vip"));
    assert!(o.s.document().categories().iter().any(|c| c.key == "vip"));

    // Cut: copy and delete, one entry.
    t.s.select(Level::Top, [42]);
    let entries = t.entries();
    assert!(t.s.cut().expect("cut").is_some());
    assert!(t.s.document().node(42).is_none());
    assert_eq!(t.entries(), entries + 1);
}

/// Every seat position of the document, by oid: its node and world place.
fn census(s: &Session) -> BTreeMap<u32, (u32, [f64; 2])> {
    let ix = s.index();
    let mut out = BTreeMap::new();
    for id in s.document().ids() {
        for seat in ix.seats(id) {
            let p = ix.to_world(id, seat.pos).expect("world");
            out.insert(seat.oid, (id, p));
        }
    }
    out
}

/// The oids an edit may move, remove or renumber, by the selection it acts
/// on: every seat of the selected nodes (and their children), of the
/// selected rows, or the selected seats.
fn touched(s: &Session, all: &BTreeMap<u32, (u32, [f64; 2])>) -> BTreeSet<u32> {
    let sel = s.selection();
    let items = sel.items();
    let doc = s.document();
    match sel.level() {
        Level::Top | Level::Section(_) => all
            .iter()
            .filter(|(_, (node, _))| {
                items.contains(node) || doc.parent(*node).is_some_and(|p| items.contains(&p))
            })
            .map(|(&oid, _)| oid)
            .collect(),
        Level::Rows(b) => match doc.node(b) {
            Some(Node::RowBlock(block)) => block
                .rows
                .iter()
                .filter(|r| items.contains(&r.rid))
                .flat_map(|r| r.oids.iter().flat_map(|o| o[0]..o[0] + o[1]))
                .collect(),
            _ => BTreeSet::new(),
        },
        Level::Seats(_) => items.clone(),
    }
}

/// A random selection: some rows or seats of a block, or some nodes.
fn random_selection(t: &mut T, r: &mut Rng) {
    let doc = t.s.document();
    let nodes: Vec<u32> = doc.ids().collect();
    let pick = r.pick(&nodes).unwrap_or(10);
    match (doc.node(pick), r.below(4)) {
        (Some(Node::RowBlock(b)), 0) => {
            let rids = b.rows.iter().map(|x| x.rid).filter(|_| r.below(2) == 0);
            let rids: Vec<u32> = rids.collect();
            t.s.select(Level::Rows(pick), rids);
        }
        (Some(Node::RowBlock(_) | Node::Row(_)), 1) => {
            let all = t.s.index().seats(pick).iter().map(|x| x.oid);
            let some: Vec<u32> = all.filter(|_| r.below(4) == 0).collect();
            t.s.select(Level::Seats(pick), some);
        }
        _ => {
            let level = doc.parent(pick).map_or(Level::Top, Level::Section);
            let sibs = doc.children(doc.parent(pick)).to_vec();
            let some: Vec<u32> = sibs.into_iter().filter(|_| r.below(3) == 0).collect();
            t.s.select(level, some);
        }
    }
}

/// One random edit of the selection through the session, sometimes with
/// hostile values. False for undo, which restores what earlier edits did.
fn random_edit(t: &mut T, r: &mut Rng) -> bool {
    let at = [r.f(800.0), r.f(800.0)];
    let wild = [f64::NAN, f64::INFINITY, 1e300, -0.0][r.below(4)];
    let end = [End::Start, End::End][r.below(2)];
    let axis = [Axis::Horizontal, Axis::Vertical][r.below(2)];
    let category = ["std", "vip", "zzz"].get(r.below(4)).map(|c| c.to_string());
    let _ = match r.below(22) {
        0 => t.s.row_seats(end, r.below(7) as i32 - 3),
        1 => t.s.shift_rows([27.5, -55.0, wild][r.below(3)]),
        2 => t.s.cut_aisle(at),
        3 => t.s.open_aisle(at, r.below(3) as u32),
        4 => t.s.set_row_label(Some("Q".into())),
        5 => t.s.delete(),
        6 => t.s.edit_seats(SeatEdit::Flag {
            flag: SeatFlag::Accessible,
            on: r.below(2) == 0,
        }),
        7 => t.s.edit_seats(SeatEdit::Remove),
        8 => t.s.edit_seats(SeatEdit::Restore),
        9 => t.s.edit_seats(SeatEdit::Nudge {
            by: [r.f(9.0), [0.0, wild][r.below(2)]],
        }),
        10 => t.s.edit_seats(SeatEdit::Category { category }),
        11 => {
            t.s.align([AlignTo::Left, AlignTo::Middle, AlignTo::Bottom][r.below(3)])
        }
        12 => t.s.distribute(axis),
        13 => t.s.flip(axis),
        14 => t.s.duplicate([r.f(500.0), [0.0, wild][r.below(2)]]),
        15 => {
            let step = ArrayStep::Around {
                centre: at,
                angle: 45.0,
            };
            t.s.array(r.below(4) as u32, step)
        }
        16 => t.s.order([Order::Forward, Order::Back][r.below(2)]),
        17 => {
            let text = t.s.copy().unwrap_or_default();
            let text = match r.below(3) {
                0 => text.replace('1', "\u{1}"),
                _ => text,
            };
            t.s.paste(&text, Some([at[0], [0.0, wild][r.below(2)]]))
        }
        18 => t.s.set_focal_point(Some([wild, 0.0])),
        19 => t.s.set_locked(r.below(4) == 0),
        20 => t.s.cut().map(|_| Applied::default()),
        _ => return t.s.undo().is_err(),
    };
    true
}

/// For random edit sequences through the session: every seat an edit does
/// not touch keeps its oid, its node and its world place; every document
/// reached parses again; an oid new after an edit was never seen before;
/// hostile values are refused without a panic. "Touched" is per selection
/// ([`touched`]): an edit may move, remove or renumber only those.
#[test]
fn untouched_seats_keep_identity() {
    let mut checked = 0;
    for seed in 0..40 {
        let mut r = Rng(seed + 7_000);
        let mut t = T::new(chart());
        let mut ever: BTreeSet<u32> = census(&t.s).keys().copied().collect();
        for _ in 0..60 {
            random_selection(&mut t, &mut r);
            let before = census(&t.s);
            let touched = touched(&t.s, &before);
            if !random_edit(&mut t, &mut r) {
                ever.extend(census(&t.s).keys());
                continue;
            }
            let after = census(&t.s);
            for (oid, (node, p)) in &before {
                if touched.contains(oid) {
                    continue;
                }
                let now = after.get(oid);
                assert!(
                    now.is_some_and(|(n, q)| n == node && close(*p, *q)),
                    "seed {seed}: seat {oid} of {node} at {p:?} is now {now:?}"
                );
                checked += 1;
            }
            for oid in after.keys().filter(|o| !before.contains_key(o)) {
                assert!(ever.insert(*oid), "seed {seed}: oid {oid} reused");
            }
            let doc = t.s.document().save();
            ChartDocument::from_json(&doc.to_json().expect("json")).expect("parses");
        }
    }
    assert!(checked > 10_000, "{checked}");
}

fn codes(doc: &ChartDocument) -> Vec<Code> {
    match compile(doc) {
        Ok(c) => c.findings.iter().map(|f| f.code).collect(),
        Err(e) => panic!("{e}"),
    }
}

fn all_seats(t: &T, id: u32) -> Vec<u32> {
    t.s.index().seats(id).iter().map(|s| s.oid).collect()
}

/// The theatre preset edited as a venue would: two rows shortened, an
/// aisle cut, wheelchair and companion seats marked (after clearing the
/// preset's flags and accessible category, so W106 shows first), a block repainted with the brush, the
/// focal point set, sections aligned, a block duplicated, a section copied
/// to the other side and flipped. It compiles without errors or W106, and
/// every seat of the preset not cut away keeps its oid on its own node.
#[test]
fn edit_a_theatre() {
    let mut t = T::new(
        cavea_testkit::presets::find("theatre")
            .expect("theatre")
            .generate(1),
    );
    let start = census(&t.s);
    // The balcony (1470) is on the second floor: seats are edited on the
    // floor shown.
    for (floor, id) in [(1, 2), (1, 493), (1, 983), (2, 1470)] {
        t.s.set_floor(floor);
        t.s.select(Level::Seats(id), all_seats(&t, id));
        t.s.edit_seats(SeatEdit::Flag {
            flag: SeatFlag::Accessible,
            on: false,
        })
        .expect("clear");
        // The preset's wheelchair seats are also in the accessible category.
        t.s.edit_seats(SeatEdit::Category { category: None })
            .expect("inherit");
    }
    t.s.set_floor(1);
    assert!(codes(&t.s.document().save()).contains(&Code::W106));

    // Shorten the two front rows of the left stalls by two seats.
    let b = t.block(2);
    let cut: Vec<u32> = [0, 1]
        .iter()
        .flat_map(|&r| oids(&b, r)[oids(&b, r).len() - 2..].to_vec())
        .collect();
    t.s.select(Level::Rows(2), [b.rows[0].rid, b.rows[1].rid]);
    t.s.row_seats(End::End, -2).expect("shorten");
    // Cut an aisle through the centre block at its front row's middle seat.
    let b = t.block(493);
    let front = oids(&b, 0);
    let at = t.seats(493)[&front[front.len() / 2]];
    t.s.select(Level::Rows(493), b.rows.iter().map(|r| r.rid));
    t.s.cut_aisle(at).expect("aisle");
    assert!(
        t.removed(493).len() >= b.rows.len() / 2,
        "{:?}",
        t.removed(493)
    );
    // Wheelchair spaces and companions on the centre block's front row.
    t.s.select(Level::Seats(493), front[..2].iter().copied());
    t.s.edit_seats(SeatEdit::Flag {
        flag: SeatFlag::Accessible,
        on: true,
    })
    .expect("wheelchair");
    t.s.select(Level::Seats(493), [front[2]]);
    t.s.edit_seats(SeatEdit::Flag {
        flag: SeatFlag::Companion,
        on: true,
    })
    .expect("companion");
    // Repaint the right stalls' block with the brush, inside its section.
    t.s.select(Level::Section(982), []);
    t.s.set_tool(ToolId::Brush);
    t.s.set_category(Some("premium".into()));
    let p = t.seats(983)[&oids(&t.block(983), 3)[4]];
    t.drag(&[p]);
    assert_eq!(t.block(983).category.as_deref(), Some("premium"));
    // The focal point at the stage front.
    t.s.set_tool(ToolId::Focal);
    t.drag(&[[0.0, -150.0]]);
    assert_eq!(t.s.document().floors()[0].focal_point, Some([0.0, -150.0]));
    t.s.set_tool(ToolId::Select);
    // Align the left and right stalls' bottoms; duplicate the left block
    // behind the stalls with its rows relabelled.
    t.s.select(Level::Top, [1, 982]);
    t.s.align(AlignTo::Bottom).expect("align");
    t.s.select(Level::Section(1), [2]);
    let copy = t.s.duplicate([0.0, 3_000.0]).expect("duplicate").created[0];
    let mut labels = t.block(copy).row_labels;
    labels.prefix = "X".into();
    let property = Property::RowLabels(Box::new(labels));
    t.s.apply(Command::SetProperty { id: copy, property })
        .expect("relabel");
    // Copy the left stalls section to the far side, relabel and flip it.
    t.s.select(Level::Top, [1]);
    let text = t.s.copy().expect("copy");
    let pasted =
        t.s.paste(&text, Some([6_000.0, 0.0]))
            .expect("paste")
            .created[0];
    t.s.apply(Command::SetProperty {
        id: pasted,
        property: Property::Label(Some("Stalls Far".into())),
    })
    .expect("label");
    t.s.flip(Axis::Horizontal).expect("flip");

    let doc = t.s.document().save();
    let found = codes(&doc);
    assert!(!found.contains(&Code::W106), "{found:?}");
    let now = census(&t.s);
    for (oid, (node, _)) in &start {
        if !cut.contains(oid) {
            assert_eq!(now.get(oid).map(|n| n.0), Some(*node), "seat {oid}");
        }
    }
    assert!(cut.iter().all(|o| !now.contains_key(o)));
}

fn ms(t: std::time::Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// Timings on `stadium-100k`, each with its undo: run with
/// `cargo test --release -p cavea-edit measure_ -- --ignored --nocapture`.
#[test]
#[ignore]
fn measure_editing() {
    let doc = cavea_testkit::presets::find("stadium-100k")
        .expect("preset")
        .generate(1);
    let mut t = T::new(doc);
    let blocks: Vec<u32> =
        t.s.document()
            .ids()
            .filter(|&id| matches!(t.s.document().node(id), Some(Node::RowBlock(_))))
            .collect();
    let timed = |t: &mut T, what: &str, f: &mut dyn FnMut(&mut T)| {
        let a = std::time::Instant::now();
        f(t);
        let apply = ms(a);
        let a = std::time::Instant::now();
        t.s.undo().expect("undo");
        println!("{what}: {apply:.3} ms, undo {:.3} ms", ms(a));
    };
    // The largest block, grown to 5,000 seats; 2,000 of them flagged.
    let mut by_size: Vec<(usize, u32)> = blocks
        .iter()
        .map(|&b| (all_seats(&t, b).len(), b))
        .collect();
    by_size.sort_unstable();
    let big = by_size.last().map_or(0, |b| b.1);
    let b = t.block(big);
    let seats = b.rows.last().map_or(1, |r| r.seats);
    let more = (5_000 - all_seats(&t, big).len() as u32).div_ceil(seats);
    let index = b.rows.len() as u32;
    t.s.apply(Command::AddRows {
        id: big,
        index,
        count: more,
        seats,
    })
    .expect("grow");
    let picked: Vec<u32> = all_seats(&t, big).into_iter().take(2_000).collect();
    t.s.select(Level::Seats(big), picked.clone());
    timed(&mut t, &format!("flag {} seats", picked.len()), &mut |t| {
        let edit = SeatEdit::Flag {
            flag: SeatFlag::Accessible,
            on: true,
        };
        t.s.edit_seats(edit).expect("flag");
    });
    // A brush stroke along 500 seats, row by row, in a category the block
    // does not have: the time per pointer move.
    t.s.set_tool(ToolId::Brush);
    let own = t.block(big).category.or_else(|| {
        let parent = t.s.document().parent(big)?;
        match t.s.document().node(parent) {
            Some(Node::Section(s)) => s.category.clone(),
            _ => None,
        }
    });
    let other =
        t.s.document()
            .categories()
            .iter()
            .find(|c| Some(&c.key) != own.as_ref());
    t.s.set_category(other.map(|c| c.key.clone()));
    let seats = t.seats(big);
    let path: Vec<[f64; 2]> = picked.iter().take(500).map(|o| seats[o]).collect();
    let moves = path.len() as f64;
    let a = std::time::Instant::now();
    t.drag(&path);
    let stroke = ms(a);
    let painted = t.block(big).seat_overrides.len();
    let a = std::time::Instant::now();
    t.s.undo().expect("undo");
    println!(
        "brush over {} seats ({painted} overrides): {:.4} ms per move, undo {:.3} ms",
        path.len(),
        stroke / moves,
        ms(a)
    );
    t.s.set_tool(ToolId::Select);
    // 50 blocks aligned (top level: their sections), a 5,000-seat block
    // duplicated, a 20,000-seat set of sections copied and pasted.
    let roots: Vec<u32> =
        t.s.document()
            .children(None)
            .iter()
            .copied()
            .take(50)
            .collect();
    t.s.select(Level::Top, roots);
    timed(&mut t, "align 50 sections", &mut |t| {
        drop(t.s.align(AlignTo::Left))
    });
    let level =
        t.s.document()
            .parent(big)
            .map_or(Level::Top, Level::Section);
    t.s.select(level, [big]);
    let what = format!("duplicate a {}-seat block", all_seats(&t, big).len());
    timed(&mut t, &what, &mut |t| {
        t.s.duplicate([0.0, 10_000.0]).expect("duplicate");
    });
    let mut picked = Vec::new();
    let mut n = 0;
    for &id in t.s.document().children(None) {
        if n >= 20_000 {
            break;
        }
        let kids = t.s.document().children(Some(id));
        n += kids.iter().map(|&k| all_seats(&t, k).len()).sum::<usize>();
        picked.push(id);
    }
    t.s.select(Level::Top, picked);
    let a = std::time::Instant::now();
    let text = t.s.copy().expect("copy");
    println!("copy {n} seats: {:.3} ms, {} bytes", ms(a), text.len());
    timed(&mut t, &format!("paste {n} seats"), &mut |t| {
        t.s.paste(&text, Some([0.0, 50_000.0])).expect("paste");
    });
}
