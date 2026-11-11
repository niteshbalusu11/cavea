//! Loading compiled charts, palette and themes, status ranges and culling.

use cavea_compiler::{
    document::{AlphaOverflow, ChartDocument, Node},
    expand::Expansion,
};

use super::*;
use crate::{
    protocol::{BuiltinTheme, ThemeToken},
    scene::binary_angle,
    theme::{self, TEXELS_PER_STATE, visual},
};

/// One section of three curved rows of chair seats (size 40), the middle seat
/// of the front row in a second category, and a second floor of one seat.
const ARC: &str = r##"{"schema":1,"name":"Arc","settings":{"seatSize":40,"seatShape":"chair"},"nextOid":200,
  "categories":[{"key":"a","label":"Stalls","color":"#2563EB"},{"key":"b","label":"Box","color":"#B45309"}],
  "floors":[{"id":1,"name":"Ground","level":0},{"id":2,"name":"Upper","level":1}],
  "nodes":[{"id":1,"type":"section","floor":1,"label":"S","category":"a","children":[
    {"id":2,"type":"rowBlock","curve":{"kind":"arc","radius":500},
     "rows":[{"rid":3,"seats":9,"oids":[[10,9]]},{"rid":4,"seats":9,"oids":[[20,9]]},{"rid":5,"seats":9,"oids":[[30,9]]}],
     "seatOverrides":{"14":{"category":"b"}}}]},
   {"id":6,"type":"rowBlock","floor":2,"origin":[0,5000],"category":"b","rows":[{"rid":7,"seats":1,"label":"U","oids":[[40,1]]}]}]}"##;

/// One draw range, as the fake records it.
fn one(range: Range<u32>) -> Vec<Range<u32>> {
    std::iter::once(range).collect()
}

fn load(e: &mut E, ccf: Vec<u8>) -> Vec<Event> {
    e.drain_events().for_each(drop);
    e.handle(Command::LoadChart { ccf });
    e.drain_events().collect()
}

fn loaded(bytes: Vec<u8>) -> E {
    let mut e = Engine::new(Fake::default());
    load(&mut e, bytes);
    resize(&mut e, W, H, 1.0, 3.0);
    tick(&mut e, 0.0);
    e
}

/// The static, dynamic and palette resource ids of the loaded chart.
fn ids(e: &E) -> (u32, u32, u32) {
    let l = e.scene.as_ref().unwrap();
    (l.static_buf.0, l.dynamic_buf.0, l.palette.0)
}

fn palette(e: &E) -> &[u8] {
    &e.backend.textures[&ids(e).2].1
}

/// Fill, stroke and ink texels of `state` in palette `row`.
fn texels(e: &E, row: usize, state: u8) -> [[u8; 4]; 3] {
    let width = PALETTE_WIDTH as usize * 4;
    let at = row * width + usize::from(state) * TEXELS_PER_STATE as usize * 4;
    let bytes = &palette(e)[at..at + 12];
    [0, 4, 8].map(|i| bytes[i..i + 4].try_into().unwrap())
}

#[test]
fn loading_a_chart_uploads_what_the_scene_implies() {
    let mut e = Engine::new(Fake::default());
    let events = load(&mut e, compile(ARC));
    let Some(Event::Ready(info)) = events.first() else {
        panic!("no Ready: {events:?}");
    };
    assert_eq!(
        (info.name.as_str(), info.seats, info.objects),
        ("Arc", 28, 28)
    );
    let floors: Vec<_> = info
        .floors
        .iter()
        .map(|f| (f.name.as_str(), f.level, f.seats))
        .collect();
    assert_eq!(floors, [("Ground", 0, 27), ("Upper", 1, 1)]);
    assert_eq!(info.sections.len(), 1);
    assert_eq!(
        (info.sections[0].label.as_str(), info.sections[0].seats),
        ("S", 27)
    );
    let cats: Vec<_> = info
        .categories
        .iter()
        .map(|c| (c.key.as_str(), c.label.as_str(), c.color, c.objects))
        .collect();
    assert_eq!(
        cats,
        [
            ("a", "Stalls", [0x25, 0x63, 0xEB, 255], 26),
            ("b", "Box", [0xB4, 0x53, 0x09, 255], 2)
        ]
    );

    // Positions against the document's own expansion (block at the origin,
    // unrotated, so node space is chart space), quantised to 0.25 cu.
    let doc = ChartDocument::from_json(ARC.as_bytes()).unwrap();
    let Node::Section(section) = &doc.nodes[0] else {
        panic!()
    };
    let Node::RowBlock(block) = &section.children[0] else {
        panic!()
    };
    let mut expansion = Expansion::new();
    expansion
        .push_row_block(block, AlphaOverflow::Double)
        .unwrap();
    let (static_id, dynamic_id, palette_id) = ids(&e);
    let statics = &e.backend.buffers[&static_id];
    assert_eq!(statics.len(), 28 * seat::STATIC_STRIDE);
    let seat = |i: usize| {
        let b = &statics[i * 12..i * 12 + 12];
        let f = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        ([f(0), f(4)], u16::from_le_bytes([b[8], b[9]]), b[10], b[11])
    };
    for (i, expected) in expansion.seats.iter().enumerate() {
        let (pos, half, _, shape) = seat(i);
        for (got, want) in pos.iter().zip(expected.pos) {
            assert!((f64::from(*got) - want).abs() <= 0.125, "seat {i}");
        }
        // Size 40 cu is 160 quarter units; half of it.
        assert_eq!((half, shape), (80, seat::shape::CHAIR));
    }
    // Interior seats of a curved row point along the arc's tangent at the
    // seat: (cos θ, −sin θ) where θ is the seat's angle from the apex.
    for row in 0..3usize {
        let radius = 500.0 + 95.0 * row as f64;
        for k in 1..8usize {
            let i = row * 9 + k;
            let theta = (seat(i).0[0] as f64 / radius).asin();
            let expected = binary_angle(theta.cos() as f32, -theta.sin() as f32);
            let got = seat(i).2;
            assert!(
                got.abs_diff(expected) <= 1 || got.abs_diff(expected) == 255,
                "seat {i}"
            );
        }
        assert_ne!(seat(row * 9).2, seat(row * 9 + 8).2, "row {row} is curved");
    }
    // The floor's lone seat takes its row's direction: straight, angle 0.
    assert_eq!(seat(27).2, 0);

    // Every seat neutral: category, visual state 0, no effects.
    let dynamic = &e.backend.buffers[&dynamic_id];
    let cat: Vec<u16> = dynamic
        .chunks(4)
        .map(|d| u16::from_le_bytes([d[0], d[1]]))
        .collect();
    assert_eq!(cat[4], 1, "the override");
    assert_eq!(cat.iter().filter(|&&c| c == 0).count(), 26);
    assert!(dynamic.chunks(4).all(|d| d[2..] == [0, 0]));
    let (desc, bytes) = &e.backend.textures[&palette_id];
    assert_eq!((desc.width, desc.height), (PALETTE_WIDTH, 3));
    assert_eq!(bytes.len(), 3 * PALETTE_WIDTH as usize * 4);

    // Only the first floor is fitted and drawn, as one range.
    resize(&mut e, W, H, 1.0, 3.0);
    tick(&mut e, 0.0);
    assert_eq!(e.backend.draws, [one(0..27)]);
    let c = cam(&e);
    assert!(c.centre[1] < 1000.0, "fitted to the first floor: {c:?}");
}

#[test]
fn a_bad_chart_is_an_error_and_leaves_the_engine_as_it_was() {
    let good = compile(ARC);
    let mut e = loaded(good.clone());
    let before = ids(&e);
    let created = e.backend.next;
    let mut bad = vec![
        Vec::new(),
        good[..good.len() / 2].to_vec(),
        b"CCF2".to_vec(),
    ];
    let mut magic = good.clone();
    magic[0] = b'X';
    bad.push(magic);
    for bytes in bad {
        let events = load(&mut e, bytes);
        assert!(
            matches!(
                events.as_slice(),
                [Event::Error {
                    code: ErrorCode::BadChart,
                    ..
                }]
            ),
            "{events:?}"
        );
    }
    assert_eq!((ids(&e), e.backend.next), (before, created));
    assert!(e.backend.destroyed.is_empty());
    // Still usable: it moves and draws.
    set_camera(&mut e, [-100.0, -100.0, 200.0, 200.0]);
    let draws = e.backend.draws.len();
    tick(&mut e, 16.0);
    assert_eq!(e.backend.draws.len(), draws + 1);

    // Single-byte corruption anywhere: a chart or an error, never a panic.
    for at in (0..good.len()).step_by(3) {
        let mut bytes = good.clone();
        bytes[at] ^= 0x5A;
        let events = load(&mut e, bytes);
        assert!(
            matches!(events.first(), Some(Event::Ready(_) | Event::Error { .. })),
            "byte {at}: {events:?}"
        );
        tick(&mut e, 32.0);
    }
}

#[test]
fn loading_a_second_chart_releases_the_first() {
    let mut e = loaded(compile(ARC));
    let (s, d, p) = ids(&e);
    let [o1, o2] = e.scene.as_ref().unwrap().overlay;
    load(&mut e, two_seats([0.0, 0.0], [10.0, 10.0]));
    assert_eq!(
        e.backend.destroyed,
        [
            ResourceId::Buffer(BufferId(s)),
            ResourceId::Buffer(BufferId(d)),
            ResourceId::Texture(TextureId(p)),
            ResourceId::Buffer(o1),
            ResourceId::Buffer(o2),
        ]
    );
    // Only the new chart's five resources are alive, and it is what is drawn.
    assert_eq!(e.backend.buffers.len() + e.backend.textures.len(), 5);
    tick(&mut e, 16.0);
    assert_eq!(e.backend.draws.last(), Some(&one(0..2)));
    let c = cam(&e);
    assert!(close(c.centre, [5.0, 5.0], 1e-3), "refitted: {c:?}");
}

#[test]
fn a_status_change_uploads_only_its_sub_range() {
    let mut e = loaded(compile(ARC));
    let (_, dynamic, _) = ids(&e);
    e.apply_status(10..13, theme::status::UNAVAILABLE);
    e.apply_status(5..6, theme::status::UNAVAILABLE);
    e.apply_status(12..13, theme::status::AVAILABLE);
    tick(&mut e, 16.0);
    // One upload per frame, covering the merged dirty range 5..13.
    assert_eq!(e.backend.buffer_writes, [(dynamic, 5 * 4, 8 * 4)]);
    let visuals: Vec<u8> = e.backend.buffers[&dynamic]
        .chunks(4)
        .map(|d| d[2])
        .collect();
    let unavailable = [5, 10, 11];
    for (i, v) in visuals.iter().enumerate() {
        let expected = if unavailable.contains(&i) {
            visual::UNAVAILABLE
        } else {
            visual::AVAILABLE
        };
        assert_eq!(*v, expected, "seat {i}");
    }
    let scene = &e.scene.as_ref().unwrap().scene;
    assert_eq!(&scene.status[9..13], [0, 1, 1, 0]);
    // A range past the last seat changes nothing.
    e.drain_events().for_each(drop);
    e.apply_status(20..40, 1);
    assert!(matches!(
        e.drain_events().next(),
        Some(Event::Error {
            code: ErrorCode::InvalidInput,
            ..
        })
    ));
    tick(&mut e, 32.0);
    assert_eq!(e.backend.buffer_writes.len(), 1);
}

const WHITE: theme::Rgba = [255, 255, 255, 255];
const BLACK: theme::Rgba = [0, 0, 0, 255];
const BLUE: theme::Rgba = [0x25, 0x63, 0xEB, 255];
const BROWN: theme::Rgba = [0xB4, 0x53, 0x09, 255];

/// `SetTheme` with `tokens`; returns the events it emitted.
fn set_theme(e: &mut E, base: BuiltinTheme, tokens: &[(&str, &str)]) -> Vec<Event> {
    e.drain_events().for_each(drop);
    let tokens = tokens
        .iter()
        .map(|&(name, value)| ThemeToken {
            name: name.into(),
            value: value.into(),
        })
        .collect();
    e.handle(Command::SetTheme { base, tokens });
    e.drain_events().collect()
}

/// The palette rows of `ARC` (two categories and the row without one) against
/// the table of 04 §9 for theme `t`.
fn assert_rows(e: &E, t: &Theme, categories: [theme::Rgba; 2]) {
    for (row, fill) in categories.into_iter().chain([t.fg_muted]).enumerate() {
        let [f, s, ink] = texels(e, row, visual::AVAILABLE);
        assert_eq!([f, s], [fill, t.available_stroke], "row {row}");
        assert!(ink == WHITE || ink == BLACK, "row {row}");
        let unavailable = [t.unavailable_fill, t.unavailable_stroke, t.fg_muted];
        assert_eq!(
            texels(e, row, visual::UNAVAILABLE),
            unavailable,
            "row {row}"
        );
        assert_eq!(texels(e, row, visual::BOOKED), unavailable, "row {row}");
        let selected = [t.selected_fill, t.selected_fill, t.selected_ink];
        assert_eq!(texels(e, row, visual::SELECTED_PENDING), selected);
        assert_eq!(texels(e, row, visual::SELECTED_HELD), selected);
    }
}

#[test]
fn themes_rewrite_the_palette_and_clear_colour_and_no_instance_data() {
    let mut e = loaded(compile(ARC));
    let light = Theme::builtin(BuiltinTheme::Light);
    assert_rows(&e, &light, [BLUE, BROWN]);
    assert_eq!(e.backend.clears.last(), Some(&light.clear()));
    let buffers = e.backend.buffers.clone();
    let mut t = 16.0;
    for base in [
        BuiltinTheme::Dark,
        BuiltinTheme::HighContrast,
        BuiltinTheme::Light,
    ] {
        let writes = e.backend.texture_writes.len();
        assert_eq!(set_theme(&mut e, base, &[]), [Event::NeedFrame]);
        tick(&mut e, t);
        t += 16.0;
        let theme = Theme::builtin(base);
        assert_eq!(e.backend.texture_writes.len(), writes + 1);
        assert_rows(&e, &theme, [BLUE, BROWN]);
        // Both category colours carry white text better than black.
        assert_eq!(texels(&e, 0, visual::AVAILABLE)[2], WHITE);
        assert_eq!(texels(&e, 1, visual::AVAILABLE)[2], WHITE);
        assert_eq!(e.backend.clears.last(), Some(&theme.clear()));
        assert_eq!(e.backend.dims.last(), Some(&theme.dim));
    }

    // Tokens apply over the base in order, with or without the prefix.
    let events = set_theme(
        &mut e,
        BuiltinTheme::Dark,
        &[
            ("category-b", "#00ff00"),
            ("--cavea-seat-unavailable-fill", "#102030"),
            ("dim", "0.5"),
            ("bg", "#000"),
            ("bg", "#123456"),
            ("font", "serif"),
        ],
    );
    assert_eq!(events, [Event::NeedFrame]);
    tick(&mut e, t);
    let mut expected = Theme::builtin(BuiltinTheme::Dark);
    expected.unavailable_fill = [0x10, 0x20, 0x30, 255];
    expected.bg = [0x12, 0x34, 0x56, 255];
    expected.dim = 0.5;
    assert_rows(&e, &expected, [BLUE, [0, 255, 0, 255]]);
    assert_eq!(texels(&e, 1, visual::AVAILABLE)[2], BLACK);
    assert_eq!(e.backend.clears.last(), Some(&expected.clear()));
    assert_eq!(e.backend.dims.last(), Some(&0.5));
    // No instance data was touched by any of it.
    assert!(e.backend.buffer_writes.is_empty());
    assert_eq!(e.backend.buffers, buffers);

    // Malformed tokens refuse the whole command with `InvalidInput`: nothing
    // changes and no frame is asked for.
    let (writes, frames) = (e.backend.texture_writes.len(), e.backend.targets.len());
    for bad in [
        ("seat-bogus", "#000"),
        ("bg", "grey"),
        ("bg", "#12345"),
        ("bg", "#ggg"),
        ("bg", "#ééé"),
        ("dim", "1.5"),
        ("dim", "-0.1"),
        ("section-tint", ""),
        ("category-a", "blue"),
    ] {
        let events = set_theme(&mut e, BuiltinTheme::Light, &[("bg", "#fff"), bad]);
        assert!(
            matches!(
                events.as_slice(),
                [Event::Error {
                    code: ErrorCode::InvalidInput,
                    ..
                }]
            ),
            "{bad:?}: {events:?}"
        );
    }
    tick(&mut e, t + 16.0);
    assert_eq!(e.backend.texture_writes.len(), writes);
    assert_eq!(e.backend.targets.len(), frames);
    assert_rows(&e, &expected, [BLUE, [0, 255, 0, 255]]);

    // The theme outlives the chart, category overrides by key included.
    set_theme(
        &mut e,
        BuiltinTheme::HighContrast,
        &[("category-a", "#ffffff")],
    );
    load(&mut e, two_seats([0.0, 0.0], [1.0, 1.0]));
    let hc = Theme::builtin(BuiltinTheme::HighContrast);
    assert_eq!(
        texels(&e, 0, visual::AVAILABLE),
        [WHITE, hc.available_stroke, BLACK]
    );
    assert_eq!(texels(&e, 0, visual::UNAVAILABLE)[0], hc.unavailable_fill);
}

/// Sections of one row of two seats, in `idx` order, alternately at the left
/// and right ends of a long strip: chunks adjacent in `idx` are far apart.
fn interleaved(sections: u32) -> Vec<u8> {
    let nodes: Vec<String> = (0..sections)
        .map(|i| {
            let x = if i % 2 == 0 { i * 50 } else { 100_000 + i * 50 };
            let id = i * 3 + 1;
            format!(
                r#"{{"id":{id},"type":"section","label":"S{i}","category":"a","children":[{{"id":{},"type":"rowBlock","origin":[{x},0],"rows":[{{"rid":{},"seats":2,"oids":[[{},2]]}}]}}]}}"#,
                id + 1,
                id + 2,
                1000 + i * 2
            )
        })
        .collect();
    compile(&format!(
        r##"{{"schema":1,"name":"Strip","settings":{{"seatSize":40}},"nextOid":{},
          "categories":[{{"key":"a","label":"A","color":"#2563EB"}}],"nodes":[{}]}}"##,
        1000 + sections * 2,
        nodes.join(",")
    ))
}

/// Checks one frame's seat ranges against 04 §11: at most 64, non-empty,
/// sorted, disjoint and within the active floor, and covering every active
/// seat whose quad (half size plus the one-pixel anti-aliased edge, rotated
/// by the seat's angle, 04 §8) meets the view. The view is at ratio 1.
fn check_ranges(e: &E, ranges: &[Range<u32>]) {
    let loaded = e.scene.as_ref().unwrap();
    let active = &loaded.scene.active;
    assert!(ranges.len() <= MAX_SEAT_DRAW_CALLS, "{}", ranges.len());
    for r in ranges {
        assert!(r.start < r.end && active.start <= r.start && r.end <= active.end);
    }
    assert!(
        ranges.windows(2).all(|w| w[0].end < w[1].start),
        "{ranges:?}"
    );
    let c = cam(e);
    let a = c.to_world([0.0, 0.0], SIZE);
    let b = c.to_world([SIZE.width, SIZE.height], SIZE);
    let statics = &e.backend.buffers[&loaded.static_buf.0];
    for i in active.clone() {
        let s = &statics[i as usize * 12..][..12];
        let f = |o: usize| f64::from(f32::from_le_bytes(s[o..o + 4].try_into().unwrap()));
        let (x, y) = (f(0), f(4));
        let half = f64::from(u16::from_le_bytes([s[8], s[9]])) * 0.25 + 1.0 / c.zoom;
        let theta = f64::from(s[10]) * std::f64::consts::TAU / 256.0;
        let reach = half * (theta.cos().abs() + theta.sin().abs());
        let meets =
            x + reach >= a[0] && x - reach <= b[0] && y + reach >= a[1] && y - reach <= b[1];
        if meets {
            assert!(
                ranges.iter().any(|r| r.contains(&i)),
                "seat {i} at ({x}, {y}) meets the view but is not drawn: {ranges:?}"
            );
        }
    }
}

#[test]
fn culling_covers_the_view_within_the_draw_call_limit() {
    let rounded = {
        let mut doc = cavea_testkit::synthetic::stadium(12_000, 7);
        doc.settings.seat_shape = cavea_compiler::document::SeatShape::Rounded;
        cavea_compiler::compile(&doc).unwrap().public
    };
    let charts = [
        cavea_compiler::compile(&cavea_testkit::synthetic::stadium(20_000, 3))
            .unwrap()
            .public,
        rounded,
        compile(ARC),
        interleaved(150),
    ];
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let (mut partial, mut split) = (0, 0);
    for ccf in charts {
        let mut e = loaded(ccf);
        let active = e.scene.as_ref().unwrap().scene.active.clone();
        // Fitted: the whole floor is one range, one draw call.
        assert_eq!(e.backend.draws.last(), Some(&one(active.clone())));
        let b = e.scene.as_ref().unwrap().scene.bounds;
        let mut ranges = one(active.clone());
        for step in 0..60 {
            let size = b.width.max(b.height) * 10f32.powf(rng.range(-2.5, 0.3));
            let x = b.x + b.width * rng.range(-0.3, 1.3);
            let y = b.y + b.height * rng.range(-0.3, 1.3);
            set_camera(&mut e, [x - size / 2.0, y - size / 2.0, size, size]);
            let (draws, frames) = (e.backend.draws.len(), e.backend.targets.len());
            tick(&mut e, f64::from(step) * 16.0 + 16.0);
            // An unchanged camera draws no frame and keeps the last ranges.
            if e.backend.targets.len() > frames {
                ranges = e.backend.draws.get(draws).cloned().unwrap_or_default();
            }
            check_ranges(&e, &ranges);
            partial += usize::from(!ranges.is_empty() && ranges != one(active.clone()));
            split += usize::from(ranges.len() > 1);
        }
    }
    assert!(
        partial > 60 && split > 10,
        "{partial} partial, {split} split"
    );

    // A view of 76 chunks no two of which are adjacent in `idx`: 64 ranges,
    // merged across the smallest gaps, still covering every visible seat.
    let mut e = loaded(interleaved(150));
    set_camera(&mut e, [-100.0, -3000.0, 7600.0, 6000.0]);
    tick(&mut e, 16.0);
    let ranges = e.backend.draws.last().unwrap().clone();
    assert_eq!(ranges.len(), MAX_SEAT_DRAW_CALLS);
    check_ranges(&e, &ranges);

    // Off the chart (past what the clamp allows, so set directly): the frame
    // is cleared and no seat is drawn.
    let (draws, frames) = (e.backend.draws.len(), e.backend.targets.len());
    e.camera = Some(Camera {
        centre: [1e7, 1e7],
        zoom: 1.0,
        rotation: [1.0, 0.0],
    });
    e.dirty = true;
    tick(&mut e, 32.0);
    assert_eq!(e.backend.draws.len(), draws);
    assert_eq!(e.backend.targets.len(), frames + 1);
}

#[test]
fn visible_ranges_are_computed_only_when_the_view_or_chart_changes() {
    let mut e = loaded(compile(ARC));
    assert_eq!(e.culls, 1);
    // Idle: no frame, nothing computed.
    assert!(tick(&mut e, 16.0).is_empty());
    // State and theme changes redraw with the same ranges.
    e.apply_status(3..5, theme::status::UNAVAILABLE);
    tick(&mut e, 32.0);
    set_theme(&mut e, BuiltinTheme::Dark, &[]);
    tick(&mut e, 48.0);
    assert_eq!((e.backend.draws.len(), e.culls), (3, 1));
    // The camera, the viewport and the chart each recompute them once.
    set_camera(&mut e, [0.0, 0.0, 100.0, 100.0]);
    tick(&mut e, 64.0);
    assert_eq!(e.culls, 2);
    resize(&mut e, 400.0, 300.0, 2.0, 3.0);
    tick(&mut e, 80.0);
    assert_eq!(e.culls, 3);
    load(&mut e, compile(ARC));
    tick(&mut e, 96.0);
    assert_eq!((e.backend.draws.len(), e.culls), (6, 4));
}
