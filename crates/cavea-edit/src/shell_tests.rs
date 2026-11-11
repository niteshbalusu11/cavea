//! The shell's views and calls (07 §2, §3, §5.3, §12, §14): changed views
//! never missing a change, the layers window against a full walk,
//! reparenting keeping seats still, inspector limits, scrubbing, the curve
//! handle, import refusals, the hand tool; undo exactness for each.

use std::collections::BTreeMap;

use cavea_compiler::ChartDocument;
use cavea_compiler::document::{Align, Curve, Node};

use crate::edit_tests::{NONE, T, chart, random_edit, random_selection};
use crate::tests::Rng;
use crate::*;

fn state(s: &Session) -> Vec<u8> {
    s.document().snapshot().to_json().expect("json")
}

/// Every view as text, by name.
fn read(s: &mut Session, view: ViewName) -> String {
    match view {
        ViewName::History => format!("{:?}", s.history()),
        ViewName::Title => s.title().to_owned(),
        ViewName::Floors => format!("{:?}", s.floors()),
        ViewName::Status => format!("{:?}", s.status()),
        ViewName::Selection => format!("{:?} {:?}", s.selection_view(), s.fit()),
        ViewName::Inspector => format!("{:?}", s.inspector()),
        ViewName::Layers => format!("{:?}", s.layers(0, u32::MAX)),
        ViewName::Categories => format!("{:?}", s.categories()),
        ViewName::Numbering => format!("{:?} {:?}", s.numbering(), s.numbering_preview()),
        ViewName::Validation => format!("{:?}", s.validation()),
        ViewName::Overlay => format!("{:?}", s.overlay()),
        ViewName::Cursor => format!("{:?}", s.cursor()),
        ViewName::Tool => format!("{:?}", s.tool_view()),
        ViewName::Guides => format!("{:?} {:?}", s.guides(), s.saved_guides()),
    }
}

/// One random call of the shell's own.
fn random_shell_call(t: &mut T, r: &mut Rng) {
    let ids: Vec<u32> = t.s.document().ids().collect();
    let id = r.pick(&ids).unwrap_or(10);
    let pitch = [40.0, 75.5, 0.5, f64::NAN][r.below(4)];
    let _ = match r.below(16) {
        0 => t.s.set_field(&Field::SeatPitch(pitch)).map(drop),
        1 => t.s.set_field(&Field::X(r.f(500.0))).map(drop),
        2 => t.s.set_field(&Field::Rows(r.below(6) as u32)).map(drop),
        3 => t.s.begin_scrub(),
        4 => t.s.set_field(&Field::RowPitch(80.0 + r.f(20.0))).map(drop),
        5 => {
            t.s.end_scrub();
            Ok(())
        }
        6 => {
            t.s.cancel_scrub();
            Ok(())
        }
        7 => {
            let how = [LayerClick::Replace, LayerClick::Toggle, LayerClick::Range][r.below(3)];
            t.s.click_layer(id, how);
            Ok(())
        }
        8 => {
            t.s.expand_layer(30, r.below(2) == 0);
            Ok(())
        }
        9 => {
            let drop_at = [Drop::Before(id), Drop::After(id), Drop::Into(30)][r.below(3)];
            let moved = [r.pick(&ids).unwrap_or(40)];
            t.s.move_layers(&moved, drop_at).map(drop)
        }
        10 => t.s.lock_layer(id, r.below(2) == 0).map(drop),
        11 => {
            t.s.validate(r.below(3_000) as u64);
            Ok(())
        }
        12 => {
            t.s.add_guide(UserGuide {
                axis: GuideAxis::Vertical,
                at: r.f(300.0),
            })
            .map(drop)
        }
        13 => {
            t.s.set_tool([ToolId::Select, ToolId::Hand, ToolId::RowBlock][r.below(3)]);
            Ok(())
        }
        14 => {
            let at = [r.f(600.0), r.f(600.0)];
            t.drag(&[at, [at[0] + r.f(80.0), at[1] + r.f(80.0)]]);
            Ok(())
        }
        _ => {
            let k = Key::Char(' ');
            match r.below(2) {
                0 => drop(t.s.input(Input::KeyDown(k, NONE))),
                _ => drop(t.s.input(Input::KeyUp(k, NONE))),
            }
            Ok(())
        }
    };
}

/// For random calls, reading only the views `changed` names after each
/// gives what reading every view gives (07 §14: nothing is missed).
#[test]
fn changed_views_miss_nothing() {
    let mut flagged = 0usize;
    let mut quiet = 0usize;
    for seed in 0..12 {
        let mut r = Rng(seed + 31_000);
        let mut t = T::new(chart());
        let mut cache: BTreeMap<ViewName, String> = BTreeMap::new();
        let first = t.s.changed();
        assert_eq!(first, VIEWS.to_vec(), "every view the first time");
        for &v in &VIEWS {
            cache.insert(v, read(&mut t.s, v));
        }
        for _ in 0..120 {
            match r.below(3) {
                0 => random_selection(&mut t, &mut r),
                1 => drop(random_edit(&mut t, &mut r)),
                _ => random_shell_call(&mut t, &mut r),
            }
            let changed = t.s.changed();
            flagged += changed.len();
            quiet += VIEWS.len() - changed.len();
            for v in changed {
                cache.insert(v, read(&mut t.s, v));
            }
            for &v in &VIEWS {
                let now = read(&mut t.s, v);
                assert_eq!(
                    cache.get(&v),
                    Some(&now),
                    "seed {seed}: {v:?} changed unreported"
                );
            }
            // Asked again at once, nothing has changed.
            assert_eq!(t.s.changed(), Vec::new());
        }
    }
    assert!(quiet > flagged, "{quiet} quiet, {flagged} flagged");
}

/// 13 §9 for the new calls: each inspector set, layers move, lock or
/// drag that leaves an undo entry undoes to the document before it, byte
/// for byte, and redoes to the one after.
#[test]
fn new_calls_undo_exactly() {
    let mut checked = 0;
    for seed in 0..20 {
        let mut r = Rng(seed + 51_000);
        let mut t = T::new(chart());
        for _ in 0..80 {
            random_selection(&mut t, &mut r);
            let before = state(&t.s);
            let entries = t.s.editor().undo_len();
            random_shell_call(&mut t, &mut r);
            if t.s.editor().gesture_open() || t.s.editor().undo_len() <= entries {
                continue;
            }
            let after = state(&t.s);
            t.s.undo().expect("undo");
            assert_eq!(state(&t.s), before, "seed {seed}");
            t.s.redo().expect("redo");
            assert_eq!(state(&t.s), after, "seed {seed}");
            checked += 1;
        }
    }
    assert!(checked > 200, "{checked}");
}

/// A chart of `n` top-level nodes, every fourth a section of up to 5
/// children, on two floors.
fn tree(r: &mut Rng, n: u32) -> ChartDocument {
    let mut nodes = Vec::new();
    let mut id = 1u32;
    for _ in 0..n {
        let floor = [1, 2][r.below(2)];
        id += 1;
        if r.below(4) == 0 {
            let section = id;
            let kids: Vec<_> = (0..r.below(6))
                .map(|_| {
                    id += 1;
                    serde_json::json!({ "id": id, "type": "text", "text": "x", "size": 10, "color": "#000000" })
                })
                .collect();
            nodes.push(serde_json::json!({ "id": section, "type": "section", "floor": floor, "children": kids }));
        } else {
            nodes.push(serde_json::json!({ "id": id, "type": "shape", "kind": "rect", "size": [10, 10], "floor": floor, "name": format!("n{id}") }));
        }
    }
    serde_json::from_value(serde_json::json!({
        "schema": 1, "name": "t", "nextOid": id + 1,
        "floors": [{ "id": 1, "name": "A" }, { "id": 2, "name": "B" }],
        "nodes": nodes,
    }))
    .expect("tree")
}

/// The tree as a full walk lists it: (depth, id).
fn walk(s: &Session, open: &std::collections::BTreeSet<u32>) -> Vec<(u8, u32)> {
    let doc = s.document();
    let mut out = Vec::new();
    for &id in doc.children(None) {
        let node = serde_json::to_value(doc.node(id)).expect("node");
        let floor = node["floor"].as_u64().map(|f| f as u32);
        if floor.or(Some(1)) != s.floor() {
            continue;
        }
        out.push((0, id));
        if open.contains(&id) {
            out.extend(doc.children(Some(id)).iter().map(|&c| (1, c)));
        }
    }
    out
}

/// Any window of the layers tree is that slice of a full walk, for random
/// trees, expansions and floors, before and after moves (07 §14).
#[test]
fn layers_window_is_a_slice_of_the_walk() {
    for seed in 0..30 {
        let mut r = Rng(seed + 41_000);
        let n = 1 + r.below(60) as u32;
        let mut s = Session::load(tree(&mut r, n)).expect("load");
        let mut open = std::collections::BTreeSet::new();
        for _ in 0..40 {
            let ids: Vec<u32> = s.document().ids().collect();
            let id = r.pick(&ids).unwrap_or(1);
            match r.below(5) {
                0 => {
                    let on = r.below(2) == 0;
                    s.expand_layer(id, on);
                    if matches!(s.document().node(id), Some(Node::Section(_))) {
                        if on {
                            open.insert(id)
                        } else {
                            open.remove(&id)
                        };
                    }
                }
                1 => drop(s.set_floor(1 + r.below(2) as u32)),
                2 => {
                    let to = r.pick(&ids).unwrap_or(1);
                    let at = [Drop::Before(to), Drop::After(to), Drop::Into(to)][r.below(3)];
                    let _ = s.move_layers(&[id], at);
                }
                3 => {
                    if let Some(row) = s.reveal_layer(id) {
                        let p = s.document().parent(id);
                        open.extend(p);
                        let page = s.layers(row, 1);
                        assert_eq!(page.rows.first().map(|x| x.id), Some(id), "seed {seed}");
                    }
                }
                _ => drop(s.undo()),
            }
            let full = walk(&s, &open);
            let total = full.len() as u32;
            for _ in 0..4 {
                let start = r.below(full.len() + 3) as u32;
                let count = r.below(12) as u32;
                let page = s.layers(start, count);
                assert_eq!(page.total, total, "seed {seed}");
                let got: Vec<(u8, u32)> = page.rows.iter().map(|x| (x.depth, x.id)).collect();
                let lo = (start as usize).min(full.len());
                let hi = (start as usize + count as usize).min(full.len());
                assert_eq!(got, full[lo..hi].to_vec(), "seed {seed}: {start}+{count}");
                let index: Vec<u32> = page.rows.iter().map(|x| x.index).collect();
                assert_eq!(index, (lo as u32..hi as u32).collect::<Vec<_>>());
            }
        }
    }
}

/// World positions of every seat, by oid.
fn seats(s: &Session) -> BTreeMap<u32, [f64; 2]> {
    let ix = s.index();
    let mut out = BTreeMap::new();
    for id in s.document().ids() {
        for seat in ix.seats(id) {
            out.insert(seat.oid, ix.to_world(id, seat.pos).expect("world"));
        }
    }
    out
}

/// Moves in the layers panel between turned sections and the top level
/// never move a seat, refuse what the structure rule forbids with nothing
/// changed, select what moved, and undo byte for byte.
#[test]
fn reparenting_keeps_seats_still() {
    let mut doc = chart();
    let extra = serde_json::json!({ "id": 60, "type": "section", "label": "Turned",
        "origin": [-900.0, 300.0], "rotation": 37.5, "children": [] });
    doc.nodes
        .push(serde_json::from_value(extra).expect("section"));
    doc.next_oid = 600;
    let mut s = Session::load(doc).expect("load");
    let rot = Command::TransformNodes {
        nodes: vec![NodeTransform {
            id: 30,
            origin: [1000.0, 0.0],
            rotation: -71.25,
        }],
    };
    s.apply(rot).expect("turn");
    let start = seats(&s);
    let mut r = Rng(5);
    let mut moved = 0;
    for _ in 0..300 {
        let ids: Vec<u32> = s.document().ids().collect();
        let picks: Vec<u32> = (0..1 + r.below(3)).filter_map(|_| r.pick(&ids)).collect();
        let to = r.pick(&ids).unwrap_or(10);
        let at = [
            Drop::Before(to),
            Drop::After(to),
            Drop::Into([30, 60, to][r.below(3)]),
        ][r.below(3)];
        let before = state(&s);
        let entries = s.editor().undo_len();
        match s.move_layers(&picks, at) {
            Ok(_) => {
                moved += 1;
                for (oid, p) in seats(&s) {
                    let q = start[&oid];
                    assert!(
                        (p[0] - q[0]).abs() < 1e-6 && (p[1] - q[1]).abs() < 1e-6,
                        "{oid}"
                    );
                }
                let mut sorted = picks.clone();
                sorted.sort_unstable();
                sorted.dedup();
                let sel: Vec<u32> = s.selection().items().iter().copied().collect();
                assert_eq!(sel, sorted);
                if r.below(3) == 0 {
                    let after = state(&s);
                    s.undo().expect("undo");
                    assert_eq!(state(&s), before);
                    s.redo().expect("redo");
                    assert_eq!(state(&s), after);
                }
            }
            Err(_) => {
                assert_eq!(state(&s), before);
                assert_eq!(s.editor().undo_len(), entries);
            }
        }
        // Sections stay top-level, and nothing else holds children.
        let doc = s.document();
        for id in doc.ids() {
            if !doc.children(Some(id)).is_empty() {
                assert!(doc.parent(id).is_none() && matches!(doc.node(id), Some(Node::Section(_))));
            }
        }
    }
    assert!(moved > 100, "{moved}");
    let e = s.move_layers(&[60], Drop::Into(30));
    assert_eq!(e, Err(CommandError::Placement));
    assert_eq!(
        s.move_layers(&[10], Drop::Into(40)),
        Err(CommandError::NotASection(40))
    );
    assert_eq!(
        s.move_layers(&[10, 40], Drop::Before(40)),
        Err(CommandError::InvalidValue("target"))
    );
}

/// Out-of-limit values and fields a node lacks are refused with nothing
/// changed; accepted ones are one undo entry each, named by the field, and
/// undo byte for byte.
#[test]
fn inspector_sets_and_refusals() {
    let mut s = Session::load(chart()).expect("load");
    s.select(Level::Top, [10]);
    let view = s.inspector();
    let b = view.row_block.as_ref().expect("block fields");
    assert_eq!(b.rows.value, Shared::Same(3.0));
    assert_eq!((b.seat_pitch.min, b.seat_pitch.unit), (1.0, Unit::Cu));
    let refused = [
        Field::SeatPitch(0.0),
        Field::SeatPitch(f64::NAN),
        Field::RowPitch(1e9),
        Field::Stagger(1.5),
        Field::Skew(90.0),
        Field::Rows(0),
        Field::Seats(4_096),
        Field::Label(Some("x".repeat(65))),
        Field::Category(Some("nope".into())),
        Field::Floor(Some(9)),
        Field::Curve(Curve::Arc { radius: 0.2 }),
        // Half a turn from the apex is the most an arc takes (03 §2.4).
        Field::Curve(Curve::Arc { radius: 30.0 }),
        Field::TextSize(12.0),
        Field::Fill(Some("red".into())),
        Field::ChartName(String::new()),
        Field::SeatSize(0.1),
    ];
    for field in refused {
        let before = state(&s);
        let entries = s.editor().undo_len();
        assert!(s.set_field(&field).is_err(), "{field:?}");
        assert_eq!(state(&s), before, "{field:?}");
        assert_eq!(s.editor().undo_len(), entries);
    }
    s.select(Level::Top, [10, 40]);
    assert_eq!(
        s.set_field(&Field::SeatPitch(60.0)),
        Err(CommandError::NotApplicable {
            id: 40,
            field: "seatPitch"
        })
    );
    let accepted = [
        (vec![10], Field::Rows(5)),
        (vec![10], Field::Rows(2)),
        (vec![10], Field::Seats(9)),
        (vec![10], Field::SeatPitch(57.5)),
        (vec![10], Field::Curve(Curve::Arc { radius: 800.0 })),
        (vec![10], Field::Curve(Curve::Bulge { amount: -30.0 })),
        (vec![10], Field::Skew(12.0)),
        (vec![10, 40, 41], Field::Rotation(270.0)),
        (vec![10, 40], Field::X(-25.0)),
        (vec![40], Field::Width(333.0)),
        (vec![40], Field::Fill(Some("#123456".into()))),
        (vec![41], Field::Text("EXIT".into())),
        (vec![41], Field::Weight(700)),
        (vec![30], Field::Priority(2.5)),
        (vec![30], Field::Entrance(Some("Door 3".into()))),
        (
            vec![],
            Field::SeatShape(cavea_compiler::document::SeatShape::Chair),
        ),
        (vec![], Field::ChartName("Hall".into())),
    ];
    for (ids, field) in accepted {
        s.select(Level::Top, ids.clone());
        let before = state(&s);
        s.set_field(&field)
            .unwrap_or_else(|e| panic!("{field:?}: {e}"));
        assert_eq!(s.history().undo, Some(field.key()));
        let after = state(&s);
        assert_ne!(after, before, "{field:?}");
        s.undo().expect("undo");
        assert_eq!(state(&s), before, "undo {field:?}");
        s.redo().expect("redo");
        assert_eq!(state(&s), after, "redo {field:?}");
    }
    let one = |s: &Session| match s.document().node(10) {
        Some(Node::RowBlock(b)) => (
            b.rows.len(),
            b.rows.iter().map(|r| r.seats).collect::<Vec<_>>(),
        ),
        _ => (0, Vec::new()),
    };
    assert_eq!(one(&s), (2, vec![9, 9]));
    // A section with the block: no row block fields, the common ones.
    s.select(Level::Top, [10, 30]);
    let view = s.inspector();
    assert!(view.row_block.is_none() && view.common.is_some());
    // Nothing selected: the chart's settings.
    s.select(Level::Top, []);
    let view = s.inspector();
    assert_eq!(view.chart.as_ref().map(|c| c.name.as_str()), Some("Hall"));
    assert!(view.common.is_none());
}

/// A scrub is one undo entry whatever it showed, holds exactly the last
/// value, keeps the last good value when one is refused, and Escape
/// restores the start without an entry.
#[test]
fn scrubbing_is_one_entry() {
    let mut s = Session::load(chart()).expect("load");
    s.select(Level::Top, [10]);
    let start = state(&s);
    let entries = s.editor().undo_len();
    s.begin_scrub().expect("scrub");
    for v in [60.0, 70.0, 80.0, 1e9, 90.0] {
        let _ = s.set_field(&Field::RowPitch(v));
    }
    s.set_field(&Field::Rows(6)).expect("rows in the scrub");
    s.set_field(&Field::Rows(4)).expect("rows in the scrub");
    assert!(s.set_field(&Field::Rows(0)).is_err());
    assert!(s.history().gesture);
    s.end_scrub();
    assert_eq!(s.editor().undo_len(), entries + 1);
    let Some(Node::RowBlock(b)) = s.document().node(10) else {
        panic!("block")
    };
    // The last call shown replaced the earlier ones: rows 4, pitch as at the start.
    assert_eq!((b.rows.len(), b.row_pitch), (4, 100.0));
    s.undo().expect("undo");
    assert_eq!(state(&s), start);
    s.begin_scrub().expect("scrub");
    s.set_field(&Field::SeatPitch(33.0)).expect("pitch");
    s.cancel_scrub();
    assert_eq!(state(&s), start);
    assert_eq!(s.editor().undo_len(), entries);
    // Anything else ends the scrub as a release would.
    s.begin_scrub().expect("scrub");
    s.set_field(&Field::SeatPitch(33.0)).expect("pitch");
    s.select(Level::Top, [40]);
    assert!(!s.editor().gesture_open());
    assert_eq!(s.editor().undo_len(), entries + 1);
}

/// The curve handle bends a straight block into an arc whose front row's
/// ends stay put and whose middle follows the pointer, as one entry;
/// Escape restores; a bulge changes its amount.
#[test]
fn curve_handle() {
    let mut t = T::new(chart());
    t.s.select(Level::Top, [10]);
    let handle = |t: &T| {
        let o = t.s.overlay();
        let h = o
            .handles
            .iter()
            .find(|h| h.kind == HandleKind::Curve)
            .copied();
        h.map(|h| [h.at[0] + h.offset[0], h.at[1] + h.offset[1]])
            .expect("curve handle")
    };
    let ends = |t: &T| {
        let ix = t.s.index();
        let front: Vec<[f64; 2]> = ix
            .seats(10)
            .iter()
            .filter(|s| s.row == 0)
            .map(|s| s.pos)
            .collect();
        let w = |p| ix.to_world(10, p).expect("world");
        (w(front[0]), w(front[front.len() - 1]))
    };
    let start = state(&t.s);
    let (l0, r0) = ends(&t);
    let at = handle(&t);
    t.drag(&[at, [at[0], at[1] + 20.0], [at[0], at[1] + 40.0]]);
    let Some(Node::RowBlock(b)) = t.s.document().node(10) else {
        panic!("block")
    };
    let Curve::Arc { radius } = b.curve else {
        panic!("arc: {:?}", b.curve)
    };
    // Six seats 50 cu apart: the middle two at ±25 cu along the arc lie
    // 40 cu behind the ends at ±125.
    let sag = radius * ((25.0 / radius).cos() - (125.0 / radius).cos());
    assert!((sag - 40.0).abs() < 1e-9, "{radius}: {sag}");
    let (l1, r1) = ends(&t);
    assert!(
        (l1[1] - l0[1]).abs() < 1e-6 && (r1[1] - r0[1]).abs() < 1e-6,
        "{l0:?} {l1:?}"
    );
    assert_eq!(t.s.history().undo, Some("curve"));
    assert_eq!(t.s.editor().undo_len(), 1);
    t.s.undo().expect("undo");
    assert_eq!(state(&t.s), start);
    // Escape in the middle of the drag restores.
    let at = handle(&t);
    t.input(Input::PointerDown, at, NONE);
    t.input(Input::PointerMove, [at[0], at[1] + 30.0], NONE);
    t.s.input(Input::KeyDown(Key::Escape, NONE));
    t.input(Input::PointerUp, [at[0], at[1] + 30.0], NONE);
    assert_eq!(state(&t.s), start);
    // A bulge keeps its kind.
    t.s.set_field(&Field::Curve(Curve::Bulge { amount: 10.0 }))
        .expect("bulge");
    let at = handle(&t);
    t.drag(&[at, [at[0], at[1] - 25.0]]);
    let Some(Node::RowBlock(b)) = t.s.document().node(10) else {
        panic!("block")
    };
    // The middle seats (t = 0.4, 0.6) are displaced 0.96 of the amount:
    // from 9.6 cu they follow the pointer 25 cu forward.
    let Curve::Bulge { amount } = b.curve else {
        panic!("bulge: {:?}", b.curve)
    };
    assert!((amount * 0.96 - (9.6 - 25.0)).abs() < 1e-9, "{amount}");
    // Left-aligned with an offset, and a drag past the deepest arc a row
    // can make: the midpoint of the front row's ends stays where it was,
    // and its middle follows the pointer as far as the arc can go.
    let mid = |t: &T| {
        let (a, b) = ends(t);
        (a[1] + b[1]) / 2.0
    };
    for (align, travel, follows) in [
        // An arc's apex is at x = 0, a left or right-aligned row's end, so
        // such a row bends less deeply before its far end is half a turn
        // from the apex: about 26 cu here.
        (Align::Left, 15.0, true),
        (Align::Right, -15.0, true),
        (Align::Left, 60.0, false),
        (Align::Center, 400.0, false),
    ] {
        t.s.set_field(&Field::Curve(Curve::None {}))
            .expect("straight");
        t.s.set_field(&Field::Align(align)).expect("align");
        t.s.select(Level::Rows(10), [11]);
        t.s.shift_rows(30.0).expect("offset");
        t.s.select(Level::Top, [10]);
        let (m0, h0) = (mid(&t), handle(&t));
        t.drag(&[h0, [h0[0], h0[1] + travel]]);
        let Some(Node::RowBlock(b)) = t.s.document().node(10) else {
            panic!("block")
        };
        assert!(matches!(b.curve, Curve::Arc { .. }), "{align:?}");
        assert!((mid(&t) - m0).abs() < 1e-6, "{align:?}: {} {m0}", mid(&t));
        let moved = handle(&t)[1] - h0[1];
        if follows {
            assert!((moved - travel).abs() < 3.0, "{align:?}: {moved}");
        } else {
            assert!(moved > 10.0 && moved < travel, "{align:?}: {moved}");
        }
    }
}

/// On an even front row the handle, between the two middle seats, follows
/// the pointer too; the status bar counts a removed position once.
#[test]
fn curve_handle_even_rows_and_status() {
    let doc = serde_json::json!({ "schema": 1, "name": "e", "nextOid": 100,
        "nodes": [{ "id": 1, "type": "rowBlock", "seatPitch": 100,
            "rows": [{ "rid": 2, "seats": 4, "oids": [[3, 4]] },
                     { "rid": 7, "seats": 3, "oids": [[8, 3]], "removed": [0, 0] }] }] });
    let mut t = T::new(serde_json::from_value(doc).expect("doc"));
    assert_eq!(t.s.status().seats, 4 + 2);
    t.s.select(Level::Top, [1]);
    let handle = |t: &T| {
        let o = t.s.overlay();
        let h = o
            .handles
            .iter()
            .find(|h| h.kind == HandleKind::Curve)
            .copied();
        h.map(|h| h.at[1]).expect("curve handle")
    };
    for curve in [Curve::None {}, Curve::Bulge { amount: 5.0 }] {
        t.s.set_field(&Field::Curve(curve)).expect("curve");
        let y0 = handle(&t);
        let o = t.s.overlay();
        let h = o
            .handles
            .iter()
            .find(|h| h.kind == HandleKind::Curve)
            .copied()
            .expect("handle");
        let at = [h.at[0] + h.offset[0], h.at[1] + h.offset[1]];
        t.drag(&[at, [at[0], at[1] + 80.0]]);
        assert!(
            (handle(&t) - y0 - 80.0).abs() < 1e-6,
            "{curve:?}: {}",
            handle(&t) - y0
        );
    }
}

/// The views `changed` names follow the floor shown and a tool reset.
#[test]
fn changed_follows_floor_and_reset() {
    let mut doc = chart();
    doc.floors.push(
        serde_json::from_value(serde_json::json!({ "id": 2, "name": "Balcony" })).expect("floor"),
    );
    let far = serde_json::json!({ "id": 70, "type": "shape", "kind": "rect", "size": [10, 10],
        "origin": [9000, 9000], "floor": 2 });
    doc.nodes.push(serde_json::from_value(far).expect("node"));
    doc.next_oid = 600;
    let mut t = T::new(doc);
    let fit = t.s.fit();
    let _ = t.s.changed();
    assert!(t.s.set_floor(2));
    assert!(t.s.changed().contains(&ViewName::Selection));
    assert_ne!(t.s.fit(), fit);
    // A box being dragged, then a selection from a panel: the box is gone.
    assert!(t.s.set_floor(1));
    t.input(Input::PointerDown, [-3000.0, -3000.0], NONE);
    t.input(Input::PointerMove, [-2900.0, -2900.0], NONE);
    assert!(t.s.overlay().marquee.is_some());
    let _ = t.s.changed();
    t.s.select(Level::Top, []);
    assert!(t.s.overlay().marquee.is_none());
    assert!(t.s.changed().contains(&ViewName::Overlay));
}

/// Import reads hostile files with typed errors naming what to fix, and
/// export is what import reads back.
#[test]
fn import_and_export() {
    let s = Session::load(chart()).expect("load");
    let text = s.export();
    let again = Session::import(&text).expect("import");
    assert_eq!(state(&again), state(&s));
    assert_eq!(again.export(), text);
    let e = |t: &str| Session::import(t).expect_err("refused");
    assert!(matches!(
        e("{\"schema\": 1,\n \"name\": 5}"),
        ImportError::Json { line: 2, .. }
    ));
    assert_eq!(
        e("{\"schema\": 9, \"name\": \"x\", \"nextOid\": 1}"),
        ImportError::Schema { schema: 9 }
    );
    let nul = "{\"schema\": 1, \"name\": \"a\\u0000\", \"nextOid\": 1}";
    assert_eq!(e(nul), ImportError::NulEscape);
    let big = format!(
        "{{\"schema\": 1, \"name\": \"{}\", \"nextOid\": 1}}",
        "x".repeat(MAX_DOCUMENT)
    );
    assert!(matches!(e(&big), ImportError::TooLarge { .. }));
    let doc: serde_json::Value = serde_json::from_str(&text).expect("json");
    let mut dup = doc.clone();
    dup["nodes"][1]["children"][0]["id"] = 10.into();
    let dup = dup.to_string();
    assert_eq!(e(&dup), ImportError::DuplicateId { id: 10 });
    let mut wide = doc.clone();
    wide["nodes"][0]["rows"][0]["seats"] = 4096.into();
    wide["nodes"][0]["rows"][0]["oids"] = serde_json::json!([[100, 4096]]);
    let wide = wide.to_string();
    assert_eq!(
        e(&wide),
        ImportError::SeatsPerRow {
            node: 10,
            row: 11,
            seats: 4096,
            limit: 4095
        }
    );
    // 245 rows of 4,095 positions: over a million objects in a few KB.
    let rows: Vec<_> = (0..245u32)
        .map(|i| serde_json::json!({ "rid": 10_000 + i * 5_000, "seats": 4095, "oids": [[10_001 + i * 5_000, 4095]] }))
        .collect();
    let huge = serde_json::json!({ "schema": 1, "name": "h", "nextOid": 2_000_000,
        "nodes": [{ "id": 1, "type": "rowBlock", "rows": rows }] });
    assert_eq!(
        e(&huge.to_string()),
        ImportError::Limit {
            what: "objects".into(),
            count: 1_003_275,
            limit: 1_000_000
        }
    );
    // A refusal says enough to fix the file.
    assert_eq!(e(&dup).to_string(), "two nodes have id 10");
}

/// The hand tool and Space pan without touching the document; Space hands
/// the tool back; the middle button pans in any tool.
#[test]
fn hand_tool_and_space() {
    let mut t = T::new(chart());
    let start = state(&t.s);
    t.s.set_tool(ToolId::RowBlock);
    t.s.input(Input::KeyDown(Key::Char(' '), NONE));
    t.s.input(Input::KeyDown(Key::Char(' '), NONE));
    assert_eq!(t.s.tool(), ToolId::Hand);
    assert_eq!(t.s.tool_view().held, Some(ToolId::RowBlock));
    assert_eq!(t.s.cursor(), Cursor::Grab);
    let p = |x: f64, button| Pointer {
        world: [x, 0.0],
        screen: [x, 0.0],
        zoom: 1.0,
        button,
        kind: PointerKind::Mouse,
        mods: NONE,
        time: 0.0,
    };
    t.s.input(Input::PointerDown(p(0.0, Button::Primary)));
    assert_eq!(t.s.cursor(), Cursor::Grabbing);
    assert_eq!(
        t.s.input(Input::PointerMove(p(30.0, Button::Primary))).pan,
        Some([30.0, 0.0])
    );
    assert_eq!(
        t.s.input(Input::PointerUp(p(35.0, Button::Primary))).pan,
        Some([5.0, 0.0])
    );
    t.s.input(Input::KeyUp(Key::Char(' '), NONE));
    assert_eq!(t.s.tool(), ToolId::RowBlock);
    // The middle button pans and draws nothing.
    t.s.input(Input::PointerDown(p(0.0, Button::Middle)));
    assert_eq!(
        t.s.input(Input::PointerMove(p(-10.0, Button::Middle))).pan,
        Some([-10.0, 0.0])
    );
    t.s.input(Input::PointerUp(p(-10.0, Button::Middle)));
    assert_eq!(state(&t.s), start);
    assert_eq!(t.s.editor().undo_len(), 0);
    // Zoom to fit frames every node of the floor shown.
    let fit = t.s.fit().expect("fit");
    assert!(fit.min[0] < -600.0 && fit.max[0] > 1000.0, "{fit:?}");
}

/// Prints the shell calls' timings on `stadium-100k` and on 5,000 single
/// rows: `cargo test --release -p cavea-edit measure_shell -- --ignored
/// --nocapture`.
#[test]
#[ignore]
fn measure_shell() {
    use std::time::Instant;
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
    let doc = cavea_testkit::presets::find("stadium-100k")
        .expect("preset")
        .generate(1);
    let text = doc
        .to_json()
        .map(String::from_utf8)
        .expect("json")
        .expect("utf8");
    let t0 = Instant::now();
    let mut s = Session::import(&text).expect("import");
    let import = ms(t0);
    let t0 = Instant::now();
    let out = s.export();
    println!(
        "stadium-100k ({} B): import {import:.1} ms, export {:.1} ms",
        out.len(),
        ms(t0)
    );
    let _ = s.changed();
    let t0 = Instant::now();
    let quiet = s.changed();
    println!(
        "changed() with nothing changed: {:.4} ms ({quiet:?})",
        ms(t0)
    );
    let blocks: Vec<u32> = s
        .document()
        .ids()
        .filter(|&id| matches!(s.document().node(id), Some(Node::RowBlock(_))))
        .take(180)
        .collect();
    let sections: Vec<u32> = s.document().children(None).to_vec();
    for &id in &sections {
        s.expand_layer(id, true);
    }
    let parent = s.document().parent(blocks[0]).expect("in a section");
    s.select(
        Level::Section(parent),
        s.document().children(Some(parent)).to_vec(),
    );
    let t0 = Instant::now();
    let changed = s.changed();
    println!(
        "changed() after a selection: {:.4} ms, {} views",
        ms(t0),
        changed.len()
    );
    for v in VIEWS {
        let t0 = Instant::now();
        let text = read(&mut s, v);
        println!("  {v:?}: {:.3} ms ({} B as Debug)", ms(t0), text.len());
    }
    let total = s.layers(0, 0).total;
    for start in [0, total / 2, total.saturating_sub(50)] {
        let t0 = Instant::now();
        let page = s.layers(start, 50);
        println!(
            "layers window of {} at {start} of {total}: {:.4} ms",
            page.rows.len(),
            ms(t0)
        );
    }
    // 180 blocks in many sections: the inspector set acts on its selection,
    // so the blocks are moved into one section first (one entry), timed too.
    let t0 = Instant::now();
    let moved: Vec<u32> = blocks.iter().copied().take(100).collect();
    s.move_layers(&moved, Drop::Into(sections[0]))
        .expect("move 100");
    println!("reparent 100 blocks: {:.2} ms", ms(t0));
    let t0 = Instant::now();
    s.undo().expect("undo");
    println!("  undo: {:.2} ms", ms(t0));
    s.move_layers(&blocks, Drop::Into(sections[0]))
        .expect("move 180");
    s.select(Level::Section(sections[0]), blocks.clone());
    let t0 = Instant::now();
    let view = s.inspector();
    println!(
        "inspector view, 180 blocks: {:.3} ms ({:?})",
        ms(t0),
        view.row_block.map(|b| b.rows.value)
    );
    let t0 = Instant::now();
    s.set_field(&Field::SeatPitch(56.0)).expect("set");
    println!("inspector set seatPitch on 180 blocks: {:.2} ms", ms(t0));
    let t0 = Instant::now();
    s.begin_scrub().expect("scrub");
    for k in 0..30 {
        s.set_field(&Field::RowPitch(90.0 + f64::from(k)))
            .expect("scrub");
    }
    s.end_scrub();
    println!(
        "scrub of 30 steps on 180 blocks: {:.2} ms each",
        ms(t0) / 30.0
    );
    // Several thousand single rows at the top level.
    let rows: Vec<_> = (0..5_000u32)
        .map(|i| {
            serde_json::json!({ "id": 10 + i * 20, "type": "row", "seats": 10,
            "origin": [0.0, f64::from(i) * 60.0], "oids": [[11 + i * 20, 10]],
            "path": { "kind": "polyline", "points": [[0, 0], [500, 0]] } })
        })
        .collect();
    let doc = serde_json::json!({ "schema": 1, "name": "rows", "nextOid": 200_000, "nodes": rows });
    let mut s = Session::import(&doc.to_string()).expect("rows");
    let _ = s.changed();
    let total = s.layers(0, 0).total;
    for start in [0, total / 2, total - 50] {
        let t0 = Instant::now();
        let page = s.layers(start, 50);
        println!(
            "5,000 rows: layers window of {} at {start}: {:.4} ms",
            page.rows.len(),
            ms(t0)
        );
    }
    let ids: Vec<u32> = s
        .document()
        .children(None)
        .iter()
        .copied()
        .step_by(50)
        .collect();
    let t0 = Instant::now();
    s.move_layers(&ids, Drop::Before(30)).expect("reorder 100");
    println!("5,000 rows: reorder 100: {:.2} ms", ms(t0));
    let t0 = Instant::now();
    let _ = s.layers(2_500, 50);
    println!(
        "5,000 rows: first window after it (list rebuilt): {:.4} ms",
        ms(t0)
    );
    let t0 = Instant::now();
    let changed = s.changed();
    println!(
        "5,000 rows: changed() after it: {:.4} ms, {} views",
        ms(t0),
        changed.len()
    );
}
