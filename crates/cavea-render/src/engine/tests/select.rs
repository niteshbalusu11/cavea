//! Hit testing, zoom-to-select, hover and focus rings, and the selection
//! state machine (04 §15, §16), driven through `Engine::handle`.

use cavea_compiler::document::{BlockRow, Node, RowBlock};

use super::*;
use crate::{
    hit::{distance2, hit_test},
    protocol::{HoldOutcome, ObjectFailure, ObjectRef, SessionMode},
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
    cam(e).to_screen([f64::from(x), f64::from(y)], e.viewport.unwrap().css)
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
            visual::SELECTED_PENDING => 'p',
            visual::SELECTED_HELD => 'h',
            _ => '?',
        })
        .collect()
}

/// The selection events, one line each.
fn summary(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| {
            Some(match event {
                Event::HoldRequest { request, labels } => format!("hold {request} {labels:?}"),
                Event::ReleaseRequest { request, labels } => {
                    format!("release {request} {labels:?}")
                }
                Event::SelectionChanged { selected, .. } => {
                    let labels: Vec<&str> = selected.iter().map(|o| o.label.as_str()).collect();
                    format!("changed {labels:?}")
                }
                Event::SelectionInvalid { rule, objects } => format!("invalid {rule} {objects:?}"),
                Event::SelectionValid => "valid".to_owned(),
                Event::ObjectUnavailable { label } => format!("unavailable {label}"),
                Event::HoldLost { labels } => format!("lost {labels:?}"),
                Event::Resync { seq } => format!("resync {seq:?}"),
                Event::Error { code, .. } => format!("error {code:?}"),
                _ => return None,
            })
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Step {
    Tap(u32),
    Ok(u32),
    /// A failed hold: code, and the seats named (none: the whole request).
    Fail(u32, &'static str, &'static [u32]),
    /// A delta setting one seat's status.
    Status(u32, u8),
    /// A snapshot with these seats unavailable.
    Snapshot(&'static [u32]),
    /// The buyer's private set.
    Private(&'static [u32]),
    Rules(Option<u32>, Option<u32>),
    Manual,
    Select(&'static [&'static str]),
    SelectIdx(&'static [u32]),
    Deselect(&'static [&'static str]),
    Clear,
    Reload,
    /// The visual states of seats 0 to 9.
    Visual(&'static str),
}

fn run(steps: &[(Step, &str)]) {
    use Step::*;
    let mut e = hall();
    let mut t = 1000.0;
    snapshot(&mut e, 0, &[]);
    let mut seq = 0;
    let label = |s: &&str| ObjectRef::Label((*s).to_owned());
    for (i, &(step, expected)) in steps.iter().enumerate() {
        e.drain_events().for_each(drop);
        let events = match step {
            Tap(idx) => {
                t += 1000.0;
                let at = seat_pos(&e, idx);
                click(&mut e, MOUSE, at, t)
            }
            Ok(request) | Fail(request, ..) => {
                let outcome = match step {
                    Fail(_, code, seats) => HoldOutcome::Failed(
                        seats
                            .iter()
                            .map(|&idx| ObjectFailure {
                                object: ObjectRef::Idx(idx),
                                code: code.to_owned(),
                            })
                            .chain(seats.is_empty().then(|| ObjectFailure {
                                object: ObjectRef::Label("elsewhere".into()),
                                code: code.to_owned(),
                            }))
                            .collect(),
                    ),
                    _ => HoldOutcome::Ok,
                };
                e.handle(Command::HoldResult { request, outcome });
                e.drain_events().collect()
            }
            Status(idx, status) => {
                seq += 1;
                delta(&mut e, seq - 1, &[(idx, status)])
            }
            Snapshot(unavailable) => {
                seq += 1;
                snapshot(&mut e, seq, unavailable)
            }
            Private(objects) => {
                let objects = objects.to_vec();
                let expires_at_ms = 9000;
                e.handle(Command::SetPrivateHolds {
                    objects,
                    expires_at_ms,
                });
                e.drain_events().collect()
            }
            Rules(max, min) => {
                e.handle(Command::SetRules {
                    max,
                    min,
                    validators: Vec::new(),
                    group_size: None,
                });
                e.drain_events().collect()
            }
            Manual => {
                e.handle(Command::SetSessionMode(SessionMode::Manual));
                e.drain_events().collect()
            }
            Select(labels) => {
                let objects = labels.iter().map(label).collect();
                e.handle(Command::Select { objects });
                e.drain_events().collect()
            }
            SelectIdx(seats) => {
                let objects = seats.iter().map(|&i| ObjectRef::Idx(i)).collect();
                e.handle(Command::Select { objects });
                e.drain_events().collect()
            }
            Deselect(labels) => {
                let objects = labels.iter().map(label).collect();
                e.handle(Command::Deselect { objects });
                e.drain_events().collect()
            }
            Clear => {
                e.handle(Command::ClearSelection);
                e.drain_events().collect()
            }
            Reload => {
                e.handle(Command::LoadChart { ccf: compile(HALL) });
                let events = e.drain_events().collect();
                tick(&mut e, t);
                events
            }
            Visual(v) => {
                assert_eq!(visuals(&e), v, "step {i}");
                continue;
            }
        };
        assert_eq!(summary(&events).join("; "), expected, "step {i}");
    }
}

#[test]
fn selection_state_machine() {
    use Step::*;
    // Each scenario: steps, each with the selection events it emits ("; "
    // between events).
    let scenarios: &[&[(Step, &str)]] = &[
        // Select, then the hold succeeds; deselecting a held seat releases it.
        &[
            (Tap(0), r#"changed ["S-A-1"]; hold 1 ["S-A-1"]"#),
            (Visual("p........."), ""),
            (Ok(1), ""),
            (Visual("h........."), ""),
            (Tap(0), r#"changed []; release 2 ["S-A-1"]"#),
            (Visual(".........."), ""),
        ],
        // The hold fails: taken by someone else, or refused by a rule. Seats
        // the server does not name are requested again.
        &[
            (Tap(0), r#"changed ["S-A-1"]; hold 1 ["S-A-1"]"#),
            (Fail(1, "not_free", &[0]), "unavailable S-A-1; changed []"),
            (Tap(0), r#"changed ["S-A-1"]; hold 2 ["S-A-1"]"#),
            (Tap(1), r#"changed ["S-A-1", "S-A-2"]"#),
            (Ok(2), r#"hold 3 ["S-A-2"]"#),
            (Tap(2), r#"changed ["S-A-1", "S-A-2", "S-A-3"]"#),
            (
                Fail(3, "orphan_rule_violated", &[1]),
                r#"invalid orphan_rule_violated ["S-A-2"]; changed ["S-A-1", "S-A-3"]; hold 4 ["S-A-3"]"#,
            ),
            (
                Fail(4, "hold_limit_exceeded", &[]),
                r#"invalid hold_limit_exceeded ["S-A-3"]; changed ["S-A-1"]"#,
            ),
            (Visual("h........."), ""),
        ],
        // Deselected while the hold is in flight: released if it succeeded,
        // nothing if it failed.
        &[
            (Tap(0), r#"changed ["S-A-1"]; hold 1 ["S-A-1"]"#),
            (Tap(0), "changed []"),
            (Ok(1), r#"release 2 ["S-A-1"]"#),
            (Tap(1), r#"changed ["S-A-2"]; hold 3 ["S-A-2"]"#),
            (Tap(1), "changed []"),
            (Fail(3, "not_free", &[1]), ""),
            (Visual(".........."), ""),
        ],
        // On, off and on again: before the request went out, no request;
        // while in flight, the result applies.
        &[
            (Tap(0), r#"changed ["S-A-1"]; hold 1 ["S-A-1"]"#),
            (Tap(1), r#"changed ["S-A-1", "S-A-2"]"#),
            (Tap(1), r#"changed ["S-A-1"]"#),
            (Tap(0), "changed []"),
            (Tap(0), r#"changed ["S-A-1"]"#),
            (Ok(1), ""),
            (Visual("h........."), ""),
            // A held seat deselected and reselected while a hold is in
            // flight is never released.
            (Tap(2), r#"changed ["S-A-1", "S-A-3"]; hold 2 ["S-A-3"]"#),
            (Tap(0), r#"changed ["S-A-3"]"#),
            (Tap(0), r#"changed ["S-A-3", "S-A-1"]"#),
            (Ok(2), ""),
            (Visual("h.h......."), ""),
        ],
        // One request in flight; the rest coalesce, releases first.
        &[
            (Tap(0), r#"changed ["S-A-1"]; hold 1 ["S-A-1"]"#),
            (Ok(1), ""),
            (Tap(1), r#"changed ["S-A-1", "S-A-2"]; hold 2 ["S-A-2"]"#),
            (Tap(0), r#"changed ["S-A-2"]"#),
            (Tap(3), r#"changed ["S-A-2", "S-A-4"]"#),
            (Tap(4), r#"changed ["S-A-2", "S-A-4", "S-A-5"]"#),
            (Ok(2), r#"release 3 ["S-A-1"]; hold 4 ["S-A-4", "S-A-5"]"#),
            // Late, repeated and unknown results change nothing.
            (Ok(2), "error InvalidInput"),
            (Ok(99), "error InvalidInput"),
            (Visual(".h.pp....."), ""),
        ],
        // The maximum refuses a tap; the minimum reports readiness.
        &[
            (Rules(Some(2), Some(2)), "invalid min []"),
            (Tap(0), r#"changed ["S-A-1"]; hold 1 ["S-A-1"]"#),
            (Tap(1), r#"changed ["S-A-1", "S-A-2"]; valid"#),
            (Tap(2), r#"invalid max ["S-A-3"]"#),
            (Tap(1), r#"changed ["S-A-1"]; invalid min ["S-A-1"]"#),
            (Rules(Some(0), None), "error InvalidInput"),
            (Rules(Some(101), None), "error InvalidInput"),
            (Rules(Some(1), Some(2)), "error InvalidInput"),
            (Rules(None, None), "valid"),
            (Visual("p........."), ""),
        ],
        // Manual session: no requests; a selected seat taken by someone else
        // leaves the selection; an unavailable seat cannot be selected.
        &[
            (Manual, ""),
            (Tap(0), r#"changed ["S-A-1"]"#),
            (Visual("h........."), ""),
            (Status(0, 1), "unavailable S-A-1; changed []"),
            (Tap(0), r#"invalid not_free ["S-A-1"]"#),
            (Visual("u........."), ""),
            (Tap(1), r#"changed ["S-A-2"]"#),
            (Tap(1), "changed []"),
            // A snapshot knocks out what is not the buyer's.
            (Tap(2), r#"changed ["S-A-3"]"#),
            (Snapshot(&[0, 2]), "unavailable S-A-3; changed []"),
        ],
        // The private set: restores the selection after a reload, survives
        // snapshots, and a held seat that leaves it is lost.
        &[
            (Reload, ""),
            (Snapshot(&[0, 1, 5]), ""),
            (Private(&[0, 1]), r#"changed ["S-A-1", "S-A-2"]"#),
            (Snapshot(&[0, 1, 5]), ""),
            (Visual("hh...u...."), ""),
            (Private(&[1]), r#"lost ["S-A-1"]; changed ["S-A-2"]"#),
            (Visual("uh...u...."), ""),
            // Released: drawn available until a set without it confirms the
            // release; a set still listing it restores nothing.
            (Tap(1), r#"changed []; release 1 ["S-A-2"]"#),
            (Private(&[1]), ""),
            (Visual("u....u...."), ""),
            (Private(&[]), "changed []"),
            (Visual("uu...u...."), ""),
            (Status(1, 0), ""),
            (Visual("u....u...."), ""),
        ],
        // The set confirms a hold in flight; a lost token loses everything.
        &[
            (Tap(0), r#"changed ["S-A-1"]; hold 1 ["S-A-1"]"#),
            (Status(0, 1), ""),
            (Private(&[0]), r#"changed ["S-A-1"]"#),
            (Visual("h........."), ""),
            (Ok(1), ""),
            (Tap(1), r#"changed ["S-A-1", "S-A-2"]; hold 2 ["S-A-2"]"#),
            (Tap(2), r#"changed ["S-A-1", "S-A-2", "S-A-3"]"#),
            (
                Fail(2, "not_found", &[]),
                r#"lost ["S-A-1", "S-A-2", "S-A-3"]; changed []"#,
            ),
            (Visual("u........."), ""),
            (Tap(1), r#"changed ["S-A-2"]; hold 3 ["S-A-2"]"#),
        ],
        // The buyer's own hold turns the public status unavailable; that
        // never knocks out the seat. A seat whose request has not gone out
        // is knocked out.
        &[
            (Tap(0), r#"changed ["S-A-1"]; hold 1 ["S-A-1"]"#),
            (Status(0, 1), ""),
            (Visual("p........."), ""),
            (Tap(1), r#"changed ["S-A-1", "S-A-2"]"#),
            (Status(1, 1), r#"unavailable S-A-2; changed ["S-A-1"]"#),
            (Ok(1), ""),
            (Status(0, 1), ""),
            (Visual("hu........"), ""),
            // Deselected, it is drawn available until the release is
            // confirmed; tapped again before the release goes out, it is
            // still the buyer's.
            (Tap(2), r#"changed ["S-A-1", "S-A-3"]; hold 2 ["S-A-3"]"#),
            (Tap(0), r#"changed ["S-A-3"]"#),
            (Visual(".up......."), ""),
            (Tap(0), r#"changed ["S-A-3", "S-A-1"]"#),
            (Visual("hup......."), ""),
        ],
        // Host commands, by label or idx, all or nothing.
        &[
            (
                Select(&["S-A-1", "S-B-2", "S-A-1"]),
                r#"changed ["S-A-1", "S-B-2"]; hold 1 ["S-A-1", "S-B-2"]"#,
            ),
            (Select(&["S-A-3", "nope"]), "error InvalidInput"),
            (SelectIdx(&[2, 9999]), "error InvalidInput"),
            (Status(2, 1), ""),
            (Select(&["S-A-3", "S-A-4"]), r#"invalid not_free ["S-A-3"]"#),
            (SelectIdx(&[3, 0]), r#"changed ["S-A-1", "S-B-2", "S-A-4"]"#),
            (Deselect(&["S-A-1", "nope"]), "error InvalidInput"),
            (
                Deselect(&["S-A-1", "S-A-9"]),
                r#"changed ["S-B-2", "S-A-4"]"#,
            ),
            (Clear, "changed []"),
            (Ok(1), r#"release 2 ["S-A-1", "S-B-2"]"#),
            (Visual("..u......."), ""),
        ],
        // A new chart forgets the selection without requests; a result for
        // the old chart is unknown.
        &[
            (Tap(0), r#"changed ["S-A-1"]; hold 1 ["S-A-1"]"#),
            (Reload, "changed []"),
            (Ok(1), "error InvalidInput"),
            (Visual(".........."), ""),
            (Tap(1), r#"changed ["S-A-2"]; hold 2 ["S-A-2"]"#),
            (Manual, "error InvalidInput"),
        ],
    ];
    for steps in scenarios {
        run(steps);
    }
}

/// A hold result, the private set confirming it and the delta showing the
/// seat unavailable, in every order: the seat is never drawn unavailable and
/// ends held. Restoring before the chart arrives works the same.
#[test]
fn own_holds_in_any_order() {
    #[derive(Clone, Copy, Debug)]
    enum S {
        Result,
        Set,
        Delta,
    }
    use S::*;
    let orders = [
        [Result, Set, Delta],
        [Result, Delta, Set],
        [Set, Result, Delta],
        [Set, Delta, Result],
        [Delta, Result, Set],
        [Delta, Set, Result],
    ];
    let private = |e: &mut E| {
        e.handle(Command::SetPrivateHolds {
            objects: vec![0],
            expires_at_ms: 1,
        });
    };
    for order in orders {
        let mut e = hall();
        snapshot(&mut e, 0, &[]);
        let at = seat_pos(&e, 0);
        click(&mut e, MOUSE, at, 1000.0);
        for step in order {
            match step {
                Result => e.handle(Command::HoldResult {
                    request: 1,
                    outcome: HoldOutcome::Ok,
                }),
                Set => private(&mut e),
                Delta => drop(delta(&mut e, 0, &[(0, 1)])),
            }
            assert!(matches!(&visuals(&e)[..1], "p" | "h"), "{order:?}");
        }
        assert_eq!(visuals(&e), "h.........", "{order:?}");
        let events: Vec<Event> = e.drain_events().collect();
        assert!(!summary(&events).iter().any(|l| l.starts_with("error")));
    }
    let mut e = Engine::new(Fake::default());
    private(&mut e);
    let mut state = vec![0; 20];
    state[0] = 1;
    e.handle(Command::SetSnapshot { seq: 3, state });
    e.handle(Command::LoadChart { ccf: compile(HALL) });
    let events: Vec<Event> = e.drain_events().collect();
    assert_eq!(summary(&events), [r#"changed ["S-A-1"]"#]);
    assert!(events.contains(&Event::SelectionChanged {
        selected: vec![e.info(0)],
        expires_at_ms: Some(1),
    }));
    assert_eq!(visuals(&e), "h.........");
}

#[test]
fn hostile_selection_commands_are_bounded() {
    let mut e = hall();
    // Thousands of labels: duplicates collapse, unknown ones are one error.
    let objects = vec![ObjectRef::Label("S-A-1".into()); 5000];
    e.handle(Command::Select { objects });
    let events: Vec<Event> = e.drain_events().collect();
    assert_eq!(summary(&events).len(), 2, "{events:?}");
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
        visual::SELECTED_PENDING
    );
    assert!(summary(&events).iter().any(|l| l.starts_with("hold 1")));
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
        let selected = e.scene.as_ref().unwrap().scene.visual[5] == visual::SELECTED_PENDING;
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

/// The label of the hall seat a mouse at `at` is over, by brute force over
/// the seats on screen (size 40, mouse tolerance up to the half size).
fn under(e: &E, at: [f64; 2]) -> Option<String> {
    let reach = (20.0 * cam(e).zoom).max(4.0);
    let d = |i: u32| {
        let p = seat_pos(e, i);
        ((p[0] - at[0]).powi(2) + (p[1] - at[1]).powi(2)).sqrt()
    };
    let idx = (0..20)
        .filter(|&i| d(i) <= reach)
        .min_by(|&a, &b| d(a).total_cmp(&d(b)))?;
    Some(format!(
        "S-{}-{}",
        ["A", "B"][idx as usize / 10],
        idx % 10 + 1
    ))
}

#[test]
fn hover_follows_the_pointer_after_a_drag_or_resize() {
    // A drag the clamp stops leaves the pointer off its seat: the release
    // hovers what is under it.
    let mut e = hall();
    e.handle(Command::SetReducedMotion(true));
    let start = seat_pos(&e, 0);
    hover(&mut e, start, 100.0);
    let end = [start[0] + 700.0, start[1]];
    let at = |p: [f64; 2]| p.map(|v| v as f32);
    ptr(&mut e, Down, 1, MOUSE, at(start), 200.0);
    ptr(&mut e, Move, 1, MOUSE, at(end), 250.0);
    ptr(&mut e, Up, 1, MOUSE, at(end), 300.0);
    tick(&mut e, 300.0);
    assert_ne!(
        under(&e, end).as_deref(),
        Some("S-A-1"),
        "the drag was clamped"
    );
    assert_eq!(e.hover.map(|i| e.info(i).label), under(&e, end));

    // A resize moves the seats under a still pointer.
    let mut e = hall();
    let [x, y] = e.scene.as_ref().unwrap().scene.pos[3];
    set_camera(&mut e, [x - 100.0, y - 75.0, 200.0, 150.0]);
    tick(&mut e, 0.0);
    let p = seat_pos(&e, 3);
    hover(&mut e, p, 100.0);
    assert_eq!(e.hover, Some(3));
    let camera = cam(&e);
    // The centre stays put, so seats shift by half the change: one pitch.
    resize(&mut e, W + 176.0, H, 1.0, 3.0);
    tick(&mut e, 200.0);
    assert_eq!(cam(&e), camera, "only the viewport changed");
    assert_ne!(under(&e, p).as_deref(), Some("S-A-4"));
    assert_eq!(e.hover.map(|i| e.info(i).label), under(&e, p));
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
