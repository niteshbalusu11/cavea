//! The keyboard baseline (11 §3): arrows move a virtual focus, Enter selects
//! it under the rules a tap obeys, the focus is remembered while hidden, and
//! the camera follows it.

use super::select::{bowl, click, hall, seat_pos, seat_px, summary};
use super::*;
use crate::protocol::HoldOutcome;

fn key(e: &mut E, key: &str) -> Vec<Event> {
    e.drain_events().for_each(drop);
    e.handle(Command::Key {
        key: key.into(),
        modifiers: Modifiers::default(),
    });
    e.drain_events().collect()
}

/// The labels `ObjectFocus` reported, `None` for a hidden focus.
fn focused(events: &[Event]) -> Vec<Option<String>> {
    (events.iter())
        .filter_map(|e| match e {
            Event::ObjectFocus(o) => Some(o.as_ref().map(|(o, _)| o.label.clone())),
            _ => None,
        })
        .collect()
}

fn shown(label: &str) -> Vec<Option<String>> {
    vec![Some(label.to_owned())]
}

#[test]
fn arrows_move_the_focus_and_enter_selects_it_as_a_tap() {
    let mut e = hall();
    // The first arrow focuses the seat nearest the view's centre.
    // The centre is as far from four seats: the lowest `idx` wins.
    assert_eq!(focused(&key(&mut e, "ArrowRight")), shown("S-A-5"));
    assert_eq!(focused(&key(&mut e, "ArrowRight")), shown("S-A-6"));
    assert_eq!(focused(&key(&mut e, "ArrowDown")), shown("S-B-6"));
    assert_eq!(focused(&key(&mut e, "ArrowLeft")), shown("S-B-5"));
    // At the edge the focus stays and nothing is reported.
    assert!(focused(&key(&mut e, "ArrowDown")).is_empty());
    // The ring is drawn: the seats, then a halo and a ring.
    tick(&mut e, 100.0);
    assert_eq!(e.backend.draws.last().map(Vec::len), Some(1));
    assert_eq!(e.backend.draws.last().unwrap()[0], 0..2);

    key(&mut e, "Enter");
    assert_eq!(
        summary(&tick(&mut e, 200.0)),
        ["changed [\"S-B-5\"]", "hold 1 [\"S-B-5\"]"]
    );
    // The focused seat's state changes: the focus is reported again.
    e.handle(Command::HoldResult {
        request: 1,
        outcome: HoldOutcome::Ok,
    });
    assert_eq!(focused(&tick(&mut e, 400.0)), shown("S-B-5"));
    // The same refusals as a tap: over the maximum, and not free.
    e.handle(Command::SetRules {
        max: Some(1),
        min: None,
        validators: Vec::new(),
        group_size: None,
    });
    key(&mut e, "ArrowLeft");
    let events = [key(&mut e, " "), tick(&mut e, 500.0)].concat();
    assert_eq!(summary(&events), ["invalid max [\"S-B-4\"]"]);
    snapshot(&mut e, 1, &[12]);
    key(&mut e, "ArrowLeft");
    let events = [key(&mut e, "Enter"), tick(&mut e, 600.0)].concat();
    assert_eq!(summary(&events), ["invalid not_free [\"S-B-3\"]"]);
}

#[test]
fn escape_tab_and_taps_keep_the_focus_in_memory() {
    let mut e = hall();
    key(&mut e, "ArrowRight");
    assert!(e.takes_key("Escape", Modifiers::default()));
    assert_eq!(focused(&key(&mut e, "Escape")), [None]);
    // Hidden, Escape is the page's again; the next arrow resumes.
    assert!(!e.takes_key("Escape", Modifiers::default()));
    assert_eq!(focused(&key(&mut e, "ArrowRight")), shown("S-A-6"));
    // Tabbing away hides it; tabbing back shows the same seat.
    e.handle(Command::SetFocusShown(false));
    assert_eq!(focused(&e.drain_events().collect::<Vec<_>>()), [None]);
    e.handle(Command::SetFocusShown(true));
    assert_eq!(
        focused(&e.drain_events().collect::<Vec<_>>()),
        shown("S-A-6")
    );
    // A tap moves the focus to the tapped seat without showing it.
    let at = seat_pos(&e, 12);
    let events = click(&mut e, PointerType::Mouse, at, 1000.0);
    assert_eq!(focused(&events), [None]);
    assert_eq!(focused(&key(&mut e, "ArrowRight")), shown("S-B-4"));
    // Keys with Ctrl, Alt or Meta, and others, are the page's.
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    assert!(!e.takes_key("ArrowRight", ctrl) && !e.takes_key("Tab", Modifiers::default()));
    // A new chart forgets the focus.
    e.handle(Command::LoadChart {
        ccf: compile(super::select::HALL),
    });
    assert_eq!(focused(&e.drain_events().collect::<Vec<_>>()), [None]);
    e.handle(Command::SetFocusShown(true));
    assert!(focused(&e.drain_events().collect::<Vec<_>>()).is_empty());
}

#[test]
fn the_camera_follows_the_focus() {
    // At overview no seat is drawn: the camera zooms in on the focus.
    let mut e = bowl();
    assert!(seat_px(&e) < crate::tuning::LOD_SEATS_SEAT_PX);
    let events = key(&mut e, "ArrowUp");
    let Some(Event::ObjectFocus(Some((_, at)))) = events.first() else {
        panic!("{events:?}");
    };
    let at = at.map(f64::from);
    let mut events = Vec::new();
    for t in 0..100 {
        let tick = tick(&mut e, f64::from(t) * 16.0);
        let more = wants_frame(&tick);
        events.extend(tick);
        if !more {
            break;
        }
    }
    let idx = e.focus.unwrap();
    assert!((seat_px(&e) - crate::tuning::LOD_DETAIL_SEAT_PX).abs() < 0.5);
    // Reported where the camera came to rest, and again once it has: the
    // camera's moves hid the tooltip.
    assert!(close(at, seat_pos(&e, idx), 0.01), "{at:?}");
    let last = |f: fn(&Event) -> bool| events.iter().rposition(f);
    let focus = last(|e| matches!(e, Event::ObjectFocus(Some(_))));
    let camera = last(|e| matches!(e, Event::CameraChanged { .. }));
    assert!(focus > camera && camera.is_some(), "{focus:?} {camera:?}");
    assert_eq!(focused(&events).len(), 1);
    // Moving along: the camera pans only when the focus nears the edge.
    let mut moves = 0;
    let mut camera = cam(&e);
    for i in 0..60 {
        key(&mut e, "ArrowRight");
        settle(&mut e, 1000.0 * f64::from(i + 1));
        let [x, y] = seat_pos(&e, e.focus.unwrap());
        assert!(
            (31.9..=W as f64 - 31.9).contains(&x) && (31.9..=H as f64 - 31.9).contains(&y),
            "step {i}: {x} {y}"
        );
        if cam(&e) != camera {
            moves += 1;
            camera = cam(&e);
        }
    }
    assert!((1..30).contains(&moves), "{moves} camera moves");
    // Plus, minus and 0 zoom.
    let zoom = cam(&e).zoom;
    key(&mut e, "+");
    settle(&mut e, 100_000.0);
    assert!((cam(&e).zoom / zoom - 2.0).abs() < 1e-6);
    key(&mut e, "0");
    settle(&mut e, 200_000.0);
    assert!(seat_px(&e) < crate::tuning::LOD_SEATS_SEAT_PX);
}
