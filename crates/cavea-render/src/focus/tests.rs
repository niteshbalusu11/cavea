//! The focus search against a brute-force statement of its rule, on the
//! presets and on synthetic stadiums of several seeds, and the properties
//! 11 §3 asks of it.

use std::collections::VecDeque;

use super::*;
use crate::scene::Scene;

const DIRS: [Dir; 4] = [Dir::Left, Dir::Right, Dir::Up, Dir::Down];

fn scene(doc: &cavea_compiler::ChartDocument) -> Scene {
    Scene::decode(&cavea_compiler::compile(doc).unwrap().public).unwrap()
}

fn preset(name: &str) -> Scene {
    scene(&cavea_testkit::presets::find(name).unwrap().generate(1))
}

fn offset(s: &Scene, from: u32, to: u32, dir: Dir) -> (f32, f32) {
    let (p, q) = (s.pos[from as usize], s.pos[to as usize]);
    axes(dir.unit(), [q[0] - p[0], q[1] - p[1]])
}

/// The rule of the module's documentation, by scanning every seat.
fn reference(s: &Scene, from: u32, dir: Dir) -> Option<u32> {
    let row = &s.chart.rows[s.row[from as usize] as usize];
    let in_row = row.seat_first..row.seat_first + u32::from(row.seat_count);
    let best = |cands: &mut dyn Iterator<Item = u32>, cone: bool| {
        cands
            .filter(|&n| n != from && s.active.contains(&n))
            .filter_map(|n| {
                let (along, across) = offset(s, from, n, dir);
                (along > 0.0 && (!cone || across.abs() <= along))
                    .then_some((along + ACROSS_WEIGHT * across.abs(), n))
            })
            .min_by(|a, b| a.partial_cmp(b).unwrap())
            .map(|(_, n)| n)
    };
    let neighbours = [from.checked_sub(1), Some(from + 1)];
    best(
        &mut neighbours
            .into_iter()
            .flatten()
            .filter(|n| in_row.contains(n)),
        true,
    )
    .or_else(|| best(&mut s.active.clone().filter(|n| !in_row.contains(n)), true))
    .or_else(|| best(&mut s.active.clone().filter(|n| !in_row.contains(n)), false))
}

/// Seats `step` and `reference` are compared from: all of them, or every
/// `stride`th.
fn check_against_reference(s: &Scene, stride: usize) {
    for from in s.active.clone().step_by(stride) {
        for dir in DIRS {
            let got = step(s, from, dir);
            assert_eq!(got, reference(s, from, dir), "from {from} {dir:?}");
            match got {
                // A different seat, strictly ahead.
                Some(to) => {
                    assert_ne!(to, from);
                    assert!(offset(s, from, to, dir).0 > 0.0);
                }
                // An edge: nothing ahead but, at most, seats of its own row.
                None => assert!(
                    s.active
                        .clone()
                        .filter(|&n| s.row[n as usize] != s.row[from as usize])
                        .all(|n| offset(s, from, n, dir).0 <= 0.0),
                    "from {from} {dir:?} reported an edge"
                ),
            }
        }
    }
}

#[test]
fn matches_the_rule_on_presets_and_synthetic_charts() {
    check_against_reference(&preset("theatre"), 1);
    check_against_reference(&preset("arena"), 41);
    for seed in 1..6 {
        check_against_reference(&scene(&cavea_testkit::synthetic::stadium(1_500, seed)), 3);
    }
}

#[test]
fn right_then_left_returns_along_a_straight_row() {
    let s = preset("arena");
    let mut checked = 0;
    for row in &s.chart.rows {
        let seats = row.seat_first..row.seat_first + u32::from(row.seat_count);
        let y = s.pos[seats.start as usize][1];
        if !seats.clone().all(|i| s.pos[i as usize][1] == y) {
            continue;
        }
        for from in seats.clone() {
            if let Some(to) = step(&s, from, Dir::Right).filter(|to| seats.contains(to)) {
                assert_eq!(step(&s, to, Dir::Left), Some(from));
                checked += 1;
            }
        }
    }
    assert!(checked > 1000, "{checked} straight steps");
}

/// Every seat of every section reaches, and is reached from, the section's
/// first seat: arrows connect the whole block, whatever its curve.
fn connected(s: &Scene) {
    let n = s.pos.len();
    let mut forward = vec![Vec::new(); n];
    let mut backward = vec![Vec::new(); n];
    for from in s.active.clone() {
        for dir in DIRS {
            if let Some(to) = step(s, from, dir) {
                forward[from as usize].push(to);
                backward[to as usize].push(from);
            }
        }
    }
    let reach = |start: u32, edges: &[Vec<u32>]| {
        let mut seen = vec![false; n];
        let mut queue = VecDeque::from([start]);
        seen[start as usize] = true;
        while let Some(i) = queue.pop_front() {
            for &j in &edges[i as usize] {
                if !std::mem::replace(&mut seen[j as usize], true) {
                    queue.push_back(j);
                }
            }
        }
        seen
    };
    for section in s.chart.sections.iter().filter(|x| x.floor == s.floor) {
        let rows = section.row_first..section.row_first + section.row_count;
        let seats: Vec<u32> = rows
            .flat_map(|r| {
                let row = &s.chart.rows[r as usize];
                row.seat_first..row.seat_first + u32::from(row.seat_count)
            })
            .collect();
        let Some(&start) = seats.first() else {
            continue;
        };
        let (to, from) = (reach(start, &forward), reach(start, &backward));
        for &i in &seats {
            assert!(to[i as usize] && from[i as usize], "seat {i} cut off");
        }
    }
}

#[test]
fn every_seat_of_a_block_reaches_every_other() {
    connected(&preset("theatre"));
    connected(&preset("arena"));
    connected(&scene(&cavea_testkit::synthetic::stadium(3_000, 9)));
}

#[test]
fn follows_a_curved_row_and_crosses_rows_and_aisles() {
    let s = preset("theatre");
    let row_of = |i: u32| s.row[i as usize];
    let section = |i: u32| s.section_of(i as usize);
    // The middle seat of a curved row: left and right stay in the row,
    // up and down change row in the same block.
    let r = s
        .chart
        .rows
        .iter()
        .position(|r| r.seat_count >= 9 && r.section != cavea_format::NONE)
        .unwrap();
    let row = &s.chart.rows[r];
    let mid = row.seat_first + u32::from(row.seat_count) / 2;
    let (a, b) = (s.pos[mid as usize - 1], s.pos[mid as usize + 1]);
    assert_ne!(a[1], b[1], "the theatre's rows curve");
    for dir in [Dir::Left, Dir::Right] {
        let to = step(&s, mid, dir).unwrap();
        assert!(to == mid - 1 || to == mid + 1, "{dir:?}");
    }
    for dir in [Dir::Up, Dir::Down] {
        if let Some(to) = step(&s, mid, dir) {
            assert_ne!(row_of(to), row_of(mid), "{dir:?}");
            assert_eq!(section(to), section(mid), "{dir:?}");
        }
    }
    // A row's end seat steps across the aisle into the next block, within
    // the cone where there is one.
    let mut crossed = 0;
    for row in &s.chart.rows {
        let ends = [
            row.seat_first,
            row.seat_first + u32::from(row.seat_count) - 1,
        ];
        for end in ends {
            for dir in [Dir::Left, Dir::Right] {
                let Some(to) = step(&s, end, dir) else {
                    continue;
                };
                let (along, across) = offset(&s, end, to, dir);
                if section(to) != section(end) && across.abs() <= along {
                    crossed += 1;
                }
            }
        }
    }
    assert!(crossed > 10, "{crossed} aisle crossings");
}

#[test]
fn nearest_is_the_closest_seat() {
    let s = preset("arena");
    let b = s.bounds;
    for k in 0..50u32 {
        let p = [
            b.x + b.width * (k as f32 * 0.37 % 1.0),
            b.y + b.height * (k as f32 * 0.61 % 1.0),
        ];
        let brute = s
            .active
            .clone()
            .map(|i| (crate::hit::distance2(p, s.pos[i as usize]), i))
            .min_by(|a, b| a.partial_cmp(b).unwrap())
            .map(|(_, i)| i);
        assert_eq!(nearest(&s, p), brute);
    }
}
