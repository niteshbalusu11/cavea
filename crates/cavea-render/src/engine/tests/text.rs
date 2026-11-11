//! Seat labels (04 §7 pass 9, §12): the atlas command, the detail level, the
//! per-chunk label buffers and their LRU, and ink that follows seat state
//! without touching them.

use cavea_format::full_label::seat_label;

use super::*;
use crate::{
    protocol::BuiltinTheme,
    text::{self, LABEL_CHUNKS, LABEL_STRIDE, glyph_code},
};

/// `n` sections 2,000 cu apart along x, each one row of three chair seats
/// (size 40) numbered 1 to 3, so each is one render chunk and a view at
/// 20 px seats shows exactly one. Section 0 is turned 90°; in section 1 the
/// seats are labelled `W1♿` (outside the atlas), `ABCDE` (too long) and `7`.
fn line(n: u32) -> Vec<u8> {
    let nodes: Vec<String> = (0..n)
        .map(|i| {
            let (id, oid) = (i * 3 + 1, 10_000 + i * 3);
            let rotation = if i == 0 { 90 } else { 0 };
            let overrides = if i == 1 {
                format!(
                    r#","seatOverrides":{{"{oid}":{{"label":"W1♿"}},"{}":{{"label":"ABCDE"}},"{}":{{"label":"7"}}}}"#,
                    oid + 1,
                    oid + 2
                )
            } else {
                String::new()
            };
            format!(
                r#"{{"id":{id},"type":"section","label":"S{i}","category":"a","children":[{{"id":{},"type":"rowBlock","origin":[{},0],"rotation":{rotation},"rows":[{{"rid":{},"seats":3,"oids":[[{oid},3]]}}]{overrides}}}]}}"#,
                id + 1,
                i * 2000,
                id + 2,
            )
        })
        .collect();
    compile(&format!(
        r##"{{"schema":1,"name":"Line","settings":{{"seatSize":40,"seatShape":"chair"}},"nextOid":{},
          "categories":[{{"key":"a","label":"A","color":"#2563EB"}}],"nodes":[{}]}}"##,
        10_000 + n * 3,
        nodes.join(",")
    ))
}

fn atlas() -> Command {
    let [w, h] = text::ATLAS_SIZE;
    Command::SetGlyphAtlas {
        rgba: vec![0; (w * h * 4) as usize],
    }
}

/// The line chart in an 800 × 600 view after its first frame, with the atlas.
fn lined(n: u32) -> E {
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart { ccf: line(n) });
    e.handle(atlas());
    resize(&mut e, W, H, 1.0, 3.0);
    tick(&mut e, 0.0);
    e
}

/// Centres the camera on chunk `c`'s middle seat with seats `s` CSS pixels
/// large and draws a frame.
fn view_chunk(e: &mut E, c: usize, s: f64, t: f64) {
    let loaded = e.scene.as_ref().unwrap();
    let chunk = &loaded.scene.chart.chunks[c];
    let [x, y] = loaded.scene.pos[chunk.seat_first as usize + 1];
    e.camera = Some(Camera {
        centre: [f64::from(x), f64::from(y)],
        zoom: s / f64::from(loaded.scene.seat_size),
        rotation: [1.0, 0.0],
    });
    tick(e, t);
}

fn label_buffer(e: &E) -> u32 {
    e.scene.as_ref().unwrap().seat_labels.buffer().unwrap().0
}

/// The glyph draws since `from`, with their label buffer bases.
fn glyph_draws(e: &E, from: usize) -> &[(Vec<Range<u32>>, Vec<u32>)] {
    &e.backend.glyph_draws[from..]
}

fn chunk_seats(e: &E, c: usize) -> Range<u32> {
    let chunk = &e.scene.as_ref().unwrap().scene.chart.chunks[c];
    chunk.seat_first..chunk.seat_first + chunk.seat_count
}

/// Writes to the label buffer since `from`.
fn label_writes(e: &E, from: usize) -> usize {
    let buffer = label_buffer(e);
    e.backend.buffer_writes[from..]
        .iter()
        .filter(|w| w.0 == buffer)
        .count()
}

/// A label instance decoded: centre, em, characters, pen positions in em.
fn instance(e: &E, at: usize) -> ([f32; 2], f32, String, Vec<f32>) {
    let bytes = &e.backend.buffers[&label_buffer(e)][at * LABEL_STRIDE..][..LABEL_STRIDE];
    let f = |i: usize| f32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    let codes = bytes[12..16].iter().take_while(|&&c| c != 0);
    let label = codes
        .map(|&c| char::from(text::GLYPHS[usize::from(c) - 1].ch))
        .collect::<String>();
    let pens = bytes[16..16 + label.len()]
        .iter()
        .map(|&p| f32::from(p as i8) / 64.0)
        .collect();
    ([f(0), f(4)], f(8), label, pens)
}

#[test]
fn labels_are_drawn_at_the_detail_level_for_exactly_the_visible_chunks() {
    let mut e = lined(4);
    assert_eq!(e.scene.as_ref().unwrap().scene.chart.chunks.len(), 4);
    for (s, labelled) in [
        (10.0, false),
        (20.0, true),
        (64.0, true),
        (13.0, true),
        (12.0, false),
    ] {
        // 13 px is inside the detail level's hysteresis band, 12 px below it.
        let from = e.backend.glyph_draws.len();
        view_chunk(&mut e, 2, s, 0.0);
        let draws = glyph_draws(&e, from);
        if labelled {
            assert_eq!(draws, [(vec![chunk_seats(&e, 2)], vec![0])], "{s} px");
        } else {
            assert!(draws.is_empty(), "{s} px");
        }
    }
    // A view of two chunks draws both, each from its own slot.
    let from = e.backend.glyph_draws.len();
    let [a, b] =
        [0, 1].map(|c| e.scene.as_ref().unwrap().scene.pos[chunk_seats(&e, c).start as usize]);
    let mid = [f64::from(a[0] + b[0]) / 2.0, f64::from(a[1] + b[1]) / 2.0];
    e.camera = Some(Camera {
        centre: mid,
        zoom: 0.4,
        rotation: [1.0, 0.0],
    });
    tick(&mut e, 0.0);
    let (ranges, bases) = &glyph_draws(&e, from)[0];
    assert_eq!(ranges, &[chunk_seats(&e, 0), chunk_seats(&e, 1)]);
    assert_eq!(bases, &[3, 6], "chunk 2 holds slot 0, three seats a slot");
    assert_eq!(e.label_stats().draws, 2);
}

#[test]
fn a_view_of_many_new_chunks_builds_two_a_frame_and_asks_for_more() {
    let mut e = lined(3);
    // 1,600 px wide: at 14.4 px seats the view is 4,444 cu, all three chunks.
    resize(&mut e, 1600.0, H, 1.0, 3.0);
    e.camera = Some(Camera {
        centre: [2055.0, 0.0],
        zoom: 0.3,
        rotation: [1.0, 0.0],
    });
    // 12 px seats: the seats level, then the detail level on a zoom in.
    tick(&mut e, 0.0);
    assert!(e.backend.glyph_draws.is_empty());
    e.camera.as_mut().unwrap().zoom = 0.36;
    let events = tick(&mut e, 16.0);
    assert!(wants_frame(&events));
    let draw = |e: &E| e.backend.glyph_draws.last().unwrap().0.len();
    assert_eq!((draw(&e), e.label_stats().built), (2, 2));
    let events = tick(&mut e, 32.0);
    assert_eq!((draw(&e), e.label_stats().built), (3, 3));
    assert!(
        !wants_frame(&events),
        "idle again once every label is built"
    );
}

#[test]
fn a_chunk_is_built_once_and_states_themes_and_pans_upload_nothing_to_it() {
    let mut e = lined(3);
    snapshot(&mut e, 1, &[]);
    view_chunk(&mut e, 2, 20.0, 0.0);
    assert_eq!(e.label_stats().built, 1);
    let writes = e.backend.buffer_writes.len();
    let draws = e.backend.glyph_draws.len();
    let seat = chunk_seats(&e, 2).start;
    // A delta, a selection, a theme and a pan within the chunk each draw a
    // frame with labels, from the same buffer.
    delta(&mut e, 1, &[(seat, 1)]);
    tick(&mut e, 16.0);
    e.handle(Command::Select {
        objects: vec![crate::protocol::ObjectRef::Idx(seat + 1)],
    });
    tick(&mut e, 32.0);
    e.handle(Command::SetTheme {
        base: BuiltinTheme::Dark,
        tokens: Vec::new(),
    });
    tick(&mut e, 48.0);
    e.camera.as_mut().unwrap().centre[0] += 30.0;
    tick(&mut e, 64.0);
    assert_eq!(e.backend.glyph_draws.len() - draws, 4);
    assert_eq!(label_writes(&e, writes), 0);
    assert_eq!(e.label_stats().built, 1);
    // The dynamic buffer, which the labels read their ink from, did change.
    let dynamic = e.scene.as_ref().unwrap().dynamic_buf.0;
    assert!(
        e.backend.buffer_writes[writes..]
            .iter()
            .any(|w| w.0 == dynamic)
    );

    // Loading another chart releases the label buffer.
    let buffer = label_buffer(&e);
    e.handle(Command::LoadChart { ccf: line(2) });
    assert!(
        e.backend
            .destroyed
            .contains(&ResourceId::Buffer(BufferId(buffer)))
    );
    assert!(e.scene.as_ref().unwrap().seat_labels.buffer().is_none());
}

#[test]
fn the_lru_evicts_the_least_recently_drawn_chunk_and_rebuilds_it_on_return() {
    let n = LABEL_CHUNKS + 2;
    let mut e = lined(n as u32);
    for c in 0..=LABEL_CHUNKS {
        view_chunk(&mut e, c, 20.0, c as f64);
    }
    let stats = e.label_stats();
    assert_eq!((stats.built, stats.cached), (65, 64));
    let slot_of = |e: &E, c: usize| e.scene.as_ref().unwrap().seat_labels.slot(c);
    assert_eq!(
        slot_of(&e, 0),
        None,
        "the 65th chunk took the first one's slot"
    );
    assert_eq!(slot_of(&e, LABEL_CHUNKS), Some(0));
    // Chunk 1 is still kept: drawn without building.
    view_chunk(&mut e, 1, 20.0, 100.0);
    assert_eq!(e.label_stats().built, 65);
    // Chunk 0 comes back: rebuilt, in the slot of chunk 2, now the least
    // recently drawn.
    let slot = slot_of(&e, 2);
    view_chunk(&mut e, 0, 20.0, 116.0);
    assert_eq!(e.label_stats().built, 66);
    assert_eq!((slot_of(&e, 0), slot_of(&e, 2)), (slot, None));
    assert_eq!(e.label_stats().cached, 64);
}

#[test]
fn instances_are_the_seat_labels_centred_on_the_seats_and_upright() {
    let mut e = lined(2);
    view_chunk(&mut e, 0, 20.0, 0.0);
    view_chunk(&mut e, 1, 20.0, 16.0);
    let loaded = e.scene.as_ref().unwrap();
    let scene = &loaded.scene;
    let slot = |c| usize::from(loaded.seat_labels.slot(c).unwrap()) * 3;
    // Section 0 is turned 90°: its chairs are drawn at 90° (64 of 256)...
    let statics = scene.encode_static();
    for (k, idx) in chunk_seats(&e, 0).enumerate() {
        assert_eq!(statics[idx as usize * 12 + 10], 64);
        // ...and its labels are upright: an instance has no angle; its
        // glyphs run along +x, centred on the seat's centre.
        let (pos, em, label, pens) = instance(&e, slot(0) + k);
        let expected = seat_label(&scene.chart, idx).unwrap().seat;
        assert_eq!(label, expected.as_str());
        assert_eq!(label, (k + 1).to_string());
        assert_eq!(pos, scene.pos[idx as usize]);
        assert_eq!(em, text::label_em(40.0, 1, 0.5));
        assert_eq!(pens, [-0.25], "one digit 0.5 em wide, centred");
    }
    // Section 1: `W1♿` and `ABCDE` are skipped whole; `7` is drawn.
    let labels: Vec<String> = (0..3).map(|k| instance(&e, slot(1) + k).2).collect();
    assert_eq!(labels, ["", "", "7"]);
    let skipped = instance(&e, slot(1));
    assert_eq!(skipped, ([0.0; 2], 0.0, String::new(), vec![]));
    assert!(glyph_code('7').is_some());
}

#[test]
fn nothing_is_drawn_or_broken_before_the_atlas_arrives_or_if_it_never_does() {
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart { ccf: line(2) });
    resize(&mut e, W, H, 1.0, 3.0);
    tick(&mut e, 0.0);
    view_chunk(&mut e, 1, 20.0, 16.0);
    for t in [32.0, 48.0] {
        let events = tick(&mut e, t);
        assert!(!events.iter().any(|e| matches!(e, Event::Error { .. })));
    }
    assert!(e.backend.glyph_draws.is_empty());
    assert!(e.scene.as_ref().unwrap().seat_labels.buffer().is_none());
    assert!(e.backend.calls.iter().any(|c| c.0 == Pipeline::Shape));

    // The wrong size is refused and changes nothing.
    e.drain_events().for_each(drop);
    e.handle(Command::SetGlyphAtlas { rgba: vec![0; 12] });
    let events: Vec<Event> = e.drain_events().collect();
    assert!(matches!(
        events.as_slice(),
        [Event::Error {
            code: ErrorCode::InvalidInput,
            ..
        }]
    ));
    tick(&mut e, 64.0);
    assert!(e.backend.glyph_draws.is_empty());

    // The atlas arriving later asks for a frame, which draws the labels.
    e.handle(atlas());
    assert!(e.drain_events().any(|event| event == Event::NeedFrame));
    tick(&mut e, 80.0);
    assert_eq!(e.backend.glyph_draws.len(), 1);
}

/// Native timings for 04 §12: building one 4,096-seat chunk's labels, and a
/// frame at detail zoom with and without labels. Run with `cargo test
/// --release -p cavea-render measure_labels -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, not a test"]
fn measure_labels() {
    let preset = cavea_testkit::presets::find("stress-500k").unwrap();
    let ccf = cavea_compiler::compile(&preset.generate(1)).unwrap().public;
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart { ccf });
    resize(&mut e, 1920.0, 1080.0, 1.0, 3.0);
    tick(&mut e, 0.0);
    let scene = &e.scene.as_ref().unwrap().scene;
    let (c, chunk) = (scene.chart.chunks.iter().enumerate())
        .max_by_key(|(_, c)| c.seat_count)
        .unwrap();
    let seats = chunk.seat_count;
    let mut labels = text::Labels::default();
    let (mut ranges, mut bases) = (Vec::new(), Vec::new());
    labels
        .prepare(&mut e.backend, scene, &[0], &mut ranges, &mut bases)
        .unwrap();
    let start = std::time::Instant::now();
    let runs: u32 = 200;
    for _ in 0..runs {
        labels
            .rebuild_for_measurement(&mut e.backend, scene, c as u32)
            .unwrap();
    }
    let build = start.elapsed().as_secs_f64() * 1000.0 / f64::from(runs);
    println!("label build: {seats}-seat chunk in {build:.3} ms");
    // Creating the buffer of 64 slots of the largest chunk, once per chart.
    let start = std::time::Instant::now();
    let mut labels = text::Labels::default();
    labels
        .prepare(&mut e.backend, scene, &[], &mut ranges, &mut bases)
        .unwrap();
    let create = start.elapsed().as_secs_f64() * 1000.0;
    // Eight new chunks in view: two are built.
    let visible: Vec<u32> = (0..8).collect();
    let start = std::time::Instant::now();
    let waiting = labels
        .prepare(&mut e.backend, scene, &visible, &mut ranges, &mut bases)
        .unwrap();
    let first = start.elapsed().as_secs_f64() * 1000.0;
    println!(
        "label buffer created in {create:.3} ms; a frame over 8 new chunks builds {} in {first:.3} ms, more waiting: {waiting}",
        labels.built
    );

    // A frame at 20 px seats over the largest chunk, redrawn by a status
    // change each time, without labels and then with them (built already).
    let frame = |e: &mut E, from: u64| {
        view_chunk(e, c, 20.0, 0.0);
        let seat = chunk_seats(e, c).start;
        let start = std::time::Instant::now();
        for i in 0..runs {
            let seq = from + u64::from(i);
            delta(e, seq, &[(seat, (i % 2) as u8)]);
            e.handle(Command::Tick {
                time_ms: f64::from(i),
            });
        }
        start.elapsed().as_secs_f64() * 1e6 / f64::from(runs)
    };
    snapshot(&mut e, 0, &[]);
    let without = frame(&mut e, 0);
    e.handle(atlas());
    let with = frame(&mut e, u64::from(runs));
    let stats = e.label_stats();
    println!(
        "frame at detail (CPU, fake backend): {without:.2} µs without labels, {with:.2} µs with ({} label draws, {} chunks built)",
        stats.draws, stats.built
    );
}
