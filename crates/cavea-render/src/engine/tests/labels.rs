//! Dynamic text, section and row labels, text objects, collision, seat and
//! decoration icons (04 §7 passes 8–11, §12), through `Engine::handle` with
//! a fake rasteriser that answers every request with a deterministic raster.

use super::*;
use crate::{
    backend::mode,
    label::{MAX_TEXT_INSTANCES, SECTION_MAX_PX, SECTION_MIN_PX, TEXT_STRIDE},
    protocol::ObjectRef,
    seat,
    text::{
        LABEL_STRIDE,
        atlas::{BUCKETS, Key, MAX_SIDE, REQUESTS_PER_BUILD, START_SIDE, Source, TextAtlas},
    },
};

/// Two floors. Ground: section North (four rows of ten, the first seat
/// accessible), section South (two rows of ten, one labelled `Ä`), the text
/// objects STAGE (centred, always shown) and Left (left-aligned, turned
/// 90°, from zoom 0.5), an exit and a bar icon. Upper: section Upper, a
/// text and a lift icon.
fn chart(display: &str) -> Vec<u8> {
    compile(&format!(
        r##"{{"schema":1,"name":"Labels","settings":{{"seatSize":10,"rowLabelDisplay":"{display}"}},"nextOid":900,
  "categories":[{{"key":"a","label":"A","color":"#2563EB"}}],
  "floors":[{{"id":1,"name":"Ground","level":0}},{{"id":2,"name":"Upper","level":1}}],
  "nodes":[
   {{"id":101,"type":"section","floor":1,"label":"North","category":"a","children":[
     {{"id":102,"type":"rowBlock","rows":[{{"rid":103,"seats":10,"oids":[[10,10]]}},{{"rid":104,"seats":10,"oids":[[20,10]]}},
       {{"rid":105,"seats":10,"oids":[[30,10]]}},{{"rid":106,"seats":10,"oids":[[40,10]]}}],
       "seatOverrides":{{"10":{{"flags":["accessible"]}}}}}}]}},
   {{"id":107,"type":"section","floor":1,"label":"South","category":"a","origin":[0,1000],"children":[
     {{"id":108,"type":"rowBlock","rows":[{{"rid":109,"seats":10,"oids":[[50,10]]}},{{"rid":110,"seats":10,"label":"Ä","oids":[[60,10]]}}]}}]}},
   {{"id":111,"type":"text","floor":1,"origin":[250,-400],"text":"STAGE","size":40,"color":"#112233","align":"center"}},
   {{"id":112,"type":"text","floor":1,"origin":[-700,100],"rotation":90,"text":"Left","size":20,"color":"#445566","align":"left","minZoom":0.5}},
   {{"id":113,"type":"icon","floor":1,"origin":[700,0],"icon":"exit","size":30}},
   {{"id":114,"type":"icon","floor":1,"origin":[700,200],"icon":"bar","size":30}},
   {{"id":115,"type":"section","floor":2,"label":"Upper","category":"a","origin":[0,5000],"children":[
     {{"id":116,"type":"rowBlock","rows":[{{"rid":117,"seats":2,"oids":[[70,2]]}}]}}]}},
   {{"id":118,"type":"text","floor":2,"origin":[0,5500],"text":"Hidden","size":40,"color":"#000000"}},
   {{"id":119,"type":"icon","floor":2,"origin":[0,5600],"icon":"lift","size":30}}]}}"##
    ))
}

/// The chart with the glyph atlas after its first frames, at the fit zoom,
/// with the text they asked for.
fn labelled(display: &str) -> E {
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart {
        ccf: chart(display),
    });
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
    e.handle(Command::LoadChart { ccf: chart("both") });
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
            (Pipeline::Text, vec![0..2], 1.0),
            (Pipeline::Text, vec![2..3], 1.0)
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
    let mut e = labelled("both");
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
                drawn.contains(&(Pipeline::Text, vec![sections.clone()], alpha)),
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
    // A new message replaces it.
    e.handle(Command::SetMessage {
        key: "soldOut".into(),
        text: "Épuisé".into(),
    });
    assert!(rasterise(&mut e, 700.0).iter().any(|a| a.1 == "Épuisé"));
}

#[test]
fn row_labels_show_from_8_px_seats_at_the_ends_the_chart_asks_for() {
    for (display, ends) in [("both", 2), ("start", 1), ("end", 1), ("none", 0)] {
        let mut e = labelled(display);
        let centre = centre_of(&e, 0);
        look(&mut e, centre, 7.5);
        rasterise(&mut e, 16.0);
        assert_eq!(labels(&e).rows, 0, "{display}: none below 8 px");
        look(&mut e, centre, 8.0);
        let asked = rasterise(&mut e, 100.0);
        // Rows A to D of North (and of South, if in view) come from the
        // baked atlas; South's `Ä` is asked of the host.
        let rows = labels(&e).rows;
        assert!(
            rows >= 4 * ends && rows % ends.max(1) == 0,
            "{display}: {rows}"
        );
        let asked_a = asked.iter().any(|a| a.1 == "Ä");
        assert_eq!(asked_a, ends > 0 && e.visible.len() > 1, "{display}");
        if ends > 0 {
            let draw = e
                .backend
                .text_draws
                .iter()
                .rev()
                .find(|d| d.0 == Pipeline::Glyph);
            assert_eq!(draw.map(|d| d.2.clone()), Some(vec![0..rows]));
            // Each instance sits at a compiled row end.
            let chart = &e.scene.as_ref().unwrap().scene.chart;
            let buffer = &e.backend.buffers[&labels(&e).row_buffers().unwrap()[0].0];
            let first = f32::from_le_bytes(buffer[0..4].try_into().unwrap());
            let row = &chart.rows[0];
            let at = if display == "end" {
                row.label_end
            } else {
                row.label_start
            };
            assert_eq!(first, at[0] as f32 * 0.25);
        }
    }
}

#[test]
fn text_objects_follow_floor_angle_alignment_and_minimum_zoom() {
    let mut e = labelled("none");
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
fn collision_keeps_the_priority_order_no_overlap_and_ignores_small_pans() {
    let mut e = labelled("both");
    let centre = centre_of(&e, 0);
    look(&mut e, centre, 9.0);
    rasterise(&mut e, 16.0);
    let row = e.scene.as_ref().unwrap().scene.chart.rows[0];
    let rows = labels(&e).rows;
    assert!(rows > 0);
    // No two placed row labels overlap (their boxes, from the instances).
    let buffer = &e.backend.buffers[&labels(&e).row_buffers().unwrap()[0].0];
    let boxes: Vec<[f32; 4]> = (0..rows as usize)
        .map(|i| {
            let b = &buffer[i * LABEL_STRIDE..][..LABEL_STRIDE];
            let f = |at: usize| f32::from_le_bytes(b[at..at + 4].try_into().unwrap());
            let n = b[12..16].iter().filter(|&&c| c != 0).count() as f32;
            let (half_w, half_h) = (0.3 * n * f(8), 0.36 * f(8));
            [f(0) - half_w, f(4) - half_h, f(0) + half_w, f(4) + half_h]
        })
        .collect();
    for (i, a) in boxes.iter().enumerate() {
        for b in &boxes[i + 1..] {
            let overlap = a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3];
            assert!(!overlap, "{a:?} {b:?}");
        }
    }
    // A one-pixel pan, and a zoom inside the √2 step, build nothing.
    let builds = labels(&e).builds;
    let writes = e.backend.buffer_writes.len();
    for (dx, s) in [(0.1, 9.0), (-0.1, 9.0), (0.0, 9.5)] {
        look(&mut e, [centre[0] + dx, centre[1]], s);
        tick(&mut e, 100.0);
    }
    assert_eq!(labels(&e).builds, builds);
    assert!(
        e.backend.buffer_writes[writes..]
            .iter()
            .all(|w| Some(BufferId(w.0)) != labels(&e).text_buffer())
    );

    // A text object over row A's start hides that row label: text objects
    // come before row labels.
    let [x, y] = row.label_start.map(|v| v as f32 * 0.25);
    let json = format!(
        r##"{{"schema":1,"name":"T","settings":{{"seatSize":10,"rowLabelDisplay":"start"}},"nextOid":900,
  "categories":[{{"key":"a","label":"A","color":"#2563EB"}}],
  "nodes":[{{"id":101,"type":"section","label":"North","category":"a","children":[
     {{"id":102,"type":"rowBlock","rows":[{{"rid":103,"seats":10,"oids":[[10,10]]}},{{"rid":104,"seats":10,"oids":[[20,10]]}}]}}]}},
   {{"id":111,"type":"text","origin":[{x},{y}],"text":"Over","size":10,"color":"#112233"}}]}}"##
    );
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart {
        ccf: compile(&json),
    });
    e.handle(super::text::atlas());
    resize(&mut e, W, H, 1.0, 3.0);
    tick(&mut e, 0.0);
    look(&mut e, [f64::from(x), f64::from(y)], 9.0);
    rasterise(&mut e, 16.0);
    assert_eq!(labels(&e).ranges[2].len(), 1, "the text object");
    assert_eq!(labels(&e).rows, 1, "row B only");
}

#[test]
fn seat_icons_follow_state_and_flags_and_nothing_is_uploaded_for_them() {
    // The shader's table: by visual state, then accessible.
    let shader = include_str!("../../../shaders/shape.wgsl");
    let table = shader.split("SEAT_ICONS = array<u32, 8>(").nth(1).unwrap();
    let icons: Vec<u32> = table
        .split(')')
        .next()
        .unwrap()
        .split(',')
        .map(|v| v.trim().trim_end_matches('u').parse().unwrap())
        .collect();
    let (none, tick_, cross, access) = (0, 1, 2, 3);
    let expect = [
        (visual::AVAILABLE, false, none),
        (visual::AVAILABLE, true, access),
        (visual::UNAVAILABLE, false, cross),
        (visual::UNAVAILABLE, true, cross),
        (visual::SELECTED_PENDING, true, tick_),
        (visual::SELECTED_HELD, false, tick_),
        (visual::SELECTED_HELD, true, tick_),
    ];
    for (v, accessible, icon) in expect {
        assert_eq!(
            icons[usize::from(v) * 2 + usize::from(accessible)],
            icon,
            "{v} {accessible}"
        );
    }

    let mut e = labelled("none");
    snapshot(&mut e, 1, &[]);
    // Seat 0 is accessible: its static shape carries the flag, and its
    // label is left out for its mark.
    let loaded = e.scene.as_ref().unwrap();
    let statics = loaded.scene.encode_static();
    assert_eq!(
        statics[11] & seat::shape::ACCESSIBLE,
        seat::shape::ACCESSIBLE
    );
    assert_eq!(statics[12 + 11] & seat::shape::ACCESSIBLE, 0);
    view_section(&mut e, 0, 20.0);
    let from = e.backend.modes.len();
    tick(&mut e, 16.0);
    // The seats' draw, before the decoration icons'.
    assert_eq!(
        e.backend.modes[from..],
        [mode::SEAT_ICONS, mode::DECORATION_ICONS]
    );
    let slot = e.scene.as_ref().unwrap().seat_labels.slot(0).unwrap();
    let labels_buf = e.scene.as_ref().unwrap().seat_labels.buffer().unwrap().0;
    let at = usize::from(slot) * 40 * LABEL_STRIDE;
    let first = &e.backend.buffers[&labels_buf][at..at + LABEL_STRIDE];
    assert_eq!(
        first, &[0; LABEL_STRIDE],
        "no number under the accessible mark"
    );

    // A hold, a delta and a theme change redraw the seats with their icons
    // and upload nothing but seat state and the palette.
    let writes = e.backend.buffer_writes.len();
    e.handle(Command::Select {
        objects: vec![ObjectRef::Idx(3)],
    });
    tick(&mut e, 32.0);
    delta(&mut e, 1, &[(5, 1)]);
    tick(&mut e, 48.0);
    e.handle(Command::SetTheme {
        base: crate::protocol::BuiltinTheme::HighContrast,
        tokens: Vec::new(),
    });
    tick(&mut e, 64.0);
    // (The mesh's vertices carry the section tints, 04 §10.)
    let loaded = e.scene.as_ref().unwrap();
    let state = [loaded.dynamic_buf.0, loaded.mesh.unwrap()[0].0];
    let written: Vec<u32> = e.backend.buffer_writes[writes..]
        .iter()
        .map(|w| w.0)
        .collect();
    assert!(written.iter().all(|b| state.contains(b)), "{written:?}");
    // Below the detail level, seats are drawn without icons.
    view_section(&mut e, 0, 10.0);
    let from = e.backend.modes.len();
    tick(&mut e, 80.0);
    assert_eq!(e.backend.modes[from], mode::PLAIN);
}

#[test]
fn decoration_icons_are_the_active_floors_drawn_every_frame() {
    let mut e = labelled("none");
    let loaded = e.scene.as_ref().unwrap();
    let ml = &loaded.map_labels;
    assert_eq!(ml.icons, 2, "exit and bar; not the upper floor's lift");
    let [statics, _] = ml.icon_bufs.unwrap();
    let bytes = &e.backend.buffers[&statics.0];
    // Shapes are the pictogram after the seat icons: exit 1, bar 4.
    assert_eq!((bytes[11], bytes[12 + 11]), (1, 4));
    assert_eq!(f32::from_le_bytes(bytes[0..4].try_into().unwrap()), 700.0);
    for s in [1.0, 20.0] {
        view_section(&mut e, 0, s);
        let from = e.backend.calls.len();
        tick(&mut e, s);
        let last = e.backend.calls[from..].last().unwrap();
        assert_eq!((last.0, last.1.clone()), (Pipeline::Shape, vec![0..2]));
        assert_eq!(e.backend.modes.last(), Some(&mode::DECORATION_ICONS));
    }
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
    // A selected seat shows its tick instead.
    e.handle(Command::Select {
        objects: vec![ObjectRef::Idx(20)],
    });
    tick(&mut e, 200.0);
    assert_eq!(labels(&e).ranges[1].len(), 39);
}

#[test]
fn the_atlas_packs_without_overlap_grows_once_and_clears_only_what_is_unused() {
    let mut backend = Fake::default();
    let mut atlas = TextAtlas::default();
    let mut events = Vec::new();
    let mut rng = Rng(0xa71a5);
    let mut n = 0u32;
    // Builds of fresh strings, each answered at once, until the atlas has
    // grown and filled.
    let mut sides = Vec::new();
    while n < 4000 {
        atlas.begin();
        for _ in 0..REQUESTS_PER_BUILD {
            let key = Key {
                source: Source::String(n),
                weight: 400,
                bucket: (n % BUCKETS.len() as u32) as u8,
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
    assert_eq!(sides, [START_SIDE, MAX_SIDE], "made at 1024, grown once");
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

/// Native timings for 04 §12: building the labels of `stadium-100k` at
/// overview and at 8 px seats, and the collision pass. Run with `cargo test
/// --release -p cavea-render measure_map_labels -- --ignored --nocapture`.
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
    let centre = cam(&e).centre;
    for (name, s) in [("overview", 1.2), ("8 px seats", 8.0)] {
        look(&mut e, centre, s);
        rasterise(&mut e, 16.0);
        let runs = 200u32;
        let start = std::time::Instant::now();
        for i in 0..runs {
            // A theme generation forces a build with the same candidates.
            e.theme_gen += 1;
            e.dirty = true;
            tick(&mut e, 1000.0 + f64::from(i));
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(runs);
        let l = labels(&e);
        println!(
            "{name}: build in {ms:.3} ms per frame ({} candidates placed: {} text, {} rows; atlas {} KiB, {} strings)",
            l.ranges[2].end + l.rows,
            l.ranges[2].end,
            l.rows,
            e.scene.as_ref().unwrap().text_atlas.bytes() / 1024,
            e.scene.as_ref().unwrap().text_atlas.rasterised,
        );
    }
}
