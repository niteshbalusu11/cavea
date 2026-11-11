//! Background images (04 §7 pass 2) on the recording backend: asked for
//! only after a frame, drawn first, on the floor shown, as `IMGS` places
//! them; refused over the limits; a failure, a stale answer, a lost context
//! and a hostile `IMGS` leave the map drawing.

use cavea_format::{Artefact, decode, encode};

use super::*;
use crate::image::{BUDGET_PX, MAX_IMAGES, STRIDE};

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const C: &str = "cccccccccccccccccccccccccccccccc";
const D: &str = "dddddddddddddddddddddddddddddddd";

/// Seats on the ground floor; background images A (turned a quarter, half
/// opaque) and B on it, C upstairs, and a reference image D, which is never
/// compiled.
fn chart() -> Vec<u8> {
    compile(&format!(
        r##"{{"schema":1,"name":"t","settings":{{"seatSize":10}},"nextOid":100,
  "categories":[{{"key":"a","label":"A","color":"#ff0000"}}],
  "floors":[{{"id":1,"name":"Ground","level":0}},{{"id":2,"name":"Upper","level":1}}],
  "nodes":[
    {{"id":1,"type":"image","floor":1,"asset":"{A}","size":[400,200],"origin":[100,50],"rotation":90,"opacity":0.5,"role":"background"}},
    {{"id":2,"type":"rowBlock","floor":1,"category":"a","rows":[{{"rid":3,"seats":3,"label":"X","oids":[[10,3]]}}]}},
    {{"id":4,"type":"image","floor":1,"asset":"{B}","size":[100,100],"role":"background"}},
    {{"id":5,"type":"image","floor":2,"asset":"{C}","size":[100,100],"role":"background"}},
    {{"id":6,"type":"image","floor":1,"asset":"{D}","size":[100,100],"role":"reference"}},
    {{"id":7,"type":"rowBlock","floor":2,"category":"a","rows":[{{"rid":8,"seats":1,"label":"Y","oids":[[20,1]]}}]}}]}}"##
    ))
}

/// The image requests among `events`: id, asset, limits.
fn requests(events: &[Event]) -> Vec<(u32, String, u32, u32)> {
    let requests = events.iter().filter_map(|e| match e {
        Event::ImageRequest {
            id,
            asset,
            max_side,
            max_pixels,
        } => Some((*id, asset.clone(), *max_side, *max_pixels)),
        _ => None,
    });
    requests.collect()
}

fn id_of(requests: &[(u32, String, u32, u32)], asset: &str) -> u32 {
    requests.iter().find(|r| r.1 == asset).unwrap().0
}

fn errors(e: &mut E) -> Vec<Event> {
    let all = e.drain_events();
    all.filter(|e| matches!(e, Event::Error { .. })).collect()
}

/// The image draws of the last frame: per draw, its instance range and the
/// byte its texture holds.
fn image_draws(e: &E, from: usize) -> Vec<Range<u32>> {
    let calls = e.backend.calls[from..].iter();
    let images = calls.filter(|c| c.0 == Pipeline::Image);
    images.flat_map(|c| c.1.clone()).collect()
}

/// Loads `ccf` in a fresh engine and draws its first frame.
fn first_frame(ccf: Vec<u8>) -> (E, Vec<Event>) {
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart { ccf });
    resize(&mut e, W, H, 1.0, 3.0);
    let loading: Vec<Event> = e.drain_events().collect();
    assert!(
        requests(&loading).is_empty(),
        "nothing is asked before a frame"
    );
    let events = tick(&mut e, 0.0);
    (e, events)
}

#[test]
fn images_are_asked_for_after_the_first_frame_and_drawn_first_as_placed() {
    let (mut e, events) = first_frame(chart());
    // The first frame drew the map without them; then the floor's two
    // images are asked for, the budget shared, in `IMGS` order.
    assert!(e.backend.calls.iter().all(|c| c.0 != Pipeline::Image));
    assert!(!e.backend.calls.is_empty());
    let asked = requests(&events);
    let half = u32::try_from(BUDGET_PX / 2).unwrap();
    let assets: Vec<_> = asked.iter().map(|r| (r.1.as_str(), r.2, r.3)).collect();
    assert_eq!(assets, [(A, 4096, half), (B, 4096, half)]);
    // Asked once: later frames ask for nothing more.
    e.dirty = true;
    assert!(requests(&tick(&mut e, 16.0)).is_empty());

    e.set_image(id_of(&asked, B), Some(&([64, 64], 2)));
    e.set_image(id_of(&asked, A), Some(&([128, 64], 1)));
    assert!(errors(&mut e).is_empty());
    assert_eq!(e.image_bytes(), (64 * 64 + 128 * 64) * 4);
    let from = e.backend.calls.len();
    tick(&mut e, 32.0);
    // Pass 2: before every other pass, one draw per image, in `IMGS` order.
    let frame = &e.backend.calls[from..];
    assert_eq!(frame[0].0, Pipeline::Image);
    assert_eq!(frame[1].0, Pipeline::Image);
    assert!(frame[2..].iter().all(|c| c.0 != Pipeline::Image));
    assert_eq!(image_draws(&e, from), [0..1, 1..2]);
    // The instances: A centred on its origin, its size halved, a quarter
    // turn, half opaque; B at the origin, opaque.
    let draw = e
        .backend
        .text_draws
        .iter()
        .rfind(|d| d.0 == Pipeline::Image);
    let bytes = &e.backend.buffers[&draw.unwrap().1];
    assert_eq!(bytes.len(), 2 * STRIDE);
    let floats: Vec<f32> = bytes
        .chunks(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    let opacity = f32::from(128u8) / 255.0;
    assert_eq!(
        floats,
        [
            100.0, 50.0, 200.0, 100.0, 16384.0, opacity, 0.0, 0.0, 50.0, 50.0, 0.0, 1.0
        ]
    );
}

#[test]
fn a_failed_slow_or_stale_image_leaves_the_map_drawing() {
    let (mut e, events) = first_frame(chart());
    let asked = requests(&events);
    // A failure: not drawn, not asked for again on this chart.
    e.set_image(id_of(&asked, A), None);
    assert!(errors(&mut e).is_empty());
    e.dirty = true;
    let from = e.backend.calls.len();
    let events = tick(&mut e, 16.0);
    assert!(requests(&events).is_empty());
    assert!(image_draws(&e, from).is_empty());
    assert!(
        e.backend.calls[from..]
            .iter()
            .any(|c| c.0 == Pipeline::Shape)
    );
    // B never answers: the map keeps drawing, input still works.
    e.handle(Command::Select {
        objects: vec![ObjectRef::Idx(0)],
    });
    tick(&mut e, 32.0);
    assert!(errors(&mut e).is_empty());
    // An answer to no open request (a made-up id, or one already answered)
    // is ignored, its image not uploaded.
    let textures = e.backend.textures.len();
    e.set_image(9999, Some(&([8, 8], 9)));
    e.set_image(id_of(&asked, A), Some(&([8, 8], 9)));
    assert_eq!(e.backend.textures.len(), textures);
    // Loading the chart again asks for the failed image again, and for B
    // with a new id: B's late answer to the old one is then stale.
    load_again(&mut e);
    let again = requests(&tick(&mut e, 48.0));
    let assets: Vec<_> = again.iter().map(|r| r.1.as_str()).collect();
    assert_eq!(assets, [A]);
    e.set_image(id_of(&asked, B), Some(&([8, 8], 9)));
    assert_eq!(e.image_bytes(), 8 * 8 * 4, "B's request was still open");
}

fn load_again(e: &mut E) {
    e.handle(Command::LoadChart { ccf: chart() });
    e.drain_events().for_each(drop);
}

#[test]
fn images_over_the_limits_are_refused() {
    let (mut e, events) = first_frame(chart());
    let asked = requests(&events);
    let (a, b) = (id_of(&asked, A), id_of(&asked, B));
    // Longer than the side allowed, then more pixels than its share.
    e.set_image(a, Some(&([4097, 1], 1)));
    assert!(matches!(
        errors(&mut e).as_slice(),
        [Event::Error {
            code: ErrorCode::InvalidInput,
            ..
        }]
    ));
    e.set_image(b, Some(&([4096, 1025], 2)));
    assert_eq!(errors(&mut e).len(), 1);
    assert_eq!(e.image_bytes(), 0);
    assert!(
        e.backend
            .textures
            .values()
            .all(|t| t.1 != [1] && t.1 != [2])
    );
    // Empty images too.
    let (mut e, events) = first_frame(chart());
    e.set_image(id_of(&requests(&events), A), Some(&([0, 10], 1)));
    assert_eq!(errors(&mut e).len(), 1);
}

#[test]
fn a_lost_context_asks_for_images_again_and_destroy_frees_them() {
    let (mut e, events) = first_frame(chart());
    let asked = requests(&events);
    e.set_image(id_of(&asked, A), Some(&([16, 16], 1)));
    tick(&mut e, 16.0);
    e.handle(Command::ContextLost);
    assert!(e.backend.textures.is_empty());
    assert_eq!(e.image_bytes(), 0);
    // B arrives while lost: nothing touches the GPU; it is asked for again.
    let made = e.backend.next;
    e.set_image(id_of(&asked, B), Some(&([16, 16], 2)));
    assert_eq!(e.backend.next, made);
    assert!(
        requests(&tick(&mut e, 32.0)).is_empty(),
        "no frame while lost"
    );
    e.handle(Command::ContextRestored);
    let again = requests(&tick(&mut e, 48.0));
    let assets: Vec<_> = again.iter().map(|r| r.1.as_str()).collect();
    assert_eq!(assets, [A, B]);
    for (id, asset, ..) in &again {
        let fill = if asset == A { 1 } else { 2 };
        e.set_image(*id, Some(&([16, 16], fill)));
    }
    let from = e.backend.calls.len();
    tick(&mut e, 64.0);
    assert_eq!(image_draws(&e, from), [0..1, 1..2]);
    // What dropping the engine (`destroy`) runs for images frees their
    // textures and instance buffer; a load in flight is never answered.
    let images: Vec<u32> = (e.backend.textures.iter())
        .filter(|t| t.1.1 == [1] || t.1.1 == [2])
        .map(|t| *t.0)
        .collect();
    assert_eq!(images.len(), 2);
    let buffers = e.backend.buffers.len();
    e.images.release(&mut e.backend);
    assert!(images.iter().all(|t| !e.backend.textures.contains_key(t)));
    assert_eq!(e.backend.buffers.len(), buffers - 1);
}

#[test]
fn only_the_floor_shown_is_drawn_and_images_no_longer_shown_are_freed() {
    let (mut e, events) = first_frame(chart());
    let asked = requests(&events);
    assert!(asked.iter().all(|r| r.1 != C && r.1 != D));
    e.set_image(id_of(&asked, A), Some(&([16, 16], 1)));
    // Another chart with only B: A's texture is freed, B still asked for
    // under its first id.
    let only_b = compile(&format!(
        r##"{{"schema":1,"name":"t","settings":{{"seatSize":10}},"nextOid":100,
  "categories":[{{"key":"a","label":"A","color":"#ff0000"}}],
  "nodes":[{{"id":1,"type":"image","asset":"{B}","size":[10,10],"role":"background"}},
    {{"id":2,"type":"rowBlock","category":"a","rows":[{{"rid":3,"seats":1,"oids":[[10,1]]}}]}}]}}"##
    ));
    e.handle(Command::LoadChart { ccf: only_b });
    assert!(e.backend.textures.values().all(|t| t.1 != [1]));
    assert_eq!(e.image_bytes(), 0);
    e.set_image(id_of(&asked, B), Some(&([16, 16], 2)));
    assert_eq!(e.image_bytes(), 16 * 16 * 4);
    assert!(requests(&tick(&mut e, 16.0)).is_empty());
}

#[test]
fn hostile_imgs_values_neither_panic_nor_ask_without_bound() {
    let mut chart = decode(&chart(), Artefact::Public).unwrap();
    let template = chart.images[0];
    let bad = chart.strings.push("../../../etc/passwd").unwrap();
    let upper = chart.strings.push(&A.to_uppercase()).unwrap();
    let mut images = vec![
        cavea_format::Image {
            asset: bad,
            ..template
        },
        cavea_format::Image {
            asset: upper,
            ..template
        },
        // Empty, inverted and extreme boxes; no opacity; any angle.
        cavea_format::Image {
            bounds: [0, 0, 0, 10],
            ..template
        },
        cavea_format::Image {
            bounds: [10, 10, -10, -10],
            ..template
        },
        cavea_format::Image {
            opacity: 0,
            ..template
        },
        cavea_format::Image {
            bounds: [i32::MIN, i32::MIN, i32::MAX, i32::MAX],
            angle: u16::MAX,
            ..template
        },
    ];
    // Then far more images than are ever considered, each its own asset.
    for i in 0..10_000u32 {
        let asset = chart.strings.push(&format!("{i:032x}")).unwrap();
        images.push(cavea_format::Image { asset, ..template });
    }
    chart.images = images;
    let ccf = encode(&chart, Artefact::Public).unwrap();
    let (mut e, events) = first_frame(ccf);
    assert!(errors(&mut e).is_empty());
    let asked = requests(&events);
    // Of the first sixteen, the extreme box (template's asset A) and the
    // ten numbered assets that follow; the others are skipped.
    assert!(asked.len() < MAX_IMAGES);
    let assets: Vec<_> = asked.iter().map(|r| r.1.clone()).collect();
    let mut expected = vec![A.to_owned()];
    expected.extend((0..10u32).map(|i| format!("{i:032x}")));
    assert_eq!(assets, expected);
    for (id, ..) in &asked {
        e.set_image(*id, Some(&([4, 4], 1)));
    }
    let from = e.backend.calls.len();
    tick(&mut e, 16.0);
    assert_eq!(image_draws(&e, from).len(), asked.len());
    assert!(errors(&mut e).is_empty());
}
