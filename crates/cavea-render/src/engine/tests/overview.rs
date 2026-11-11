//! Level of detail, the mesh passes, section counters and tints (04 §5, §7,
//! §10), and how pulses compose with them (04 §17).

use super::*;
use crate::{
    protocol::{BuiltinTheme, ObjectRef},
    theme::visual,
};

/// Two floors. The ground floor: section A (category a, two rows of four),
/// section B (category b, one row of four), a stage below the seats and a
/// line above them. The upper floor: section C and a shape of its own.
const VENUE: &str = r##"{"schema":1,"name":"Venue","settings":{"seatSize":10},"nextOid":500,
  "categories":[{"key":"a","label":"A","color":"#2563EB"},{"key":"b","label":"B","color":"#B45309"}],
  "floors":[{"id":1,"name":"Ground","level":0},{"id":2,"name":"Upper","level":1}],
  "nodes":[
   {"id":101,"type":"section","floor":1,"label":"A","category":"a","children":[
     {"id":102,"type":"rowBlock","rows":[{"rid":103,"seats":4,"oids":[[10,4]]},{"rid":104,"seats":4,"oids":[[20,4]]}]}]},
   {"id":105,"type":"section","floor":1,"label":"B","category":"b","origin":[400,0],"children":[
     {"id":106,"type":"rowBlock","rows":[{"rid":107,"seats":4,"oids":[[30,4]]}]}]},
   {"id":108,"type":"shape","floor":1,"kind":"rect","origin":[200,-300],"size":[400,100],"style":{"fill":"#1F2937"}},
   {"id":109,"type":"shape","floor":1,"kind":"line","points":[[0,0],[500,0]],"style":{"stroke":"#FF0000","strokeWidth":4},"layer":"above"},
   {"id":110,"type":"section","floor":2,"label":"C","category":"a","origin":[0,3000],"children":[
     {"id":111,"type":"rowBlock","rows":[{"rid":112,"seats":2,"oids":[[40,2]]}]}]},
   {"id":113,"type":"shape","floor":2,"kind":"rect","origin":[0,3200],"size":[100,100],"style":{"fill":"#00FF00"}}]}"##;

fn venue() -> E {
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart {
        ccf: compile(VENUE),
    });
    resize(&mut e, W, H, 1.0, 3.0);
    tick(&mut e, 0.0);
    e
}

/// Centres the first seat with the default seat `s` CSS pixels large (set
/// directly, past the zoom limits) and returns the level reported, if any.
fn at_seat_px(e: &mut E, s: f64, t: f64) -> Option<Lod> {
    let loaded = e.scene.as_ref().unwrap();
    let [x, y] = loaded.scene.pos[0];
    e.camera = Some(Camera {
        centre: [f64::from(x), f64::from(y)],
        zoom: s / f64::from(loaded.scene.seat_size),
        rotation: [1.0, 0.0],
    });
    tick(e, t).into_iter().find_map(|event| match event {
        Event::CameraChanged { lod, .. } => Some(lod),
        _ => None,
    })
}

/// The draws of the last frame: pipeline, ranges and opacity.
fn frame_calls(e: &E, from: usize) -> Vec<(Pipeline, Vec<Range<u32>>, f32)> {
    e.backend.calls[from..].to_vec()
}

/// The index ranges of the venue's mesh: the ground floor's decoration below
/// and above, its section fills, and every upper-floor range.
fn venue_ranges(e: &E) -> [Range<u32>; 3] {
    let chart = &e.scene.as_ref().unwrap().scene.chart;
    let draw = |floor, layer| {
        let d = chart
            .mesh
            .draws
            .iter()
            .find(|d| d.floor == floor && d.layer == layer);
        d.map(|d| d.index_first..d.index_first + d.index_count)
            .unwrap()
    };
    let [a, b] = [&chart.sections[0], &chart.sections[1]];
    assert_eq!(a.mesh_first + a.mesh_count, b.mesh_first);
    [
        draw(0, Layer::Below),
        a.mesh_first..b.mesh_first + b.mesh_count,
        draw(0, Layer::Above),
    ]
}

#[test]
fn passes_draw_the_active_floor_in_order_by_level() {
    let mut e = venue();
    let [below, fills, above] = venue_ranges(&e);
    let seats = 0..12;
    use Pipeline::{Mesh, Shape};

    let n = e.backend.calls.len();
    assert_eq!(at_seat_px(&mut e, 1.0, 16.0), Some(Lod::Overview));
    assert_eq!(
        frame_calls(&e, n),
        [
            (Mesh, vec![below.clone()], 1.0),
            (Mesh, vec![fills.clone()], 1.0),
            (Mesh, vec![above.clone()], 1.0),
        ]
    );
    let n = e.backend.calls.len();
    assert_eq!(at_seat_px(&mut e, 3.5, 32.0), Some(Lod::CrossFade));
    assert_eq!(
        frame_calls(&e, n),
        [
            (Mesh, vec![below.clone()], 1.0),
            (Mesh, vec![fills.clone()], 0.5),
            (Shape, vec![seats.clone()], 0.5),
            (Mesh, vec![above.clone()], 1.0),
        ]
    );
    for (s, lod) in [(8.0, Lod::Seats), (40.0, Lod::Detail)] {
        let n = e.backend.calls.len();
        assert_eq!(at_seat_px(&mut e, s, 48.0), Some(lod));
        let calls = frame_calls(&e, n);
        let pipelines: Vec<_> = calls.iter().map(|c| (c.0, c.2)).collect();
        assert_eq!(
            pipelines,
            [(Mesh, 1.0), (Shape, 1.0), (Mesh, 1.0)],
            "{lod:?}"
        );
        assert_eq!(calls[0].1, vec![below.clone()]);
        assert_eq!(calls[2].1, vec![above.clone()]);
    }
    // Overlays come last: hover a seat.
    let seat = e.scene.as_ref().unwrap().scene.pos[0];
    let at = cam(&e)
        .to_screen(seat.map(f64::from), SIZE)
        .map(|v| v as f32);
    let n = e.backend.calls.len();
    ptr(&mut e, Move, 1, MOUSE, at, 64.0);
    tick(&mut e, 64.0);
    let order: Vec<_> = frame_calls(&e, n).iter().map(|c| c.0).collect();
    assert_eq!(order, [Mesh, Shape, Mesh, Shape]);
    // The upper floor's ranges are never drawn.
    let chart = &e.scene.as_ref().unwrap().scene.chart;
    let upper: Vec<_> = (chart.sections[2].mesh_first..)
        .take(chart.sections[2].mesh_count as usize)
        .chain(
            chart
                .mesh
                .draws
                .iter()
                .filter(|d| d.floor == 1)
                .flat_map(|d| d.index_first..d.index_first + d.index_count),
        )
        .collect();
    assert!(!upper.is_empty());
    for (pipeline, ranges, _) in &e.backend.calls {
        if *pipeline == Mesh {
            assert!(ranges.iter().all(|r| upper.iter().all(|i| !r.contains(i))));
        }
    }
}

#[test]
fn levels_have_hysteresis_at_every_boundary() {
    use Lod::{CrossFade as F, Detail as D, Overview as O, Seats as S};
    let mut e = venue();
    // Up and down across each threshold: a level is entered at its threshold
    // and left only below 90% of it.
    let sweep = [
        (1.0, O),
        (2.4, O),
        (2.5, F),
        (2.3, F),
        (2.26, F),
        (2.24, O),
        (2.49, O),
        (4.99, F),
        (5.0, S),
        (4.6, S),
        (4.51, S),
        (4.49, F),
        (4.9, F),
        (13.9, S),
        (14.0, D),
        (12.7, D),
        (12.59, S),
        (13.5, S),
        (64.0, D),
        (0.5, O),
    ];
    let mut t = 16.0;
    for (s, lod) in sweep {
        let calls = e.backend.calls.len();
        assert_eq!(at_seat_px(&mut e, s, t), Some(lod), "s = {s}");
        let drawn: Vec<_> = frame_calls(&e, calls);
        let shapes = drawn.iter().filter(|c| c.0 == Pipeline::Shape).count();
        let meshes = drawn.iter().filter(|c| c.0 == Pipeline::Mesh).count();
        match lod {
            O => assert_eq!((shapes, meshes), (0, 3), "s = {s}: no seats at overview"),
            S | D => assert_eq!((shapes, meshes), (1, 2), "s = {s}: no fills"),
            F => {}
        }
        // The seats' opacity and the fills' are complementary and depend
        // only on the seat size: 0 to 2.5 px, rising to 1 at 4.5 px.
        let alpha = ((s - 2.5) / 2.0).clamp(0.0, 1.0) as f32;
        assert!((e.seat_alpha() - alpha).abs() < 1e-6, "s = {s}");
        for (pipeline, _, a) in &drawn {
            if *pipeline == Pipeline::Shape {
                assert_eq!(*a, e.seat_alpha());
            }
        }
        if let Some(fill) = drawn
            .get(1)
            .filter(|c| c.0 == Pipeline::Mesh && alpha < 1.0)
        {
            assert_eq!(fill.2, 1.0 - e.seat_alpha());
        }
        // A still camera draws nothing, mid-fade included.
        assert!(tick(&mut e, t + 8.0).is_empty());
        t += 16.0;
    }

    // Without sections there is no overview or cross-fade: seats at any size.
    let mut e = engine();
    for (s, lod) in [(0.5, S), (3.0, S), (14.0, D), (13.0, D), (12.0, S)] {
        assert_eq!(at_seat_px(&mut e, s, t), Some(lod), "s = {s}");
        assert_eq!(e.seat_alpha(), 1.0);
        t += 16.0;
    }
}

/// Seats not drawn unavailable per section, counted from scratch.
fn recount(e: &E) -> Vec<u32> {
    let scene = &e.scene.as_ref().unwrap().scene;
    let mut open = vec![0; scene.chart.sections.len()];
    for (i, &v) in scene.visual.iter().enumerate() {
        if let Some(n) = open.get_mut(scene.section_of(i) as usize) {
            *n += u32::from(v != visual::UNAVAILABLE);
        }
    }
    open
}

#[test]
fn section_counters_match_a_recount_and_count_own_seats_as_open() {
    let mut e = Engine::new(Fake::default());
    let ccf = cavea_compiler::compile(&cavea_testkit::synthetic::stadium(3_000, 5))
        .unwrap()
        .public;
    e.handle(Command::LoadChart { ccf });
    resize(&mut e, W, H, 1.0, 3.0);
    tick(&mut e, 0.0);
    let n = e.objects().unwrap() as u32;
    let open = |e: &E| e.scene.as_ref().unwrap().sections.open.clone();
    assert_eq!(open(&e), recount(&e));
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut seq = 1;
    let mut t = 16.0;
    for round in 0..20 {
        if round % 5 == 0 {
            let unavailable: Vec<u32> = (0..n).filter(|_| rng.range(0.0, 1.0) < 0.4).collect();
            snapshot(&mut e, seq, &unavailable);
        }
        for _ in 0..3 {
            let mut idx: Vec<u32> = (0..40)
                .map(|_| rng.range(0.0, f64::from(n)) as u32)
                .collect();
            idx.sort_unstable();
            idx.dedup();
            let changes: Vec<(u32, u8)> = idx
                .iter()
                .map(|&i| (i, u8::from(rng.range(0.0, 1.0) < 0.6)))
                .collect();
            delta(&mut e, seq, &changes);
            seq += 1;
            assert_eq!(open(&e), recount(&e), "round {round}");
        }
        tick(&mut e, t);
        t += 16.0;
    }

    // The buyer's own seat counts as open while selected, even once a delta
    // marks it unavailable to everyone else, and after deselecting while its
    // hold is in flight (it is drawn available, 04 §16, §17).
    snapshot(&mut e, seq, &[]);
    let section = e.scene.as_ref().unwrap().scene.section_of(0) as usize;
    let seats = e.scene.as_ref().unwrap().scene.chart.sections[section].seat_count;
    assert_eq!(open(&e)[section], seats);
    e.handle(Command::Select {
        objects: vec![ObjectRef::Idx(0)],
    });
    delta(&mut e, seq, &[(0, 1), (1, 1)]);
    assert_eq!(open(&e)[section], seats - 1);
    assert_eq!(open(&e), recount(&e));
    e.handle(Command::ClearSelection);
    assert_eq!(open(&e)[section], seats - 1);
    assert_eq!(open(&e), recount(&e));
}

/// The colour bytes of every vertex of section `s`'s outline, as uploaded.
fn tints(e: &E, s: usize) -> Vec<[u8; 4]> {
    let loaded = e.scene.as_ref().unwrap();
    let chart = &loaded.scene.chart;
    let section = &chart.sections[s];
    let bytes = &e.backend.buffers[&loaded.mesh.unwrap()[0].0];
    let indices = &chart.mesh.indices[section.mesh_first as usize..][..section.mesh_count as usize];
    indices
        .iter()
        .map(|&v| bytes[v as usize * 12 + 8..][..4].try_into().unwrap())
        .collect()
}

#[test]
fn section_tints_follow_availability_in_every_theme() {
    let blue = [0x25, 0x63, 0xEB];
    for theme in [
        BuiltinTheme::Light,
        BuiltinTheme::Dark,
        BuiltinTheme::HighContrast,
    ] {
        let mut e = venue();
        e.handle(Command::SetTheme {
            base: theme,
            tokens: Vec::new(),
        });
        let th = Theme::builtin(theme);
        let grey = th.unavailable_fill;
        // The colour's weight is the strength times the open share.
        let expect = |open: f64| {
            let w = f64::from(th.section_tint) * open;
            let mix = |k: usize| {
                (f64::from(grey[k]) + (f64::from(blue[k]) - f64::from(grey[k])) * w).round() as u8
            };
            [mix(0), mix(1), mix(2), 255]
        };
        snapshot(&mut e, 1, &[]);
        tick(&mut e, 16.0);
        // All available, half (4 of section A's 8 seats), sold out.
        for (unavailable, open) in [
            (&[][..], 1.0),
            (&[0, 1, 2, 3][..], 0.5),
            (&[0, 1, 2, 3, 4, 5, 6, 7][..], 0.0),
        ] {
            snapshot(&mut e, 1, unavailable);
            tick(&mut e, 32.0);
            let got = tints(&e, 0);
            assert!(!got.is_empty());
            assert!(
                got.iter().all(|&c| c == expect(open)),
                "{theme:?} {open}: {got:?}"
            );
        }
        assert_eq!(tints(&e, 0)[0], [grey[0], grey[1], grey[2], 255]);
    }
}

#[test]
fn tints_are_rewritten_only_for_changed_sections_once_per_frame() {
    let mut e = venue();
    at_seat_px(&mut e, 1.0, 16.0);
    snapshot(&mut e, 1, &[]);
    tick(&mut e, 32.0);
    let loaded = e.scene.as_ref().unwrap();
    let vertices = loaded.mesh.unwrap()[0].0;
    let chart = &loaded.scene.chart;
    // A section's outline vertices are one span, in section order.
    let span = |s: usize| {
        let sec = &chart.sections[s];
        let idx = &chart.mesh.indices[sec.mesh_first as usize..][..sec.mesh_count as usize];
        let (lo, hi) = (idx.iter().min().unwrap(), idx.iter().max().unwrap());
        (lo * 12, (hi - lo + 1) as usize * 12)
    };
    let (a, b, c) = (span(0), span(1), span(2));
    let writes = |e: &E, from: usize| -> Vec<(u32, usize)> {
        e.backend.buffer_writes[from..]
            .iter()
            .filter(|w| w.0 == vertices)
            .map(|w| (w.1, w.2))
            .collect()
    };

    // Two deltas in section B before a frame at overview: one frame, one
    // write of B's vertices only.
    let (from, frames) = (e.backend.buffer_writes.len(), e.backend.targets.len());
    delta(&mut e, 1, &[(8, 1)]);
    delta(&mut e, 2, &[(9, 1)]);
    tick(&mut e, 48.0);
    assert_eq!(writes(&e, from), [b]);
    assert_eq!(e.backend.targets.len(), frames + 1);
    // A change in section A writes A's vertices only.
    let from = e.backend.buffer_writes.len();
    delta(&mut e, 3, &[(0, 1)]);
    tick(&mut e, 64.0);
    assert_eq!(writes(&e, from), [a]);
    // A theme change rewrites every section, upper floor included.
    let from = e.backend.buffer_writes.len();
    e.handle(Command::SetTheme {
        base: BuiltinTheme::Dark,
        tokens: Vec::new(),
    });
    tick(&mut e, 80.0);
    assert_eq!(writes(&e, from), [(a.0, c.0 as usize + c.1 - a.0 as usize)]);
}

#[test]
fn nothing_pulses_at_overview_and_a_pulse_ends_on_time_across_levels() {
    let mut e = venue();
    snapshot(&mut e, 1, &[]);
    // At overview a delta starts no pulse: one frame for the tint, then idle.
    at_seat_px(&mut e, 1.0, 16.0);
    delta(&mut e, 1, &[(0, 1)]);
    assert!(e.state.pulses.is_empty());
    assert_eq!(e.scene.as_ref().unwrap().scene.fx[0], 0);
    tick(&mut e, 32.0);
    assert!(tick(&mut e, 48.0).is_empty());

    // A pulse started at the seats level runs out at 400 ms even if the
    // camera goes to overview meanwhile, then the loop is idle.
    at_seat_px(&mut e, 8.0, 64.0);
    delta(&mut e, 2, &[(1, 1)]);
    assert_eq!(e.state.pulses.len(), 1);
    tick(&mut e, 80.0);
    assert_eq!(at_seat_px(&mut e, 1.0, 96.0), Some(Lod::Overview));
    let end = settle(&mut e, 112.0);
    assert_eq!(end, 464.0, "the batch began at the tick at 80 ms");
    assert_eq!(e.scene.as_ref().unwrap().scene.fx[1], 0);
}

#[test]
fn a_seat_taken_again_pulses_its_full_time_and_nothing_pulses_before_a_frame() {
    let mut e = venue();
    snapshot(&mut e, 1, &[]);
    at_seat_px(&mut e, 8.0, 0.0);
    let fx = |e: &E| e.scene.as_ref().unwrap().scene.fx[0];
    // Taken (batch from 16 ms to 416), freed, taken again (batch to 616).
    delta(&mut e, 1, &[(0, 1)]);
    tick(&mut e, 16.0);
    delta(&mut e, 2, &[(0, 0)]);
    tick(&mut e, 116.0);
    delta(&mut e, 3, &[(0, 1)]);
    tick(&mut e, 216.0);
    tick(&mut e, 420.0);
    assert_eq!(fx(&e), 4, "the newer batch still runs");
    tick(&mut e, 620.0);
    assert_eq!(fx(&e), 0);

    // A delta before the first frame starts no pulse: the camera is not
    // placed yet, and the fit may be at overview.
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart {
        ccf: compile(VENUE),
    });
    resize(&mut e, W, H, 1.0, 3.0);
    snapshot(&mut e, 1, &[]);
    delta(&mut e, 1, &[(0, 1)]);
    assert!(e.state.pulses.is_empty());
}
