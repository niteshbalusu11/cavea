//! Dynamic text, section labels, text objects and seat labels from the
//! dynamic atlas (04 §7 passes 9–10, §12), through `Engine::handle` with a
//! fake rasteriser that answers every request with a deterministic raster.

use super::*;
use crate::{
    label::{MAX_TEXT_INSTANCES, SECTION_MAX_PX, SECTION_MIN_PX, TEXT_STRIDE},
    text::atlas::{BUCKETS, Key, MAX_SIDE, REQUESTS_PER_BUILD, START_SIDE, Source, TextAtlas},
};

/// One range, as draws record them.
fn one(range: Range<u32>) -> Vec<Range<u32>> {
    vec![range]
}

/// Two floors. Ground: sections North (four rows of ten) and South (two
/// rows of ten), the text objects STAGE (centred, always shown) and Left
/// (left-aligned, turned 90°, from zoom 0.5). Upper: section Upper and a
/// text.
fn chart() -> Vec<u8> {
    compile(
        r##"{"schema":1,"name":"Labels","settings":{"seatSize":10},"nextOid":900,
  "categories":[{"key":"a","label":"A","color":"#2563EB"}],
  "floors":[{"id":1,"name":"Ground","level":0},{"id":2,"name":"Upper","level":1}],
  "nodes":[
   {"id":101,"type":"section","floor":1,"label":"North","category":"a","children":[
     {"id":102,"type":"rowBlock","rows":[{"rid":103,"seats":10,"oids":[[10,10]]},{"rid":104,"seats":10,"oids":[[20,10]]},
       {"rid":105,"seats":10,"oids":[[30,10]]},{"rid":106,"seats":10,"oids":[[40,10]]}]}]},
   {"id":107,"type":"section","floor":1,"label":"South","category":"a","origin":[0,1000],"children":[
     {"id":108,"type":"rowBlock","rows":[{"rid":109,"seats":10,"oids":[[50,10]]},{"rid":110,"seats":10,"oids":[[60,10]]}]}]},
   {"id":111,"type":"text","floor":1,"origin":[250,-400],"text":"STAGE","size":40,"color":"#112233","align":"center"},
   {"id":112,"type":"text","floor":1,"origin":[-700,100],"rotation":90,"text":"Left","size":20,"color":"#445566","align":"left","minZoom":0.5},
   {"id":115,"type":"section","floor":2,"label":"Upper","category":"a","origin":[0,5000],"children":[
     {"id":116,"type":"rowBlock","rows":[{"rid":117,"seats":2,"oids":[[70,2]]}]}]},
   {"id":118,"type":"text","floor":2,"origin":[0,5500],"text":"Hidden","size":40,"color":"#000000"}]}"##,
    )
}

/// The chart with the glyph atlas after its first frames, at the fit zoom,
/// with the text they asked for.
fn labelled() -> E {
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart { ccf: chart() });
    e.handle(super::text::atlas());
    resize(&mut e, W, H, 1.0, 3.0);
    rasterise(&mut e, 0.0);
    e
}

/// The text requests among `events`: id, text, px.
fn requests(events: &[Event]) -> Vec<(u32, String, u16)> {
    let request = |e: &Event| match e {
        Event::TextRequest { id, text, px, .. } => Some((*id, text.clone(), *px)),
        _ => None,
    };
    events.iter().filter_map(request).collect()
}

/// The fake rasteriser: answers each request with a raster 0.6 em a
/// character wide and 1.25 em tall, fully covered.
fn answer(e: &mut E, asked: &[(u32, String, u16)]) {
    for (id, text, px) in asked {
        let px = u32::from(*px);
        let width = (text.chars().count() as u32 * px * 3).div_ceil(5) + 4;
        let height = (px * 5).div_ceil(4) + 4;
        let pixels = vec![255; (width * height) as usize];
        e.handle(Command::SetText {
            id: *id,
            width,
            height,
            px,
            pixels,
        });
    }
}

/// Ticks at `t` and answers what was asked, until nothing more is asked.
fn rasterise(e: &mut E, t: f64) -> Vec<(u32, String, u16)> {
    let mut all = Vec::new();
    for i in 0..20 {
        let asked = requests(&tick(e, t + f64::from(i)));
        if asked.is_empty() {
            break;
        }
        answer(e, &asked);
        all.extend(asked);
    }
    tick(e, t + 50.0);
    all
}

/// Puts the camera on `centre` with the default seat `s` CSS pixels large.
fn look(e: &mut E, centre: [f64; 2], s: f64) {
    let seat = f64::from(e.scene.as_ref().unwrap().scene.seat_size);
    e.camera = Some(Camera {
        centre,
        zoom: s / seat,
        rotation: [1.0, 0.0],
    });
}

fn labels(e: &E) -> &crate::label::MapLabels {
    &e.scene.as_ref().unwrap().map_labels
}

/// The `text` instances of range `r`: anchor, em, raster em (texels),
/// anchor in the raster, angle, min and max em (quarter CSS px), colour.
#[allow(clippy::type_complexity)]
fn instances(e: &E, r: Range<u32>) -> Vec<([f32; 2], f32, u16, [i16; 2], u16, [u8; 2], [u8; 4])> {
    let buffer = labels(e).text_buffer().unwrap().0;
    let bytes = &e.backend.buffers[&buffer];
    r.map(|i| {
        let b = &bytes[i as usize * TEXT_STRIDE..][..TEXT_STRIDE];
        let f = |at: usize| f32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        let u = |at: usize| u16::from_le_bytes(b[at..at + 2].try_into().unwrap());
        let anchor = [u(20) as i16, u(22) as i16];
        (
            [f(0), f(4)],
            f(8),
            u(24),
            anchor,
            u(26),
            [b[29], b[30]],
            b[32..36].try_into().unwrap(),
        )
    })
    .collect()
}

/// The `text` and row label draws of the last frame since `from`.
fn draws_since(e: &E, from: usize) -> Vec<(Pipeline, Vec<Range<u32>>, f32)> {
    let draws = e.backend.text_draws[from..].iter();
    draws.map(|(p, _, r, a)| (*p, r.clone(), *a)).collect()
}

fn view_section(e: &mut E, section: usize, s: f64) {
    let centre = centre_of(e, section);
    look(e, centre, s);
}

fn centre_of(e: &E, section: usize) -> [f64; 2] {
    let b = e.scene.as_ref().unwrap().scene.chart.sections[section].bounds;
    [f64::from(b[0] + b[2]) / 8.0, f64::from(b[1] + b[3]) / 8.0]
}

#[test]
fn a_string_is_asked_for_once_per_bucket_drawn_once_supplied_and_never_when_idle() {
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart { ccf: chart() });
    resize(&mut e, W, H, 1.0, 3.0);
    // The first frame, at the fit zoom (the cross-fade), then overview.
    let mut asked = requests(&tick(&mut e, 0.0));
    view_section(&mut e, 0, 1.0);
    asked.extend(requests(&tick(&mut e, 16.0)));
    let mut texts: Vec<(&str, u16)> = asked.iter().map(|a| (a.1.as_str(), a.2)).collect();
    // The ground floor's sections and its text shown at these zooms (the
    // text in two buckets, the labels capped at 24 px in one); not the
    // upper floor's, nor Left (from zoom 0.5). Each once.
    texts.sort_unstable();
    texts.dedup_by_key(|t| t.0);
    let names: Vec<&str> = texts.iter().map(|t| t.0).collect();
    assert_eq!(names, ["North", "STAGE", "South"]);
    let mut pairs: Vec<(&str, u16)> = asked.iter().map(|a| (a.1.as_str(), a.2)).collect();
    let n = pairs.len();
    pairs.sort_unstable();
    pairs.dedup();
    assert_eq!(pairs.len(), n, "each string once per size");
    assert!(
        e.backend.text_draws.is_empty(),
        "nothing drawn before a raster"
    );
    // Asked once: the next frames ask nothing more while waiting.
    view_section(&mut e, 0, 1.01);
    assert!(requests(&tick(&mut e, 32.0)).is_empty());

    answer(&mut e, &asked);
    let from = e.backend.text_draws.len();
    let events = tick(&mut e, 48.0);
    assert!(requests(&events).is_empty());
    let draws = draws_since(&e, from);
    // Section labels at the fills' opacity (1 at overview), then the text.
    assert_eq!(
        draws,
        [
            (Pipeline::Text, one(0..2), 1.0),
            (Pipeline::Text, one(2..3), 1.0)
        ]
    );

    // Idle: no frame, no build, no upload.
    let (writes, textures, builds) = (
        e.backend.buffer_writes.len(),
        e.backend.texture_writes.len(),
        labels(&e).builds,
    );
    for t in 0..20 {
        assert!(tick(&mut e, 64.0 + f64::from(t) * 16.0).is_empty());
    }
    assert_eq!(e.backend.buffer_writes.len(), writes);
    assert_eq!(e.backend.texture_writes.len(), textures);
    assert_eq!(labels(&e).builds, builds);

    // Zooming in by more than √2 asks for the text again, a bucket up (the
    // section labels stay capped), and draws its old raster meanwhile.
    let stage = asked.iter().rev().find(|a| a.1 == "STAGE").map(|a| a.2);
    view_section(&mut e, 0, 2.0);
    let from = e.backend.text_draws.len();
    let again = requests(&tick(&mut e, 400.0));
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].1, "STAGE");
    assert!(Some(again[0].2) > stage);
    assert_eq!(draws_since(&e, from).len(), 2, "still drawn, scaled");
}

#[test]
fn section_labels_show_at_overview_and_cross_fade_at_their_anchor_within_the_clamp() {
    let mut e = labelled();
    let centre = centre_of(&e, 0);
    look(&mut e, centre, 1.0);
    rasterise(&mut e, 16.0);
    for (s, shown) in [
        (1.0, true),
        (3.0, true),
        (4.4, true),
        (6.0, false),
        (20.0, false),
    ] {
        look(&mut e, centre, s);
        e.dirty = true;
        let from = e.backend.text_draws.len();
        tick(&mut e, 200.0 + s);
        rasterise(&mut e, 300.0 + s);
        let sections = labels(&e).ranges[0].clone();
        let alpha = 1.0 - e.seat_alpha();
        let drawn = draws_since(&e, from);
        if shown {
            assert!(
                drawn.contains(&(Pipeline::Text, one(sections.clone()), alpha)),
                "{s}: {drawn:?}"
            );
            assert!(alpha > 0.0);
        } else {
            assert!(
                sections.is_empty() || !drawn.iter().any(|d| d.1 == [sections.clone()]),
                "{s}"
            );
        }
    }
    look(&mut e, centre, 1.0);
    rasterise(&mut e, 500.0);
    let chart = &e.scene.as_ref().unwrap().scene.chart;
    let (north, south) = (chart.sections[0], chart.sections[1]);
    let got = instances(&e, labels(&e).ranges[0].clone());
    assert_eq!(got.len(), 2);
    for (instance, section) in got.iter().zip([north, south]) {
        let (pos, em, _, _, angle, clamp, colour) = *instance;
        assert_eq!(pos, section.label_anchor.map(|v| v as f32 * 0.25));
        assert!(em > 0.0 && em <= f32::from(section.label_size) * 0.25);
        assert_eq!(
            clamp,
            [(SECTION_MIN_PX * 4.0) as u8, (SECTION_MAX_PX * 4.0) as u8]
        );
        assert_eq!((angle, colour), (0, e.theme.fg));
    }

    // South sells out: its label gets a second line, the message, smaller.
    let south_seats = south.seat_first..south.seat_first + south.seat_count;
    snapshot(&mut e, 1, &south_seats.collect::<Vec<_>>());
    let asked = rasterise(&mut e, 600.0);
    assert!(asked.iter().any(|a| a.1 == "Sold out"));
    let got = instances(&e, labels(&e).ranges[0].clone());
    assert_eq!(got.len(), 3, "North, South and its sold-out line");
    let (name, sold) = (got[1], got[2]);
    assert_eq!(name.0, sold.0, "one anchor");
    assert!(sold.1 < name.1);
    assert!(name.3[1] > sold.3[1], "the name above, the message below");
    // Hidden on its own below 8 px, so a narrow section keeps its name.
    assert_eq!(sold.5[0], 32);
    // A new message replaces it.
    e.handle(Command::SetMessage {
        key: "soldOut".into(),
        text: "Épuisé".into(),
    });
    assert!(rasterise(&mut e, 700.0).iter().any(|a| a.1 == "Épuisé"));
}

#[test]
fn text_objects_follow_floor_angle_alignment_and_minimum_zoom() {
    let mut e = labelled();
    let centre = centre_of(&e, 0);
    look(&mut e, centre, 2.0);
    let asked = rasterise(&mut e, 16.0);
    assert!(!asked.iter().any(|a| a.1 == "Left" || a.1 == "Hidden"));
    // Zoom 0.5 CSS px per cu (5 px seats): Left shows; Hidden never does.
    look(&mut e, centre, 5.0);
    let asked = rasterise(&mut e, 100.0);
    assert!(asked.iter().any(|a| a.1 == "Left"));
    assert!(!asked.iter().any(|a| a.1 == "Hidden"));
    let got = instances(&e, labels(&e).ranges[2].clone());
    let by_colour = |c: [u8; 4]| got.iter().find(|i| i.6 == c).copied().unwrap();
    let stage = by_colour([0x11, 0x22, 0x33, 255]);
    let left = by_colour([0x44, 0x55, 0x66, 255]);
    assert_eq!((stage.0, stage.1, stage.4), ([250.0, -400.0], 40.0, 0));
    assert_eq!((left.0, left.1, left.4), ([-700.0, 100.0], 20.0, 16384));
    // Centred on its position, or starting at it; vertically centred.
    let raster = |p: u16, chars: u16| (chars * p * 3).div_ceil(5) + 4;
    let half = (f32::from(raster(stage.2, 5)) / 2.0).round() as i16;
    assert_eq!(stage.3, [half, stage.3[1]]);
    assert_eq!(left.3[0], 0);
    assert_eq!(left.5, [0, 0], "drawn at any size, as authored");
}

#[test]
fn seat_labels_outside_the_baked_atlas_come_from_the_dynamic_one_a_few_a_frame() {
    // Section 1 of the line chart: `W1♿`, `ABCDE` and `7`; more such
    // labels in a row of forty.
    let overrides: Vec<String> = (0..40)
        .map(|i| format!(r#""{}":{{"label":"№{i}"}}"#, 100 + i))
        .collect();
    let json = format!(
        r##"{{"schema":1,"name":"N","settings":{{"seatSize":40}},"nextOid":200,
  "categories":[{{"key":"a","label":"A","color":"#2563EB"}}],
  "nodes":[{{"id":1,"type":"rowBlock","category":"a","rows":[{{"rid":2,"seats":40,"oids":[[100,40]]}}],
     "seatOverrides":{{{}}}}}]}}"##,
        overrides.join(",")
    );
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart {
        ccf: compile(&json),
    });
    e.handle(super::text::atlas());
    resize(&mut e, 3000.0, H, 1.0, 3.0);
    tick(&mut e, 0.0);
    snapshot(&mut e, 1, &[]);
    let x = e.scene.as_ref().unwrap().scene.pos[20];
    look(&mut e, [f64::from(x[0]), f64::from(x[1])], 20.0);
    let events = tick(&mut e, 16.0);
    let asked = requests(&events);
    assert_eq!(asked.len(), REQUESTS_PER_BUILD as usize, "capped");
    assert!(wants_frame(&events), "and asks for the next frame");
    answer(&mut e, &asked);
    let more = rasterise(&mut e, 32.0);
    assert_eq!(asked.len() + more.len(), 40);
    let seats = labels(&e).ranges[1].clone();
    assert_eq!(seats.len(), 40);
    let got = instances(&e, seats);
    assert!(got.iter().all(|i| i.5[0] == 28), "hidden below 7 px");
}

#[test]
fn the_atlas_packs_without_overlap_grows_once_and_clears_only_what_is_unused() {
    let mut backend = Fake::default();
    let mut atlas = TextAtlas::default();
    let mut events = Vec::new();
    let mut rng = Rng(0xa71a5);
    let mut n = 0u32;
    // Builds that use every string so far and add fresh ones, each
    // answered at once, until the atlas has grown and filled.
    let mut sides = Vec::new();
    let bucket = |i: u32| (i % BUCKETS.len() as u32) as u8;
    while n < 1500 {
        atlas.begin();
        for i in 0..n {
            let key = Key {
                source: Source::String(i),
                weight: 400,
                bucket: bucket(i),
            };
            atlas.lookup(key, "", "f", &mut events);
        }
        for _ in 0..REQUESTS_PER_BUILD {
            let key = Key {
                source: Source::String(n),
                weight: 400,
                bucket: bucket(n),
            };
            n += 1;
            atlas.lookup(key, "x", "f", &mut events);
        }
        for event in events.drain(..) {
            let Event::TextRequest { id, .. } = event else {
                continue;
            };
            let (w, h) = (rng.range(1.0, 400.0) as u32, rng.range(1.0, 90.0) as u32);
            atlas
                .supply(&mut backend, id, [w, h, 16], &vec![1; (w * h) as usize])
                .unwrap();
        }
        let (rects, side) = atlas.rects();
        if sides.last() != Some(&side) {
            sides.push(side);
        }
        for (i, a) in rects.iter().enumerate() {
            assert!(u32::from(a[0] + a[2]) <= side && u32::from(a[1] + a[3]) <= side);
            for b in &rects[i + 1..] {
                let apart = a[0] + a[2] <= b[0]
                    || b[0] + b[2] <= a[0]
                    || a[1] + a[3] <= b[1]
                    || b[1] + b[3] <= a[1];
                assert!(apart, "{a:?} {b:?}");
            }
        }
    }
    assert_eq!(
        sides,
        [START_SIDE, MAX_SIDE],
        "made at 1024, grown once, when all in use"
    );
    assert!(atlas.rasterised > 0);
    // Full, with every raster used by this build: a raster that does not
    // fit is dropped and nothing is cleared.
    let (before, _) = atlas.rects();
    atlas.begin();
    let mut used = 0;
    for i in 0..n {
        let key = |b| Key {
            source: Source::String(i),
            weight: 400,
            bucket: b,
        };
        used += u32::from(
            atlas
                .lookup(key((i % BUCKETS.len() as u32) as u8), "", "f", &mut events)
                .is_some(),
        );
    }
    assert!(used > 0);
    // Rasters as large as the limits allow, until one no longer fits.
    let mut kept = before.len();
    for i in 0..REQUESTS_PER_BUILD {
        let key = Key {
            source: Source::Seat(i),
            weight: 400,
            bucket: 0,
        };
        atlas.lookup(key, "big", "f", &mut events);
        let Some(Event::TextRequest { id, .. }) = events.pop() else {
            panic!("asked")
        };
        atlas
            .supply(&mut backend, id, [1000, 500, 16], &vec![1; 500_000])
            .unwrap();
        let now = atlas.rects().0.len();
        assert!(now >= kept, "nothing cleared while every raster is in use");
        if now == kept {
            break;
        }
        kept = now;
    }
    assert_eq!(atlas.rects().0.len(), kept, "the last was dropped");
    // In a later build that uses none of them, a new raster clears the rest.
    atlas.begin();
    let key = Key {
        source: Source::Seat(100),
        weight: 400,
        bucket: 0,
    };
    atlas.lookup(key, "big", "f", &mut events);
    let Some(Event::TextRequest { id, .. }) = events.pop() else {
        panic!("asked")
    };
    atlas
        .supply(&mut backend, id, [1000, 500, 16], &vec![1; 500_000])
        .unwrap();
    assert_eq!(atlas.rects().0.len(), 1);
    // Rasters past the limits are refused.
    atlas.begin();
    let key = Key {
        source: Source::Seat(3),
        weight: 400,
        bucket: 0,
    };
    atlas.lookup(key, "x", "f", &mut events);
    let Some(Event::TextRequest { id, .. }) = events.pop() else {
        panic!("asked")
    };
    assert!(
        atlas
            .supply(&mut backend, id, [2000, 10, 16], &vec![1; 20_000])
            .is_err()
    );
    assert!(MAX_TEXT_INSTANCES >= 2 * REQUESTS_PER_BUILD as usize);
}

/// Native timings for 04 §12: a frame that rebuilds nothing, and a build of
/// the labels in view, on `stadium-100k` at overview and at detail. Run with
/// `cargo test --release -p cavea-render measure_map_labels -- --ignored
/// --nocapture`.
#[test]
#[ignore = "a measurement, not a test"]
fn measure_map_labels() {
    let preset = cavea_testkit::presets::find("stadium-100k").unwrap();
    let ccf = cavea_compiler::compile(&preset.generate(1)).unwrap().public;
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart { ccf });
    e.handle(super::text::atlas());
    resize(&mut e, 1920.0, 1080.0, 1.0, 3.0);
    tick(&mut e, 0.0);
    let fit = cam(&e).centre;
    let [x, y] = e.scene.as_ref().unwrap().scene.chart.sections[0].label_anchor;
    let section = [f64::from(x) / 4.0, f64::from(y) / 4.0];
    for (name, centre, s) in [
        ("overview", fit, 1.2),
        ("detail, 20 px seats", section, 20.0),
    ] {
        look(&mut e, centre, s);
        rasterise(&mut e, 16.0);
        let runs = 200u32;
        let start = std::time::Instant::now();
        for i in 0..runs {
            e.dirty = true;
            tick(&mut e, 500.0 + f64::from(i));
        }
        let frame = start.elapsed().as_secs_f64() * 1e6 / f64::from(runs);
        let start = std::time::Instant::now();
        for i in 0..runs {
            // A theme generation forces a build of the same labels.
            e.theme_gen += 1;
            e.dirty = true;
            tick(&mut e, 1000.0 + f64::from(i));
        }
        let build = start.elapsed().as_secs_f64() * 1000.0 / f64::from(runs);
        let atlas = &e.scene.as_ref().unwrap().text_atlas;
        println!(
            "{name}: a frame in {frame:.1} µs, {} draw calls; a build of {} labels in {build:.3} ms (CPU, test backend); atlas {} KiB, {} strings",
            e.label_stats().frame_draws,
            labels(&e).ranges[2].end,
            atlas.bytes() / 1024,
            atlas.rasterised,
        );
    }
}

/// Asks for `key`'s string in the current build and returns the request id.
fn ask(atlas: &mut TextAtlas, source: Source, text: &str) -> Option<u32> {
    let key = Key {
        source,
        weight: 400,
        bucket: 0,
    };
    let mut events = Vec::new();
    atlas.lookup(key, text, "f", &mut events);
    events.into_iter().find_map(|e| match e {
        Event::TextRequest { id, .. } => Some(id),
        _ => None,
    })
}

#[test]
fn the_atlas_never_stalls_on_pending_entries_empty_answers_or_many_strings() {
    let mut backend = Fake::default();
    // A pending request the next build does not use, and a raster too wide
    // for the first texture: it grows instead of clearing forever.
    let mut atlas = TextAtlas::default();
    atlas.begin();
    ask(&mut atlas, Source::String(0), "a").unwrap();
    atlas.begin();
    let id = ask(&mut atlas, Source::String(1), "b").unwrap();
    atlas
        .supply(&mut backend, id, [1024, 10, 16], &[1; 10_240])
        .unwrap();
    assert_eq!(atlas.rects(), (vec![[0, 0, 1024, 10]], MAX_SIDE));

    // Empty answers settle their requests: the host's "cannot draw".
    let mut atlas = TextAtlas::default();
    for i in 0..200 {
        atlas.begin();
        let id = ask(&mut atlas, Source::String(i), "\u{200d}").unwrap();
        atlas.supply(&mut backend, id, [0, 0, 0], &[]).unwrap();
    }

    // Past the most strings it tracks, the ones not in use make room.
    let mut atlas = TextAtlas::default();
    let mut n = 0;
    while n < crate::text::atlas::MAX_ENTRIES as u32 + 100 {
        atlas.begin();
        for _ in 0..REQUESTS_PER_BUILD {
            let id = ask(&mut atlas, Source::String(n), "x").expect("asked");
            atlas.supply(&mut backend, id, [1, 1, 16], &[1]).unwrap();
            n += 1;
        }
    }
}

#[test]
fn an_answer_for_the_previous_chart_is_never_taken_for_the_new_ones() {
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart { ccf: chart() });
    resize(&mut e, W, H, 1.0, 3.0);
    let old: Vec<u32> = requests(&tick(&mut e, 0.0)).iter().map(|r| r.0).collect();
    assert!(!old.is_empty());
    e.handle(Command::LoadChart { ccf: chart() });
    let new = requests(&tick(&mut e, 16.0));
    assert!(!new.is_empty());
    assert!(new.iter().all(|r| !old.contains(&r.0)), "{old:?} {new:?}");
    // The late answer changes nothing.
    answer(&mut e, &[(old[0], "STAGE".into(), 16)]);
    assert!(
        e.drain_events()
            .all(|ev| !matches!(ev, Event::Error { .. }))
    );
}
