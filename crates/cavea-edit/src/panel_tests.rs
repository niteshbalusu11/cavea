//! The edit core behind the numbering, categories and validation panels and
//! the rulers' guides (`docs/07-designer.md` §5.2, §6, §7, §10).

use std::collections::{BTreeMap, BTreeSet};

use cavea_compiler::document::{
    AlphaOverflow, Category, LabelScheme, LabelStart, Node, RowDirection, RowLabels, SeatDirection,
    SeatLabels,
};
use cavea_compiler::{ChartDocument, CompileError, Detail, Finding, MAX_FINDINGS, compile};
use cavea_testkit::presets::PRESETS;

use crate::editor::Editor;
use crate::index::SpatialIndex;
use crate::selection::Selection;
use crate::tests::{Rng, command, subjects};
use crate::tool::snap::{self, Moving, Snap};
use crate::tool::{Context, Pointer};
use crate::validation::{self, local};
use crate::*;

fn state(s: &Session) -> Vec<u8> {
    s.document().snapshot().to_json().expect("json")
}

fn findings(doc: &ChartDocument) -> Vec<Finding> {
    match compile(doc) {
        Ok(c) => c.findings,
        Err(CompileError::Invalid(f)) => f,
        Err(e) => panic!("{e}"),
    }
}

type FindingKey = (String, Option<u32>, Option<u32>, String);

fn keys<'a>(f: impl Iterator<Item = &'a Finding>) -> Vec<FindingKey> {
    let mut out: Vec<FindingKey> = f
        .map(|f| (f.code.as_str().into(), f.node, f.related(), f.message()))
        .collect();
    out.sort();
    out
}

/// DSN-038: for seeded random edit sequences, after the units alone are
/// compiled, the errors they give are exactly the whole chart's compile's
/// errors of those kinds (as multisets); after the full pass, the list is
/// exactly the compile's, and publish is allowed exactly when it has no
/// error. Passes are interrupted by edits at random.
#[test]
fn validation_is_the_compilers() {
    let (mut local_checked, mut full_checked, mut errors_seen, mut warnings_seen) = (0, 0, 0, 0);
    for (n, start) in subjects().into_iter().enumerate() {
        let mut r = Rng(n as u64 + 301);
        let mut s = Session::load(start.clone()).expect("load");
        for i in 0..150 {
            // Back to the chart as loaded now and then: a chart without
            // errors is where warnings show.
            if i % 15 == 0 {
                s = Session::load(start.clone()).expect("load");
            }
            let cmd = command(&mut r, s.document());
            let _ = s.apply(cmd);
            match r.below(4) {
                // Leave the pass half done; the next edit lands on it.
                0 => {
                    s.validate(1 + r.below(3_000) as u64);
                    continue;
                }
                1 => continue,
                _ => {}
            }
            while !s.validator().units_done() {
                s.validate(1);
            }
            let reference = findings(&s.document().snapshot());
            // Past a chart-wide limit the compiler skips nodes a unit checks.
            if reference.len() < MAX_FINDINGS && !reference.iter().any(validation::limit) {
                let theirs = keys(reference.iter().filter(|f| local(f)));
                assert_eq!(keys(s.validator().local_findings()), theirs, "subject {n}");
                local_checked += 1;
            }
            let p = s.validate(u64::MAX);
            assert!(p.done && p.next == 0);
            let view = s.validation();
            assert!(view.complete);
            let ours: Vec<FindingKey> = {
                let mut v: Vec<FindingKey> = view
                    .findings
                    .iter()
                    .map(|f| (f.code.into(), f.node, f.related, f.message.clone()))
                    .collect();
                v.sort();
                v
            };
            assert_eq!(ours, keys(reference.iter()), "subject {n}");
            let errors = reference.iter().filter(|f| f.code.as_str() < "W").count();
            assert_eq!(
                view.publishable,
                errors == 0 && compile(&s.document().snapshot()).is_ok()
            );
            assert!(view.findings.iter().all(|f| !f.stale));
            errors_seen += errors;
            warnings_seen += reference.len() - errors;
            full_checked += 1;
        }
    }
    assert!(
        local_checked > 200 && full_checked > 200,
        "{local_checked} {full_checked}"
    );
    assert!(
        errors_seen > 100 && warnings_seen > 50,
        "{errors_seen} {warnings_seen}"
    );
}

/// A chart with a focal point above block 10, which faces it; block 20,
/// beyond the focal point, faces the same way, so away from it; a section
/// holds block 31; free row 50. Overrides on block 10: a row label and two
/// seat overrides, one label only and one label with a flag.
fn chart() -> ChartDocument {
    serde_json::from_value(serde_json::json!({
        "schema": 1, "name": "n", "nextOid": 500, "settings": { "seatSize": 40 },
        "categories": [{ "key": "std", "label": "Standard", "color": "#2563EB" },
                       { "key": "vip", "label": "VIP", "color": "#B45309" }],
        "floors": [{ "id": 1, "name": "Stalls", "focalPoint": [0, -500] }],
        "nodes": [
            { "id": 10, "type": "rowBlock", "seatPitch": 50, "rowPitch": 100, "category": "std",
              "rows": [{ "rid": 11, "seats": 6, "oids": [[100, 6]], "label": "AA" },
                       { "rid": 12, "seats": 5, "oids": [[110, 5]], "removed": [2] },
                       { "rid": 13, "seats": 6, "oids": [[120, 6]] }],
              "seatOverrides": { "101": { "label": "W1" },
                                 "102": { "label": "W2", "flags": ["accessible"] } } },
            { "id": 20, "type": "rowBlock", "seatPitch": 50, "rowPitch": 100, "category": "vip",
              "origin": [0, -1500],
              "rows": [{ "rid": 21, "seats": 4, "oids": [[200, 4]] },
                       { "rid": 22, "seats": 4, "oids": [[210, 4]] }] },
            { "id": 30, "type": "section", "label": "Box", "category": "vip", "origin": [1000, 0],
              "children": [{ "id": 31, "type": "rowBlock", "seatPitch": 50, "rowPitch": 100,
                "rows": [{ "rid": 32, "seats": 4, "oids": [[130, 4]] }] }] },
            { "id": 50, "type": "row", "origin": [-600, 400], "seatPitch": 50, "seats": 7,
              "path": { "kind": "polyline", "points": [[0, 0], [300, 0]] },
              "oids": [[150, 7]], "category": "std", "label": "Z" }
        ]
    }))
    .expect("chart")
}

const SCHEMES: [LabelScheme; 7] = [
    LabelScheme::Numeric,
    LabelScheme::Odd,
    LabelScheme::Even,
    LabelScheme::Alpha,
    LabelScheme::AlphaLower,
    LabelScheme::Roman,
    LabelScheme::Custom,
];

fn list(r: &mut Rng) -> Vec<String> {
    (0..r.below(12)).map(|i| format!("c{i}")).collect()
}

fn start(r: &mut Rng) -> Option<LabelStart> {
    match r.below(4) {
        0 => None,
        1 => Some(LabelStart::Number(1 + r.below(4) as u32)),
        2 => Some(LabelStart::Text(["C", "3", "iv", "b"][r.below(4)].into())),
        _ => Some(LabelStart::Number(2)),
    }
}

fn numbering(r: &mut Rng, ids: &[u32]) -> Numbering {
    let affix = |r: &mut Rng| ["", "R", "-x"][r.below(3)].to_string();
    let rows = (r.below(3) > 0).then(|| RowLabels {
        scheme: SCHEMES[r.below(7)],
        start: start(r),
        step: 1 + r.below(2) as u32,
        skip: if r.below(2) == 0 {
            vec!["I".into(), "O".into()]
        } else {
            Vec::new()
        },
        direction: [RowDirection::FrontToBack, RowDirection::BackToFront][r.below(2)],
        prefix: affix(r),
        suffix: affix(r),
        custom: list(r),
    });
    let seats = (r.below(4) > 0).then(|| SeatLabels {
        scheme: SCHEMES[r.below(7)],
        start: start(r),
        step: 1 + r.below(2) as u32,
        direction: [
            SeatDirection::Ltr,
            SeatDirection::Rtl,
            SeatDirection::CenterOut,
            SeatDirection::Serpentine,
        ][r.below(4)],
        continuous: r.below(2) == 0,
        prefix: affix(r),
        suffix: affix(r),
        custom: list(r),
    });
    let across = (r.below(2) == 0).then(|| {
        let mut order = ids.to_vec();
        order.reverse();
        order.truncate(r.below(order.len() + 1));
        order
    });
    Numbering {
        rows,
        seats,
        after_z: [
            None,
            Some(AlphaOverflow::Spreadsheet),
            Some(AlphaOverflow::Double),
        ][r.below(3)],
        across,
        keep_overrides: r.below(3) == 0,
    }
}

/// The labels of the selected targets as the document stands: a numbering
/// that sets nothing previews them.
fn current(s: &mut Session) -> Preview {
    let none = Numbering {
        rows: None,
        seats: None,
        after_z: None,
        across: None,
        keep_overrides: true,
    };
    s.preview_numbering(&none).expect("current").clone()
}

fn random_targets(s: &mut Session, r: &mut Rng) -> Vec<u32> {
    let doc = s.document();
    let mut seat_nodes: Vec<u32> = doc
        .ids()
        .filter(|&id| matches!(doc.node(id), Some(Node::RowBlock(_) | Node::Row(_))))
        .collect();
    let tops: Vec<u32> = doc.children(None).to_vec();
    match r.below(3) {
        0 => {
            let some: Vec<u32> = tops.into_iter().filter(|_| r.below(2) == 0).collect();
            s.select(Level::Top, some);
        }
        1 => {
            let id = r.pick(&seat_nodes).unwrap_or(10);
            s.select(Level::Seats(id), []);
        }
        _ => {
            seat_nodes.retain(|&id| s.document().parent(id).is_none());
            s.select(Level::Top, seat_nodes);
        }
    }
    s.numbering().nodes.iter().map(|n| n.id).collect()
}

/// DSN-027: for random selections and numberings, the labels previewed
/// are the labels the expansion gives once applied, and so is the hovered
/// seat's full label; previewing changes nothing, applying is one undo
/// entry that undo reverts byte for byte and redo repeats; nodes not
/// selected are untouched; label overrides are cleared unless kept, and
/// every other override field stays; `ltr` and `rtl` are stored as seen
/// facing the focal point; numeric numbering continued across blocks gives
/// no label twice.
#[test]
fn numbering_previews_what_it_applies() {
    let mut applied = 0;
    let docs = [chart(), PRESETS[0].generate(2), chart()];
    for (n, doc) in docs.into_iter().enumerate() {
        let mut s = Session::load(doc).expect("load");
        let mut r = Rng(n as u64 + 77);
        for round in 0..80 {
            let ids = random_targets(&mut s, &mut r);
            let mut num = numbering(&mut r, &ids);
            if round % 5 == 0 {
                // Numeric, continuous, across every target: one sequence.
                num.seats = Some(SeatLabels {
                    continuous: true,
                    ..SeatLabels::default()
                });
                num.across = Some(Vec::new());
                num.keep_overrides = false;
            }
            let before = state(&s);
            let entries = s.editor().undo_len();
            let old: BTreeMap<u32, Node> = s
                .document()
                .ids()
                .filter_map(|id| Some((id, s.document().node(id)?.clone())))
                .collect();
            let preview = s.preview_numbering(&num).expect("preview").clone();
            assert_eq!(state(&s), before, "a preview changes nothing");
            assert_eq!(s.editor().undo_len(), entries);
            let hovered = preview.rows.iter().find_map(|row| {
                let seat = row.seats.first()?;
                // The canvas finds the hovered seat by `seat_under`.
                let under = s
                    .seat_under(seat.at, 1e6)
                    .expect("a seat under the pointer");
                assert!(ids.contains(&under.node), "round {round}: not a target");
                Some((row.node, seat.oid))
            });
            let sample = hovered.and_then(|(node, oid)| s.sample(node, oid));
            assert!(s.numbering_preview().is_some());
            s.number(&num).expect("number");
            assert!(s.numbering_preview().is_none(), "applying ends the preview");
            let after = state(&s);
            if after == before {
                continue;
            }
            applied += 1;
            assert_eq!(s.editor().undo_len(), entries + 1);
            assert_eq!(current(&mut s), preview, "round {round}");
            assert_eq!(hovered.and_then(|(node, oid)| s.sample(node, oid)), sample);
            for (id, was) in &old {
                let now = s.document().node(*id).expect("kept");
                if !ids.contains(id) {
                    assert_eq!(now, was, "node {id} not selected");
                    continue;
                }
                let over = |n: &Node| match n {
                    Node::RowBlock(b) => b.seat_overrides.clone(),
                    Node::Row(r) => r.seat_overrides.clone(),
                    _ => BTreeMap::new(),
                };
                for (oid, o) in over(was) {
                    let kept = over(now).get(&oid).cloned();
                    match (num.keep_overrides || num.seats.is_none(), kept) {
                        (true, k) => assert_eq!(k.as_ref(), Some(&o)),
                        (false, Some(k)) => {
                            assert!(k.label.is_none());
                            assert_eq!(
                                (&k.flags, &k.category, k.nudge),
                                (&o.flags, &o.category, o.nudge)
                            );
                        }
                        (false, None) => {
                            assert!(
                                o.flags.is_empty() && o.category.is_none() && o.nudge.is_none()
                            );
                        }
                    }
                }
                if let (Node::RowBlock(b), Some(_), false) = (now, &num.rows, num.keep_overrides) {
                    assert!(b.rows.iter().all(|row| row.label.is_none()));
                }
            }
            // As seen facing the focal point.
            let view = s.numbering();
            if let (Some(asked), Some(seen)) = (&num.seats, &view.seats)
                && num.across.is_none()
                && view.nodes.iter().all(|n| n.facing != Facing::Unknown)
            {
                assert_eq!(seen.direction, Shared::Same(asked.direction));
            }
            if round % 5 == 0 {
                let labels: Vec<&str> = preview
                    .rows
                    .iter()
                    .flat_map(|r| r.seats.iter().map(|s| s.label.as_str()))
                    .collect();
                let distinct: BTreeSet<&str> = labels.iter().copied().collect();
                assert_eq!(distinct.len(), labels.len(), "across: {labels:?}");
            }
            assert!(s.undo().expect("undo"));
            assert_eq!(state(&s), before);
            assert!(s.redo().expect("redo"));
            assert_eq!(state(&s), after);
        }
    }
    assert!(applied > 120, "{applied}");
}

/// Facing: block 10 faces the focal point and block 20 faces away, so the
/// panel's left to right is stored `ltr` on 10 and `rtl` on 20, and both
/// read `ltr` in the view; seat 1 of each is at the left as seen from the
/// focal point. Hostile parts are refused without a change.
#[test]
fn numbering_faces_the_focal_point() {
    let mut s = Session::load(chart()).expect("load");
    s.select(Level::Top, [10, 20]);
    let view = s.numbering();
    let facing: Vec<Facing> = view.nodes.iter().map(|n| n.facing).collect();
    assert_eq!(facing, [Facing::Forward, Facing::Reversed]);
    let ltr = Numbering {
        rows: None,
        seats: Some(SeatLabels::default()),
        after_z: None,
        across: None,
        keep_overrides: false,
    };
    s.number(&ltr).expect("number");
    let stored = |s: &Session, id| match s.document().node(id) {
        Some(Node::RowBlock(b)) => b.seat_labels.direction,
        _ => panic!("block"),
    };
    assert_eq!(
        (stored(&s, 10), stored(&s, 20)),
        (SeatDirection::Ltr, SeatDirection::Rtl)
    );
    assert_eq!(
        s.numbering().seats.map(|v| v.direction),
        Some(Shared::Same(SeatDirection::Ltr))
    );
    // Seat 1 is on the left of a viewer at the block facing the focal
    // point: looking up the screen from block 10 (left is −x), down it
    // from block 20 (left is +x).
    let preview = current(&mut s);
    for (id, left) in [(10, -1.0), (20, 1.0)] {
        let row = preview.rows.iter().find(|r| r.node == id).expect("row");
        let x = |label: &str| {
            row.seats
                .iter()
                .find(|x| x.label == label)
                .expect("seat")
                .at[0]
        };
        assert!(left * (x("1") - x("4")) > 0.0, "node {id}");
    }
    // Hostile values.
    let before = state(&s);
    let long = Numbering {
        seats: Some(SeatLabels {
            prefix: "x".repeat(65),
            ..SeatLabels::default()
        }),
        ..ltr.clone()
    };
    assert_eq!(s.number(&long), Err(CommandError::Limit("label bytes")));
    let huge = Numbering {
        rows: Some(RowLabels {
            custom: vec!["a".into(); MAX_LIST + 1],
            ..RowLabels::default()
        }),
        ..ltr
    };
    assert!(s.preview_numbering(&huge).is_err());
    assert_eq!(state(&s), before);
}

/// Every category key named anywhere: node fields and seat overrides.
fn named(doc: &Document) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for id in doc.ids() {
        let Some(node) = doc.node(id) else { continue };
        if let Some(Some(k)) = crate::command::category(node) {
            out.insert(k.clone());
        }
        if let Node::RowBlock(b) = node {
            out.extend(b.seat_overrides.values().filter_map(|o| o.category.clone()));
        }
        if let Node::Row(r) = node {
            out.extend(r.seat_overrides.values().filter_map(|o| o.category.clone()));
        }
    }
    out
}

fn dangling(doc: &Document) -> BTreeSet<String> {
    let listed: BTreeSet<String> = doc.categories().iter().map(|c| c.key.clone()).collect();
    named(doc).difference(&listed).cloned().collect()
}

/// DSN-028: for random category edits on charts whose seats use every
/// category through node fields, section defaults and seat overrides: a
/// merge or a delete with a replacement leaves no key named that the chart
/// does not list, moves every object of the old category to the new one,
/// and moves the active category with it; a delete without a replacement
/// is refused while anything names the key; each edit is one undo entry,
/// undone byte for byte and redone.
#[test]
fn categories_merge_and_delete() {
    let mut merged = 0;
    for (n, doc) in [
        chart(),
        PRESETS[0].generate(3),
        cavea_testkit::synthetic::stadium(3_000, 2),
    ]
    .into_iter()
    .enumerate()
    {
        let mut s = Session::load(doc).expect("load");
        let mut r = Rng(n as u64 + 900);
        // Paint some seats with overrides.
        let blocks: Vec<u32> = s
            .document()
            .ids()
            .filter(|&id| matches!(s.document().node(id), Some(Node::RowBlock(_))))
            .collect();
        for &id in blocks.iter().take(6) {
            let oids: Vec<u32> = s.index().seats(id).iter().take(3).map(|x| x.oid).collect();
            let key = s.document().categories().last().map(|c| c.key.clone());
            s.select(Level::Seats(id), oids);
            s.edit_seats(SeatEdit::Category { category: key })
                .expect("paint");
        }
        for _ in 0..80 {
            let keys: Vec<String> = s
                .document()
                .categories()
                .iter()
                .map(|c| c.key.clone())
                .collect();
            let a = keys.get(r.below(keys.len())).cloned().unwrap_or_default();
            let b = keys.get(r.below(keys.len())).cloned().unwrap_or_default();
            let view = s.categories();
            let count = |v: &CategoriesView, k: &str| {
                v.categories
                    .iter()
                    .find(|c| c.key == k)
                    .map_or(0, |c| c.objects)
            };
            let before = state(&s);
            let dangling_before = dangling(s.document());
            if r.below(3) == 0 {
                s.set_category(Some(a.clone()));
            }
            let active = s.category().map(str::to_owned);
            let result = match r.below(5) {
                0 => s.delete_category(&a, None),
                1 => s.delete_category(&a, Some(&b)),
                2 => s.add_category(
                    Some(0),
                    Category {
                        key: format!("k{}", r.below(1000)),
                        label: "New".into(),
                        color: "#0F766E".into(),
                        accessible: false,
                    },
                ),
                3 => s.move_category(&a, r.below(keys.len()) as u32),
                _ => s.merge_category(&a, &b),
            };
            assert!(dangling(s.document()).is_subset(&dangling_before));
            if let Err(CommandError::CategoryInUse { key, references }) = &result {
                assert_eq!(key, &a);
                assert!(*references > 0);
            }
            if result.is_err() {
                assert_eq!(state(&s), before);
                continue;
            }
            let removed = !s.document().categories().iter().any(|c| c.key == a);
            if removed
                && a != b
                && keys.contains(&b)
                && view
                    .categories
                    .iter()
                    .any(|c| c.key == a && c.references > 0)
            {
                merged += 1;
                let now = s.categories();
                assert_eq!(count(&now, &b), count(&view, &b) + count(&view, &a));
                assert!(!named(s.document()).contains(&a));
                if active.as_deref() == Some(&a) {
                    assert_eq!(s.category(), Some(b.as_str()));
                }
            }
            let after = state(&s);
            assert!(s.undo().expect("undo"));
            assert_eq!(state(&s), before);
            assert!(s.redo().expect("redo"));
            assert_eq!(state(&s), after);
        }
    }
    assert!(merged > 10, "{merged}");
}

/// The categories view counts what the compiler compiles: on a chart
/// without errors, each category's objects are the compiled seats of it
/// (from the seats' expansion), and its colour checks are W107's: weak
/// exactly when W107 reports low contrast, and too similar to an earlier
/// category exactly when W107 names that one first.
#[test]
fn category_view_matches_the_compiler() {
    let mut r = Rng(5);
    for round in 0..40 {
        let mut doc = PRESETS[0].generate(round);
        for c in &mut doc.categories {
            let v = r.next() as u32 & 0xFF_FFFF;
            // Near-greys now and then, so similar pairs occur.
            c.color = if r.below(3) == 0 {
                let g = 0x60 + r.below(16) as u32;
                format!("#{g:02X}{g:02X}{g:02X}")
            } else {
                format!("#{v:06X}")
            };
        }
        let warnings = compile(&doc).expect("compiles").findings;
        let s = Session::load(doc).expect("load");
        let view = s.categories();
        let mut seats: BTreeMap<String, u32> = BTreeMap::new();
        let d = s.document();
        for id in d.ids() {
            let section = d
                .parent(id)
                .and_then(|p| d.node(p))
                .and_then(crate::command::category)
                .cloned()
                .flatten();
            let mut e = cavea_compiler::Expansion::new();
            let overflow = d.settings().alpha_overflow;
            let expanded = match d.node(id) {
                Some(Node::RowBlock(b)) => e.push_row_block(b, overflow).is_ok(),
                Some(Node::Row(r)) => e.push_row(r, overflow).is_ok(),
                _ => false,
            };
            if expanded {
                for seat in e.seats.iter().filter(|x| !x.removed) {
                    let key = seat
                        .category
                        .map(str::to_owned)
                        .or(section.clone())
                        .unwrap_or_default();
                    *seats.entry(key).or_default() += 1;
                }
            }
        }
        for (i, c) in view.categories.iter().enumerate() {
            assert_eq!(
                c.objects,
                seats.get(&c.key).copied().unwrap_or(0),
                "{}",
                c.key
            );
            let low = warnings
                .iter()
                .any(|f| matches!(&f.detail, Detail::LowContrast { key, .. } if key == &c.key));
            assert_eq!(c.contrast.map(|x| x.weak), Some(low), "{}", c.color);
            let first = warnings.iter().find_map(|f| match &f.detail {
                Detail::SimilarColors { key, other } if key == &c.key => Some(other.clone()),
                _ => None,
            });
            let earlier = view.categories[..i]
                .iter()
                .find(|o| c.similar.contains(&o.key));
            assert_eq!(earlier.map(|o| o.key.clone()), first);
        }
    }
}

fn pointer(at: [f64; 2]) -> Pointer {
    Pointer {
        world: at,
        screen: at,
        zoom: 1.0,
        button: Button::Primary,
        kind: PointerKind::Mouse,
        mods: Modifiers::default(),
        time: 0.0,
    }
}

/// DSN-031: user guides are snap targets for points and moves, of the same
/// rank as edges and centres: a guide beats the grid even when the grid is
/// nearer, an edge and a guide go by distance, and guides can be turned
/// off. Row alignment ranks above them (07 §5.2).
#[test]
fn guides_snap_points_and_moves() {
    let doc: ChartDocument = serde_json::from_value(serde_json::json!({
        "schema": 1, "name": "g", "nextOid": 100,
        "nodes": [{ "id": 1, "type": "shape", "kind": "rect", "size": [100, 100],
                    "origin": [149, 500] },
                  { "id": 2, "type": "shape", "kind": "rect", "size": [40, 40],
                    "origin": [1001, 1001] }]
    }))
    .expect("doc");
    let mut editor = Editor::load(doc).expect("load");
    let mut index = SpatialIndex::new();
    index.sync(editor.document());
    let mut selection = Selection::default();
    let mut at = |guide: f64, on: bool, p: [f64; 2]| {
        let mut snap = Snap::default();
        snap.settings.guides = on;
        snap.user = vec![UserGuide {
            axis: GuideAxis::Vertical,
            at: guide,
        }];
        let mut cx = Context {
            editor: &mut editor,
            index: &index,
            selection: &mut selection,
            floor: None,
            category: None,
            snap: &mut snap,
        };
        snap::point(&mut cx, &pointer(p), None, None)
    };
    // Far from the shape: the guide (2 away) over the grid line (1 away).
    assert_eq!(at(103.0, true, [101.0, 2201.0])[0], 103.0);
    assert_eq!(at(103.0, false, [101.0, 2201.0])[0], 100.0);
    // The shape's left edge at 99: 2 away; a guide 1 away wins, 3 away not.
    assert_eq!(at(102.0, true, [101.0, 201.0])[0], 102.0);
    assert_eq!(at(104.0, true, [101.0, 201.0])[0], 99.0);
    // A move: shape 2's left edge (981) onto a guide at 1000 when dragged
    // by 17 (edge at 998, 2 away; the grid at 1000 too, but so is the guide).
    let mut snap = Snap::default();
    snap.user = vec![UserGuide {
        axis: GuideAxis::Vertical,
        at: 1_003.0,
    }];
    let mut cx = Context {
        editor: &mut editor,
        index: &index,
        selection: &mut selection,
        floor: None,
        category: None,
        snap: &mut snap,
    };
    let m = Moving::new(&cx, &[2], None).expect("moving");
    // Edge from 981 by 20 to 1001: the guide at 1003 (2) over the grid
    // line at 1000 (1).
    let v = snap::translation(&mut cx, &pointer([0.0, 0.0]), &m, [20.0, 0.0]);
    assert_eq!(v[0], 22.0);
}

/// Guides are per floor, not undoable, saved and restored as plain data,
/// listed in the overlay, and refused when not finite or past the limit.
#[test]
fn guides_live_in_the_session() {
    let mut s = Session::load(chart()).expect("load");
    let g = UserGuide {
        axis: GuideAxis::Horizontal,
        at: 250.0,
    };
    assert_eq!(s.add_guide(g), Ok(0));
    s.move_guide(0, 260.0).expect("move");
    assert_eq!(
        s.move_guide(0, f64::NAN),
        Err(CommandError::InvalidValue("guide"))
    );
    assert_eq!(s.move_guide(5, 1.0), Err(CommandError::IndexOutOfRange));
    s.select(Level::Top, [10]);
    s.delete().expect("delete");
    s.undo().expect("undo");
    assert_eq!(s.guides(), [UserGuide { at: 260.0, ..g }]);
    assert_eq!(s.overlay().user_guides, s.guides());
    let saved = s.saved_guides();
    let json = serde_json::to_string(&saved).expect("json");
    assert_eq!(
        json,
        r#"[{"floor":1,"guides":[{"axis":"horizontal","at":260.0}]}]"#
    );
    s.clear_guides();
    assert!(s.guides().is_empty());
    s.restore_guides(serde_json::from_str(&json).expect("parse"))
        .expect("restore");
    assert_eq!(s.saved_guides(), saved);
    for i in 1..MAX_GUIDES {
        s.add_guide(UserGuide { at: i as f64, ..g }).expect("add");
    }
    assert_eq!(s.add_guide(g), Err(CommandError::Limit("guides")));
}

/// The validation view, focus and canvas markers on a small example.
#[test]
fn findings_select_and_mark() {
    let mut doc = chart();
    // Block 31 copies block 10's labels outside any section: duplicates.
    if let Some(Node::Section(sec)) = doc.nodes.get_mut(2) {
        sec.label = None;
    }
    let mut s = Session::load(doc).expect("load");
    assert!(!s.validation().complete);
    while !s.validate(1).done {}
    let view = s.validation();
    assert!(view.complete && !view.publishable);
    let dup = view
        .findings
        .iter()
        .find(|f| f.code == "E001")
        .expect("E001");
    let node = dup.node.expect("node");
    let frame = s.focus(node, dup.row, dup.seat).expect("frame");
    assert_eq!(
        s.selection().items().iter().copied().collect::<Vec<_>>(),
        [node]
    );
    assert_eq!(Some(frame), s.index().bounds(node));
    let marks = s.overlay().markers;
    assert!(
        marks
            .iter()
            .any(|m| m.node == node && m.severity == "error")
    );
    // An empty seat label names its row and seat; focus selects the seat.
    s.select(Level::Seats(50), [152]);
    s.edit_seats(SeatEdit::Label {
        label: Some(String::new()),
    })
    .expect("label");
    let view = s.validation();
    assert!(!view.complete, "the document changed");
    s.validate(1);
    let empty = s
        .validation()
        .findings
        .into_iter()
        .find(|f| f.code == "E003")
        .expect("E003");
    assert_eq!(
        (empty.node, empty.row, empty.seat),
        (Some(50), Some(50), Some(152))
    );
    s.focus(50, empty.row, empty.seat).expect("frame");
    assert_eq!(s.selection().level(), Level::Seats(50));
}

fn ms(t: std::time::Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// Prints the panels' timings on `stadium-100k`: run with
/// `cargo test --release -p cavea-edit measure_panels -- --ignored --nocapture`.
#[test]
#[ignore]
fn measure_panels() {
    use std::time::Instant;
    let doc = cavea_testkit::presets::find("stadium-100k")
        .expect("preset")
        .generate(1);
    let mut s = Session::load(doc).expect("load");
    let tops: Vec<u32> = s.document().children(None).to_vec();
    s.select(Level::Top, tops.clone());
    let num = Numbering {
        rows: Some(RowLabels::default()),
        seats: Some(SeatLabels {
            continuous: true,
            ..SeatLabels::default()
        }),
        after_z: None,
        across: Some(Vec::new()),
        keep_overrides: false,
    };
    let t = Instant::now();
    let view = s.numbering();
    let v = ms(t);
    let t = Instant::now();
    let seats: usize = s
        .preview_numbering(&num)
        .expect("preview")
        .rows
        .iter()
        .map(|r| r.seats.len())
        .sum();
    let p = ms(t);
    let t = Instant::now();
    s.number(&num).expect("number");
    let a = ms(t);
    let t = Instant::now();
    s.undo().expect("undo");
    let u = ms(t);
    println!(
        "numbering {} nodes, {seats} seats: view {v:.2} ms, preview {p:.2} ms, apply {a:.2} ms, undo {u:.2} ms",
        view.nodes.len()
    );
    let t = Instant::now();
    let cats = s.categories();
    let c = ms(t);
    let uses = |k: &str| {
        cats.categories
            .iter()
            .find(|x| x.key == k)
            .map_or(0, |x| x.objects)
    };
    let t = Instant::now();
    s.merge_category("value", "standard").expect("merge");
    let m = ms(t);
    let t = Instant::now();
    s.undo().expect("undo");
    let mu = ms(t);
    println!(
        "categories view {c:.3} ms; merge value ({}) into standard ({}): {m:.2} ms, undo {mu:.2} ms",
        uses("value"),
        uses("standard")
    );
    let t = Instant::now();
    while !s.validator().units_done() {
        s.validate(u64::MAX);
    }
    let first = ms(t);
    let t = Instant::now();
    s.validate(u64::MAX);
    let full = ms(t);
    let t = Instant::now();
    let listed = s.validation().findings.len();
    let lv = ms(t);
    println!(
        "validation: first pass {first:.1} ms (units and full), full pass {full:.1} ms, view {lv:.3} ms ({listed} findings)"
    );
    let block = s
        .document()
        .children(Some(tops[0]))
        .first()
        .copied()
        .expect("block");
    s.apply(Command::SetProperty {
        id: block,
        property: Property::SeatPitch(56.0),
    })
    .expect("edit");
    let t = Instant::now();
    let p = s.validate(1);
    let step = ms(t);
    println!(
        "one block edited: unit step {step:.2} ms; next piece (full pass) costs {}",
        p.next
    );
    let t = Instant::now();
    s.validate(u64::MAX);
    println!("full pass after it: {:.1} ms", ms(t));
    // A drawing tool's point snap, without and with 100 guides.
    s.set_tool(ToolId::Rect);
    s.select(Level::Top, []);
    let drag = |s: &mut Session| {
        s.input(Input::PointerDown(pointer([10.0, 2_000.0])));
        let t = Instant::now();
        let n = 200;
        for i in 0..n {
            s.input(Input::PointerMove(pointer([f64::from(i) * 13.0, 2_100.0])));
        }
        s.input(Input::KeyDown(Key::Escape, Modifiers::default()));
        ms(t) / f64::from(n)
    };
    let without = drag(&mut s);
    for i in 0..100 {
        let axis = if i % 2 == 0 {
            GuideAxis::Vertical
        } else {
            GuideAxis::Horizontal
        };
        s.add_guide(UserGuide {
            axis,
            at: f64::from(i) * 97.0,
        })
        .expect("guide");
    }
    let with = drag(&mut s);
    println!("rect tool pointer move: {without:.4} ms without guides, {with:.4} ms with 100");
    // Markers: every section labelled alike, so every block's seats clash.
    s.set_tool(ToolId::Select);
    let sections: Vec<u32> = s.document().children(None).to_vec();
    let commands = sections
        .iter()
        .filter(|&&id| matches!(s.document().node(id), Some(Node::Section(_))))
        .map(|&id| Command::SetProperty {
            id,
            property: Property::Label(Some("S".into())),
        })
        .collect();
    s.apply(Command::Composite { commands }).expect("labels");
    let t = Instant::now();
    s.validate(u64::MAX);
    let pass = ms(t);
    let t = Instant::now();
    let view = s.validation();
    let v = ms(t);
    let t = Instant::now();
    let o = s.overlay();
    println!(
        "with {} findings: pass {pass:.1} ms, view {v:.3} ms, overlay {:.3} ms ({} markers)",
        view.findings.len(),
        ms(t),
        o.markers.len()
    );
}

/// Sol review round 1 (S1, S3, S4, S5): a node moved into a section stops
/// being a unit of its own; renumbering keeps an override for an oid the
/// node lacks (which `editSeats` could not clear) and so applies what it
/// previews; the active category stays one of the chart's through undo;
/// switching tools ends the preview.
#[test]
fn panel_state_follows_the_document() {
    let mut value = serde_json::to_value(chart()).expect("json");
    value["nodes"][1]["category"] = serde_json::Value::Null;
    value["nodes"][1]["seatOverrides"] = serde_json::json!({ "999": { "label": "X" } });
    let doc: ChartDocument = serde_json::from_value(value).expect("doc");
    let mut s = Session::load(doc).expect("load");
    while !s.validate(1).done {}
    let e002 = |s: &Session| {
        s.validation()
            .findings
            .iter()
            .any(|f| f.code == "E002" && f.node == Some(20))
    };
    assert!(e002(&s), "block 20 has no category");
    s.apply(Command::PlaceNode {
        id: 20,
        parent: Some(30),
        index: 0,
    })
    .expect("place");
    while !s.validator().units_done() {
        s.validate(1);
    }
    assert!(!e002(&s), "the section gives it one");
    // Renumbering block 20 with a dangling label override.
    s.select(Level::Section(30), [20]);
    let num = Numbering {
        rows: None,
        seats: Some(SeatLabels {
            start: Some(LabelStart::Number(5)),
            ..SeatLabels::default()
        }),
        after_z: None,
        across: None,
        keep_overrides: false,
    };
    let preview = s.preview_numbering(&num).expect("preview").clone();
    s.set_tool(ToolId::Rect);
    assert!(
        s.numbering_preview().is_none(),
        "a tool switch ends the preview"
    );
    s.select(Level::Section(30), [20]);
    s.number(&num).expect("applies what it previews");
    assert_eq!(current(&mut s), preview);
    // Sol review round 2 (S6): overlapping oid runs (E009) still let
    // renumbering clear the override it previews as cleared.
    let mut runs = Session::load(chart()).expect("load");
    runs.apply(Command::SetSeatOverride {
        id: 10,
        oid: 102,
        value: None,
    })
    .expect("clear");
    let mut v = serde_json::to_value(runs.document().snapshot()).expect("json");
    v["nodes"][0]["rows"][1]["oids"] = serde_json::json!([[101, 1], [300, 4]]);
    v["nodes"][0]["seatOverrides"] = serde_json::json!({ "103": { "label": "W3" } });
    let mut runs = Session::load(serde_json::from_value(v).expect("doc")).expect("load");
    runs.select(Level::Top, [10]);
    let shown = runs.preview_numbering(&num).expect("preview").clone();
    runs.number(&num).expect("applies what it previews");
    assert_eq!(current(&mut runs), shown);
    // S7: chart-level findings are stale once the chart changes.
    let bad = Category {
        key: "std".into(),
        label: "S".into(),
        color: "#2563EB".into(),
        accessible: false,
    };
    let mut c = serde_json::to_value(chart()).expect("json");
    c["categories"][0]["color"] = serde_json::json!("bad");
    let mut fix = Session::load(serde_json::from_value(c).expect("doc")).expect("load");
    while !fix.validate(1).done {}
    fix.update_category("std", bad).expect("fix the colour");
    let colour = fix
        .validation()
        .findings
        .into_iter()
        .find(|f| f.message.contains("colour"));
    assert!(colour.is_some_and(|f| f.stale));
    // Undo of a rename of the active category.
    s.set_category(Some("std".into()));
    let vip = Category {
        key: "gold".into(),
        label: "Gold".into(),
        color: "#CA8A04".into(),
        accessible: false,
    };
    s.update_category("std", vip).expect("rename");
    assert_eq!(s.category(), Some("gold"));
    s.undo().expect("undo");
    assert_eq!(s.category(), Some("std"));
}
