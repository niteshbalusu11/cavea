use std::time::Instant;

use cavea_compiler::ChartDocument;
use cavea_compiler::document::Node;
use cavea_testkit::presets;

use crate::tests::{Rng, command};
use crate::*;

fn extent(index: &SpatialIndex, doc: &Document) -> Rect {
    let mut r = Rect::new([-100.0, -100.0], [100.0, 100.0]);
    for id in doc.ids() {
        if let Some(b) = index.bounds(id) {
            r = Rect::new(
                [r.min[0].min(b.min[0]), r.min[1].min(b.min[1])],
                [r.max[0].max(b.max[0]), r.max[1].max(b.max[1])],
            );
        }
    }
    r
}

fn hidden(doc: &Document, id: u32) -> bool {
    let value = serde_json::to_value(doc.node(id)).expect("json");
    value.get("hidden") == Some(&serde_json::Value::Bool(true))
}

/// Every node at `scope`'s level, found by scanning all nodes, with its
/// place among its siblings.
fn level(doc: &Document, scope: Option<u32>) -> Vec<(usize, u32)> {
    let mut out: Vec<(usize, u32)> = doc
        .ids()
        .filter(|&id| doc.parent(id) == scope && !hidden(doc, id))
        .filter_map(|id| {
            doc.children(scope)
                .iter()
                .position(|&c| c == id)
                .map(|i| (i, id))
        })
        .collect();
    out.sort_unstable();
    out
}

/// The index kept up to date across random edits agrees with one built from
/// scratch, and its queries agree with a brute-force scan of all nodes.
#[test]
fn index_matches_brute_force() {
    let subjects: Vec<ChartDocument> = vec![
        presets::PRESETS[0].generate(2),
        cavea_testkit::synthetic::stadium(2_000, 3),
        ChartDocument::from_json(include_bytes!(
            "../../../cavea-compiler/samples/schema-1/full.json"
        ))
        .expect("full"),
    ];
    for (n, doc) in subjects.into_iter().enumerate() {
        let mut r = Rng(100 + n as u64);
        let mut editor = Editor::load(doc).expect("load");
        let mut index = SpatialIndex::new();
        for step in 0..150 {
            index.sync(editor.document());
            let doc = editor.document();
            let mut fresh = SpatialIndex::new();
            fresh.sync(doc);
            for id in doc.ids() {
                assert_eq!(index.bounds(id), fresh.bounds(id), "step {step} node {id}");
            }
            let area = extent(&fresh, doc);
            let sections: Vec<u32> = doc
                .children(None)
                .iter()
                .copied()
                .filter(|&id| matches!(doc.node(id), Some(Node::Section(_))))
                .collect();
            for _ in 0..20 {
                let scope = if r.below(2) == 0 {
                    None
                } else {
                    r.pick(&sections)
                };
                let point = |r: &mut Rng| {
                    let t = |r: &mut Rng| (r.next() % 1001) as f64 / 1000.0;
                    let (u, v) = (t(r), t(r));
                    [
                        area.min[0] + u * (area.max[0] - area.min[0]),
                        area.min[1] + v * (area.max[1] - area.min[1]),
                    ]
                };
                let p = point(&mut r);
                let rect = Rect::new(p, point(&mut r));
                let candidates = level(doc, scope);
                let expected_hit = candidates
                    .iter()
                    .rev()
                    .find(|&&(_, id)| fresh.bounds(id).is_some_and(|b| b.contains_point(p)))
                    .map(|&(_, id)| id);
                assert_eq!(
                    index.hit(doc, p, scope),
                    expected_hit,
                    "step {step} scope {scope:?} {candidates:?} {:?}",
                    candidates
                        .iter()
                        .map(|&(_, id)| fresh.bounds(id))
                        .collect::<Vec<_>>()
                );
                for mode in [Select::Contain, Select::Touch] {
                    let expected: Vec<u32> = candidates
                        .iter()
                        .filter(|&&(_, id)| {
                            fresh.bounds(id).is_some_and(|b| match mode {
                                Select::Contain => rect.contains(&b),
                                Select::Touch => rect.intersects(&b),
                            })
                        })
                        .map(|&(_, id)| id)
                        .collect();
                    assert_eq!(index.select(doc, rect, scope, mode), expected);
                }
                assert_eq!(index.anchors(rect, &[]), fresh.anchors(rect, &[]));
            }
            // A seat's own position picks it in seat-edit mode.
            let blocks: Vec<u32> = doc
                .ids()
                .filter(|&id| matches!(doc.node(id), Some(Node::RowBlock(_))))
                .collect();
            if let Some(b) = r.pick(&blocks)
                && let Some(seat) = r.pick(index.seats(b))
                && let Some(world) = world(doc, b, seat.pos)
                // Random moves by f64::MAX leave nodes where seats cannot be told apart.
                && world.iter().all(|v| v.abs() < 1e9)
            {
                assert_eq!(index.seat_at(b, world, 1.0), Some(seat.oid));
            }
            let cmd = command(&mut r, editor.document());
            let _ = editor.apply(cmd);
            if r.below(6) == 0 {
                let _ = editor.undo();
            }
        }
    }
}

/// The world position of a point in a node's space, from the node chain.
fn world(doc: &Document, id: u32, p: [f64; 2]) -> Option<[f64; 2]> {
    let mut p = p;
    let mut at = Some(id);
    while let Some(id) = at {
        let (origin, rotation) = match doc.node(id)? {
            Node::Section(n) => (n.origin, n.rotation),
            Node::RowBlock(n) => (n.origin, n.rotation),
            _ => return None,
        };
        let (s, c) = (rotation % 360.0).to_radians().sin_cos();
        p = [
            p[0] * c - p[1] * s + origin[0],
            p[0] * s + p[1] * c + origin[1],
        ];
        at = doc.parent(id);
    }
    Some(p)
}

fn ms(t: Instant, n: u32) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0 / f64::from(n)
}

/// Prints the edit core's timings on `stadium-100k` against the designer
/// budgets (01 §5): run with
/// `cargo test --release -p cavea-edit measure_ -- --ignored --nocapture`.
#[test]
#[ignore]
fn measure_edit() {
    let doc = presets::find("stadium-100k").expect("preset").generate(1);
    let bytes = doc.to_json().expect("json");
    let t = Instant::now();
    let parsed = ChartDocument::from_json(&bytes).expect("parse");
    let parse = ms(t, 1);
    let t = Instant::now();
    let mut editor = Editor::load(parsed).expect("load");
    let load = ms(t, 1);
    let mut index = SpatialIndex::new();
    let t = Instant::now();
    index.sync(editor.document());
    let build = ms(t, 1);
    println!(
        "stadium-100k: {} nodes, {} B JSON; parse {parse:.2} ms, load {load:.2} ms, index {build:.2} ms",
        editor.document().ids().count(),
        bytes.len()
    );

    let blocks: Vec<u32> = editor
        .document()
        .ids()
        .filter(|&id| matches!(editor.document().node(id), Some(Node::RowBlock(_))))
        .collect();
    let seats = |ids: &[u32], index: &SpatialIndex| -> usize {
        ids.iter().map(|&id| index.seats(id).len()).sum()
    };
    let step = |editor: &mut Editor, index: &mut SpatialIndex, f: &dyn Fn(&mut Editor)| {
        let t = Instant::now();
        f(editor);
        index.sync(editor.document());
        ms(t, 1)
    };
    let target = blocks[0];
    let set = step(&mut editor, &mut index, &|e| {
        let property = Property::SeatPitch(50.0);
        e.apply(Command::SetProperty {
            id: target,
            property,
        })
        .expect("set");
    });
    let undo = step(&mut editor, &mut index, &|e| drop(e.undo()));
    let redo = step(&mut editor, &mut index, &|e| drop(e.redo()));
    println!(
        "seat pitch of one block ({} seats), apply + index: {set:.3} ms; undo {undo:.3} ms; redo {redo:.3} ms",
        index.seats(target).len()
    );

    for count in [15, 50] {
        let ids: Vec<u32> = blocks.iter().copied().take(count).collect();
        editor.begin_gesture().expect("begin");
        let frames = 120;
        let t = Instant::now();
        for i in 0..frames {
            let delta = [1.0, if i % 2 == 0 { 0.5 } else { -0.5 }];
            editor
                .apply(Command::MoveNodes {
                    ids: ids.clone(),
                    delta,
                })
                .expect("move");
            index.sync(editor.document());
        }
        let frame = ms(t, frames);
        editor.end_gesture().expect("end");
        let undo = step(&mut editor, &mut index, &|e| drop(e.undo()));
        let redo = step(&mut editor, &mut index, &|e| drop(e.redo()));
        println!(
            "drag {count} blocks ({} seats), per frame apply + index: {frame:.3} ms; undo {undo:.3} ms; redo {redo:.3} ms",
            seats(&ids, &index)
        );
    }

    let doc = editor.document();
    let area = extent(&index, doc);
    let mid = [
        (area.min[0] + area.max[0]) / 2.0,
        (area.min[1] + area.max[1]) / 2.0,
    ];
    let n = 1_000;
    let t = Instant::now();
    let mut found = 0;
    for i in 0..n {
        let d = f64::from(i);
        let rect = Rect::new([area.min[0] + d, area.min[1]], mid);
        found += index.select(doc, rect, None, Select::Touch).len();
    }
    let select = ms(t, n);
    let t = Instant::now();
    for i in 0..n {
        let p = [area.min[0] + f64::from(i) * 10.0, mid[1]];
        found += usize::from(index.hit(doc, p, None).is_some());
    }
    let hit = ms(t, n);
    let t = Instant::now();
    for i in 0..n {
        let rect = Rect::new(
            [mid[0] + f64::from(i), mid[1]],
            [mid[0] + 3000.0, mid[1] + 3000.0],
        );
        found += index.anchors(rect, &[]).len();
    }
    let anchors = ms(t, n);
    println!(
        "box select {:.4} ms, point {:.4} ms, anchors in 30 m square {:.4} ms ({found})",
        select, hit, anchors
    );
}
