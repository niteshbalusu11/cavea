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
    load(&mut e, two_seats([0.0, 0.0], [10.0, 10.0]));
    assert_eq!(
        e.backend.destroyed,
        [
            ResourceId::Buffer(BufferId(s)),
            ResourceId::Buffer(BufferId(d)),
            ResourceId::Texture(TextureId(p))
        ]
    );
    // Only the new chart's three resources are alive, and it is what is drawn.
    assert_eq!(e.backend.buffers.len() + e.backend.textures.len(), 3);
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

#[test]
fn a_theme_change_rewrites_the_palette_and_no_instance_data() {
    let mut e = loaded(compile(ARC));
    let light = Theme::builtin(BuiltinTheme::Light);
    // Category rows take the chart's colours; unavailable is the same everywhere.
    assert_eq!(texels(&e, 0, visual::AVAILABLE)[0], [0x25, 0x63, 0xEB, 255]);
    assert_eq!(texels(&e, 1, visual::AVAILABLE)[0], [0xB4, 0x53, 0x09, 255]);
    for row in 0..3 {
        assert_eq!(
            texels(&e, row, visual::UNAVAILABLE),
            [
                light.unavailable_fill,
                light.unavailable_stroke,
                light.fg_muted
            ]
        );
    }
    let buffers = e.backend.buffers.clone();
    let token = |name: &str, value: &str| ThemeToken {
        name: name.into(),
        value: value.into(),
    };
    e.handle(Command::SetTheme {
        base: BuiltinTheme::Dark,
        tokens: vec![token("category-b", "#00ff00")],
    });
    let events = tick(&mut e, 16.0);
    assert!(events.is_empty(), "{events:?}");
    let dark = Theme::builtin(BuiltinTheme::Dark);
    assert_eq!(e.backend.texture_writes.len(), 1);
    assert_eq!(e.backend.clears.last(), Some(&dark.clear()));
    assert_eq!(texels(&e, 0, visual::AVAILABLE)[0], [0x25, 0x63, 0xEB, 255]);
    assert_eq!(texels(&e, 1, visual::AVAILABLE)[0], [0, 255, 0, 255]);
    assert_eq!(texels(&e, 2, visual::UNAVAILABLE)[0], dark.unavailable_fill);
    // Ink contrasts with the fill it sits on.
    assert_eq!(texels(&e, 1, visual::AVAILABLE)[2], [0, 0, 0, 255]);
    assert!(e.backend.buffer_writes.is_empty());
    assert_eq!(e.backend.buffers, buffers);

    // A bad token is refused and changes nothing.
    e.handle(Command::SetTheme {
        base: BuiltinTheme::HighContrast,
        tokens: vec![token("seat-unavailable-fill", "grey")],
    });
    assert!(matches!(
        e.drain_events().next(),
        Some(Event::Error {
            code: ErrorCode::InvalidInput,
            ..
        })
    ));
    assert_eq!(e.backend.texture_writes.len(), 1);
    // The theme outlives the chart.
    load(&mut e, two_seats([0.0, 0.0], [1.0, 1.0]));
    assert_eq!(texels(&e, 0, visual::UNAVAILABLE)[0], dark.unavailable_fill);
}

#[test]
fn culling_covers_the_view_within_the_draw_call_limit() {
    let doc = cavea_testkit::synthetic::stadium(20_000, 3);
    let mut e = loaded(cavea_compiler::compile(&doc).unwrap().public);
    let seats = e.scene.as_ref().unwrap().scene.pos.len() as u32;
    // Fitted: the whole chart is one range, one draw call.
    assert_eq!(e.backend.draws.last(), Some(&one(0..seats)));

    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut partial = 0;
    for step in 0..60 {
        let size = f64::from(rng.range(100.0, 6000.0));
        let (x, y) = (rng.range(-9000.0, 9000.0), rng.range(-9000.0, 9000.0));
        set_camera(&mut e, [x, y, size as f32, size as f32]);
        let draws = e.backend.draws.len();
        tick(&mut e, f64::from(step) * 16.0 + 16.0);
        let ranges = match e.backend.draws.get(draws) {
            Some(r) => r.clone(),
            None => Vec::new(),
        };
        assert!(ranges.len() <= MAX_SEAT_DRAW_CALLS);
        partial += usize::from(!ranges.is_empty() && ranges != one(0..seats));
        let c = cam(&e);
        let a = c.to_world([0.0, 0.0], SIZE);
        let b = c.to_world([SIZE.width, SIZE.height], SIZE);
        let scene = &e.scene.as_ref().unwrap().scene;
        let half = f64::from(scene.max_half);
        for (i, p) in scene.pos.iter().enumerate() {
            let [px, py] = p.map(f64::from);
            let inside =
                px + half >= a[0] && px - half <= b[0] && py + half >= a[1] && py - half <= b[1];
            if inside {
                let i = i as u32;
                assert!(
                    ranges.iter().any(|r| r.contains(&i)),
                    "seat {i} at step {step}"
                );
            }
        }
    }
    assert!(partial > 10, "too few partial views: {partial}");

    // Panned far off the chart: nothing is drawn (the clamp allows a third of
    // the view to show the chart, so pan the camera directly past it).
    let (draws, frames) = (e.backend.draws.len(), e.backend.targets.len());
    e.camera = Some(Camera {
        centre: [1e6, 1e6],
        zoom: 1.0,
        rotation: [1.0, 0.0],
    });
    e.dirty = true;
    tick(&mut e, 5000.0);
    assert_eq!(e.backend.draws.len(), draws);
    assert_eq!(
        e.backend.targets.len(),
        frames + 1,
        "the frame was still cleared"
    );
}
