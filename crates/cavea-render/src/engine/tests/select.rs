//! Hit testing, zoom-to-select, hover and focus rings, and the selection
//! state machine (04 §15, §16), driven through `Engine::handle`.

use cavea_compiler::document::{BlockRow, Node, RowBlock};

use super::*;
use crate::{
    hit::{distance2, hit_test},
    protocol::ObjectRef,
    theme::visual,
};

/// Section `S`: rows `A` and `B` of ten seats of size 40 (pitch 55, 95).
/// Seat `idx` 0 is `S-A-1`, 10 is `S-B-1`.
const HALL: &str = r##"{"schema":1,"name":"Hall","settings":{"seatSize":40},"nextOid":100,
  "categories":[{"key":"a","label":"Stalls","color":"#2563EB"}],
  "nodes":[{"id":1,"type":"section","label":"S","category":"a","children":[
    {"id":2,"type":"rowBlock","rows":[{"rid":3,"seats":10,"oids":[[10,10]]},{"rid":4,"seats":10,"oids":[[30,10]]}]}]}]}"##;

fn hall() -> E {
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart { ccf: compile(HALL) });
    resize(&mut e, W, H, 1.0, 3.0);
    tick(&mut e, 0.0);
    e
}

fn seat_pos(e: &E, idx: u32) -> [f64; 2] {
    let [x, y] = e.scene.as_ref().unwrap().scene.pos[idx as usize];
    cam(e).to_screen([f64::from(x), f64::from(y)], SIZE)
}

fn click(e: &mut E, ty: PointerType, [x, y]: [f64; 2], t: f64) -> Vec<Event> {
    let at = [x as f32, y as f32];
    ptr(e, Down, 1, ty, at, t);
    ptr(e, Up, 1, ty, at, t + 10.0);
    tick(e, t + 10.0)
}

fn visuals(e: &E) -> String {
    let scene = &e.scene.as_ref().unwrap().scene;
    (scene.visual.iter().take(10))
        .map(|&v| match v {
            visual::AVAILABLE => '.',
            visual::UNAVAILABLE => 'u',
            visual::SELECTED_HELD => 'h',
            _ => '?',
        })
        .collect()
}

/// The selection events, one line each.
fn summary(events: &[Event]) -> Vec<String> {
    let line = |kind: &str, labels: &mut dyn Iterator<Item = &str>| {
        labels.fold(kind.to_owned(), |l, label| format!("{l} {label}"))
    };
    events
        .iter()
        .filter_map(|event| {
            Some(match event {
                Event::SelectionChanged { selected } => {
                    line("changed", &mut selected.iter().map(|o| o.label.as_str()))
                }
                Event::SelectionInvalid { rule, objects } => line(
                    &format!("invalid {rule}"),
                    &mut objects.iter().map(String::as_str),
                ),
                Event::SelectionValid => "valid".to_owned(),
                Event::ObjectUnavailable { label } => format!("unavailable {label}"),
                Event::Error { code, .. } => format!("error {code:?}"),
                _ => return None,
            })
        })
        .collect()
}

/// Runs one scripted scenario: a step per line, `step => events`, the
/// events as `summary` writes them, separated by `;`. Steps: `tap idx`,
/// `status idx code`, `rules max min` (`-` for none), `select labels`,
/// `select# idx…`, `deselect labels`, `clear`, `reload`, and `visual` with
/// the visual states of seats 0 to 9 (`.` available, `u` unavailable, `h`
/// selected).
fn run(script: &str) {
    let mut e = hall();
    let mut t = 1000.0;
    for line in script.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (step, expected) = line.split_once("=>").unwrap_or((line, ""));
        let mut words = step.split_whitespace();
        let verb = words.next().unwrap();
        let args: Vec<&str> = words.collect();
        let num = |i: usize| args[i].parse::<u32>().unwrap();
        let opt = |i: usize| (args[i] != "-").then(|| num(i));
        let labels = || {
            args.iter()
                .map(|l| ObjectRef::Label((*l).to_owned()))
                .collect()
        };
        e.drain_events().for_each(drop);
        let events: Vec<Event> = match verb {
            "tap" => {
                t += 1000.0;
                let at = seat_pos(&e, num(0));
                click(&mut e, MOUSE, at, t)
            }
            "visual" => {
                assert_eq!(visuals(&e), args[0], "{line}");
                continue;
            }
            _ => {
                match verb {
                    "status" => e.apply_status(num(0)..num(0) + 1, num(1) as u8),
                    "rules" => e.handle(Command::SetRules {
                        max: opt(0),
                        min: opt(1),
                        validators: Vec::new(),
                        group_size: None,
                    }),
                    "select" => e.handle(Command::Select { objects: labels() }),
                    "select#" => {
                        let objects = (0..args.len()).map(|i| ObjectRef::Idx(num(i))).collect();
                        e.handle(Command::Select { objects });
                    }
                    "deselect" => e.handle(Command::Deselect { objects: labels() }),
                    "clear" => e.handle(Command::ClearSelection),
                    "reload" => e.handle(Command::LoadChart { ccf: compile(HALL) }),
                    _ => panic!("unknown step {line}"),
                }
                let events = e.drain_events().collect();
                tick(&mut e, t);
                events
            }
        };
        let expected: Vec<&str> = expected
            .split(';')
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(summary(&events), expected, "{line}");
    }
}

#[test]
fn selection_state_machine() {
    // Select and deselect.
    run("
        tap 0 => changed S-A-1
        visual h.........
        tap 0 => changed
        visual ..........
    ");
    // The maximum refuses a tap; the minimum reports readiness.
    run("
        rules 2 2 => invalid min
        tap 0 => changed S-A-1
        tap 1 => changed S-A-1 S-A-2 ; valid
        tap 2 => invalid max S-A-3
        rules 1 - => error InvalidInput
        tap 1 => changed S-A-1 ; invalid min S-A-1
        rules 0 - => error InvalidInput
        rules 101 - => error InvalidInput
        rules 1 2 => error InvalidInput
        rules - - => valid
        visual h.........
    ");
    // A selected seat taken by someone else leaves the selection; a taken
    // seat cannot be selected; a seat becoming free again keeps its state.
    run("
        tap 0 => changed S-A-1
        tap 1 => changed S-A-1 S-A-2
        status 0 1 => unavailable S-A-1 ; changed S-A-2
        tap 0 => invalid not_free S-A-1
        status 1 0 =>
        visual uh........
        status 0 0 =>
        tap 0 => changed S-A-2 S-A-1
    ");
    // Host commands, by label or idx, all or nothing.
    run("
        select S-A-1 S-B-2 S-A-1 => changed S-A-1 S-B-2
        select S-A-3 nope => error InvalidInput
        select# 2 9999 => error InvalidInput
        status 2 1 =>
        select S-A-3 S-A-4 => invalid not_free S-A-3
        select# 3 0 => changed S-A-1 S-B-2 S-A-4
        deselect S-A-1 nope => error InvalidInput
        deselect S-A-1 S-A-9 => changed S-B-2 S-A-4
        clear => changed
        clear =>
        visual ..u.......
    ");
    // A new chart forgets the selection.
    run("
        tap 0 => changed S-A-1
        reload => changed
        visual ..........
        tap 1 => changed S-A-2
    ");
}

#[test]
fn hostile_selection_commands_are_bounded() {
    let mut e = hall();
    // Thousands of labels: duplicates collapse, unknown ones are one error.
    let objects = vec![ObjectRef::Label("S-A-1".into()); 5000];
    e.handle(Command::Select { objects });
    let events: Vec<Event> = e.drain_events().collect();
    assert_eq!(summary(&events), ["changed S-A-1"]);
    let objects = (0..5000)
        .map(|i| ObjectRef::Label(format!("x{i}")))
        .collect();
    e.handle(Command::Select { objects });
    let events: Vec<Event> = e.drain_events().collect();
    assert!(
        matches!(events.as_slice(), [Event::Error { message, .. }] if message.starts_with("5000 unknown"))
    );
    // More seats than the maximum allows: refused whole.
    let objects = (0..20).map(ObjectRef::Idx).collect();
    e.handle(Command::SetRules {
        max: Some(5),
        min: None,
        validators: Vec::new(),
        group_size: None,
    });
    e.handle(Command::Select { objects });
    assert!(summary(&e.drain_events().collect::<Vec<_>>())[0].starts_with("invalid max"));
    // Rules this build does not have are refused, not ignored.
    e.handle(Command::SetRules {
        max: None,
        min: None,
        validators: vec!["consecutive".into()],
        group_size: None,
    });
    assert_eq!(
        summary(&e.drain_events().collect::<Vec<_>>()),
        ["error Unsupported"]
    );
}

#[test]
fn a_tap_is_one_frame_and_one_sub_range_upload() {
    let mut e = hall();
    let dynamic = e.scene.as_ref().unwrap().dynamic_buf.0;
    let (writes, draws) = (e.backend.buffer_writes.len(), e.backend.draws.len());
    let at = seat_pos(&e, 5);
    let events = click(&mut e, TOUCH, at, 1000.0);
    // Drawn in the frame of the `Tick` that took the tap.
    assert_eq!(e.backend.draws.len(), draws + 1);
    assert_eq!(e.backend.buffer_writes[writes..], [(dynamic, 5 * 4, 4)]);
    assert_eq!(
        e.scene.as_ref().unwrap().scene.visual[5],
        visual::SELECTED_HELD
    );
    assert_eq!(summary(&events), ["changed S-A-6"]);
    // Nothing more is drawn or asked for.
    assert!(!wants_frame(&tick(&mut e, 1100.0)));
    assert_eq!(e.backend.draws.len(), draws + 1);
}

/// The chart's seat size on screen, in CSS pixels.
fn seat_px(e: &E) -> f64 {
    f64::from(e.scene.as_ref().unwrap().scene.seat_size) * cam(e).zoom
}

/// A 3,000-seat stadium (seats 44 cu), whose fit shows seats of about 3 px.
fn bowl() -> E {
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart { ccf: stadium(3000) });
    resize(&mut e, W, H, 1.0, 3.0);
    tick(&mut e, 0.0);
    e
}

/// Sets the zoom so that seats are `px` on screen, centred on seat 5.
fn zoom_to(e: &mut E, px: f64) {
    let scene = &e.scene.as_ref().unwrap().scene;
    let [x, y] = scene.pos[5];
    let side = (f64::from(W) / (px / f64::from(scene.seat_size))) as f32;
    let height = side * H / W;
    set_camera(e, [x - side / 2.0, y - height / 2.0, side, height]);
    tick(e, 0.0);
    // The fit pads by 24 px on each side.
    let got = seat_px(e);
    assert!((got / px - 1.0).abs() < 0.1, "{got} for {px}");
}

#[test]
fn tolerances_differ_for_touch_and_mouse() {
    // Seats of 20 px (half 10): a touch 11 px from the centre hits (12 px
    // tolerance), a click does not (4 px, raised to the half size, 10).
    for (ty, hits) in [(TOUCH, true), (MOUSE, false)] {
        let mut e = bowl();
        zoom_to(&mut e, 20.0);
        let [x, y] = seat_pos(&e, 5);
        let off = 11.0 * 20.0 / seat_px(&e);
        click(&mut e, ty, [x + off, y], 1000.0);
        let selected = e.scene.as_ref().unwrap().scene.visual[5] == visual::SELECTED_HELD;
        assert_eq!(selected, hits, "{ty:?}");
    }
}

#[test]
fn small_seats_zoom_instead_of_selecting() {
    // Touch below 18 px flies to 32 px centred on the tap, then selects.
    let mut e = bowl();
    zoom_to(&mut e, 12.0);
    let at = seat_pos(&e, 5);
    let world = cam(&e).to_world(at, SIZE);
    assert!(summary(&click(&mut e, TOUCH, at, 1000.0)).is_empty());
    settle(&mut e, 1016.0);
    assert!((seat_px(&e) - 32.0).abs() < 1e-6, "{}", seat_px(&e));
    // The tap position went through f32.
    assert!(close(cam(&e).centre, world, 0.01));
    let at = seat_pos(&e, 5);
    assert!(!summary(&click(&mut e, TOUCH, at, 3000.0)).is_empty());

    // A mouse selects from 5 px; below that a click zooms the same way.
    for (px, selects) in [(6.0, true), (4.0, false)] {
        let mut e = bowl();
        zoom_to(&mut e, px);
        let at = seat_pos(&e, 5);
        let events = summary(&click(&mut e, MOUSE, at, 1000.0));
        assert_eq!(!events.is_empty(), selects, "{px} px");
        settle(&mut e, 1016.0);
        let expected = if selects { px } else { 32.0 };
        assert!((seat_px(&e) / expected - 1.0).abs() < 0.1, "{px} px");
    }
}

/// The one instance range of the last draw call.
fn last_draw(e: &E) -> (u32, u32) {
    match e.backend.draws.last().map(Vec::as_slice) {
        Some([range]) => (range.start, range.end),
        other => panic!("{other:?}"),
    }
}

fn hover(e: &mut E, [x, y]: [f64; 2], t: f64) -> Vec<Event> {
    ptr(e, Move, 1, MOUSE, [x as f32, y as f32], t);
    tick(e, t)
}

fn hovered(events: &[Event]) -> Vec<Option<String>> {
    (events.iter())
        .filter_map(|e| match e {
            Event::ObjectHover(o) | Event::ObjectFocus(o) => {
                Some(o.as_ref().map(|(o, _)| o.label.clone()))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn hover_emits_and_draws_only_on_change() {
    let mut e = hall();
    let draws = e.backend.draws.len();
    let [x, y] = seat_pos(&e, 3);
    let events = hover(&mut e, [x + 2.0, y], 100.0);
    let Some(Event::ObjectHover(Some((object, anchor)))) =
        events.iter().find(|e| matches!(e, Event::ObjectHover(_)))
    else {
        panic!("{events:?}");
    };
    assert_eq!(
        (
            object.label.as_str(),
            object.section.as_str(),
            object.row.as_str()
        ),
        ("S-A-4", "S", "A")
    );
    assert_eq!((object.seat.as_str(), object.category.as_str()), ("4", "a"));
    assert!(close(anchor.map(f64::from), [x, y], 1e-3));
    // The seats and the ring: a halo and a ring, uploaded once.
    assert_eq!(e.backend.draws.len(), draws + 2);
    assert_eq!(last_draw(&e), (0, 2));
    let writes = e.backend.buffer_writes.len();
    // Moving within the seat: no event, no frame.
    let events = hover(&mut e, [x - 2.0, y + 1.0], 200.0);
    assert!(hovered(&events).is_empty() && !wants_frame(&events));
    assert_eq!(e.backend.draws.len(), draws + 2);
    // Focus adds a second ring; the overlay is uploaded again.
    e.set_focus(Some(4));
    assert_eq!(
        hovered(&e.drain_events().collect::<Vec<_>>()),
        [Some("S-A-5".into())]
    );
    tick(&mut e, 300.0);
    assert_eq!(last_draw(&e), (0, 4));
    assert_eq!(e.backend.buffer_writes.len(), writes + 2);
    // Leaving: hover cleared, one ring left.
    ptr(&mut e, Cancel, 1, MOUSE, [0.0, 0.0], 400.0);
    assert_eq!(hovered(&tick(&mut e, 400.0)), [None]);
    assert_eq!(last_draw(&e), (0, 2));
    e.set_focus(None);
    assert_eq!(hovered(&e.drain_events().collect::<Vec<_>>()), [None]);
    tick(&mut e, 500.0);
    // No overlay drawn: the last draw is the seats'.
    assert_eq!(last_draw(&e), (0, 20));

    // A long press reports the object for a tooltip; the next tap ends it.
    let mut e = hall();
    let at = seat_pos(&e, 12).map(|v| v as f32);
    ptr(&mut e, Down, 1, TOUCH, at, 0.0);
    assert_eq!(hovered(&tick(&mut e, 600.0)), [Some("S-B-3".into())]);
    ptr(&mut e, Up, 1, TOUCH, at, 700.0);
    assert!(hovered(&tick(&mut e, 700.0)).is_empty());
    let events = click(&mut e, TOUCH, [5.0, 5.0], 2000.0);
    assert_eq!(hovered(&events), [None]);
}

/// The stadium of the test kit plus a block of rows outside any section.
fn stadium(seats: u32) -> Vec<u8> {
    let mut doc = cavea_testkit::synthetic::stadium(seats, 5);
    let Some(Node::Section(section)) = doc.nodes.first() else {
        panic!("stadium has sections");
    };
    let Some(Node::RowBlock(block)) = section.children.first() else {
        panic!("section has a block");
    };
    let next = doc.next_oid;
    let rows = (0..3)
        .map(|r| BlockRow {
            rid: next + r * 20,
            seats: 12,
            offset: 0.0,
            oids: vec![[next + r * 20 + 1, 12]],
            label: Some(format!("L{r}")),
            removed: Vec::new(),
        })
        .collect();
    let block = RowBlock {
        id: next + 100,
        origin: [0.0, 0.0],
        rotation: 0.0,
        curve: cavea_compiler::document::Curve::None {},
        category: Some("premium".into()),
        rows,
        ..block.clone()
    };
    doc.nodes.push(Node::RowBlock(block));
    doc.next_oid = next + 101;
    cavea_compiler::compile(&doc).unwrap().public
}

#[test]
fn hit_test_finds_the_nearest_seat_within_tolerance() {
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart { ccf: stadium(3000) });
    let scene = &e.scene.as_ref().unwrap().scene;
    assert!(!scene.loose_rows.is_empty());
    let brute = |p: [f32; 2], tolerance: f32| {
        let mut best: Option<(f32, u32)> = None;
        for idx in scene.active.clone() {
            let row = &scene.chart.rows[scene.row[idx as usize] as usize];
            let tol = (f32::from(row.seat_size) * 0.125).max(tolerance);
            let d2 = distance2(scene.pos[idx as usize], p);
            if d2 <= tol * tol && best.is_none_or(|b| (d2, idx) < b) {
                best = Some((d2, idx));
            }
        }
        best.map(|(_, idx)| idx)
    };
    let b = scene.bounds;
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let (mut hits, mut misses) = (0, 0);
    for _ in 0..4000 {
        // Zoom from a whole-chart view to 64 px seats; touch or mouse.
        let zoom = 0.01 * 10f32.powf(rng.range(0.0, 2.2));
        let px = if rng.range(0.0, 1.0) < 0.5 { 12.0 } else { 4.0 };
        let tolerance = px / zoom;
        let p = if rng.range(0.0, 1.0) < 0.7 {
            let idx = rng.range(0.0, f64::from(scene.active.end)) as usize;
            let [x, y] = scene.pos[idx.min(scene.pos.len() - 1)];
            let r = f64::from(tolerance) * 2.0 + 40.0;
            [x + rng.range(-r, r), y + rng.range(-r, r)]
        } else {
            [
                rng.range(f64::from(b.x), f64::from(b.x + b.width)),
                rng.range(f64::from(b.y), f64::from(b.y + b.height)),
            ]
        };
        let got = hit_test(scene, p, tolerance);
        assert_eq!(got, brute(p, tolerance), "{p:?} at tolerance {tolerance}");
        if got.is_some() {
            hits += 1
        } else {
            misses += 1
        }
    }
    assert!(hits > 1000 && misses > 1000, "{hits} hits, {misses} misses");
}

/// Hit-test time at 100,000 seats. Run with `cargo test --release -p
/// cavea-render measure_ -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, not a test"]
fn measure_hit_test() {
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart {
        ccf: cavea_compiler::compile(&cavea_testkit::synthetic::stadium(100_000, 1))
            .unwrap()
            .public,
    });
    let scene = &e.scene.as_ref().unwrap().scene;
    let mut rng = Rng(7);
    let points: Vec<([f32; 2], f32)> = (0..100_000)
        .map(|_| {
            let [x, y] = scene.pos[rng.range(0.0, 99_999.0) as usize];
            // Taps near seats at 18 to 64 px seats: the zooms that select.
            let tolerance = 12.0 / (rng.range(18.0, 64.0) / 44.0);
            (
                [x + rng.range(-30.0, 30.0), y + rng.range(-30.0, 30.0)],
                tolerance,
            )
        })
        .collect();
    let start = std::time::Instant::now();
    let found = points
        .iter()
        .filter(|(p, t)| hit_test(scene, *p, *t).is_some())
        .count();
    let per = start.elapsed() / points.len() as u32;
    println!("hit test at 100,000 seats: {per:?} per test ({found} hits)");
    let start = std::time::Instant::now();
    let index = cavea_format::full_label::LabelIndex::build(&scene.chart);
    println!("label index build: {:?}", start.elapsed());
    drop(index);
}
