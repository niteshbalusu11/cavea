//! Snapshots, deltas, frames and the pulse (04 §4, §17), driven through
//! `Engine::handle`.

use cavea_format::wire::{self, Change, View};

use super::*;
use crate::protocol::StatusChange;

fn hall_ccf() -> Vec<u8> {
    compile(
        r##"{"schema":1,"name":"Hall","settings":{"seatSize":40},"nextOid":100,
  "categories":[{"key":"a","label":"Stalls","color":"#2563EB"}],
  "nodes":[{"id":1,"type":"rowBlock","category":"a","rows":[{"rid":3,"seats":20,"oids":[[10,20]]}]}]}"##,
    )
}

fn shown(e: &mut E, ccf: Vec<u8>) -> u32 {
    e.handle(Command::LoadChart { ccf });
    resize(e, W, H, 1.0, 3.0);
    tick(e, 0.0);
    e.scene.as_ref().unwrap().dynamic_buf.0
}

fn status(e: &E) -> &[u8] {
    &e.scene.as_ref().unwrap().scene.status
}

/// Uploads to the dynamic buffer since `from`: (byte offset, length).
fn uploads(e: &E, dynamic: u32, from: usize) -> Vec<(u32, usize)> {
    (e.backend.buffer_writes[from..].iter())
        .filter(|w| w.0 == dynamic)
        .map(|w| (w.1, w.2))
        .collect()
}

fn errors(events: &[Event]) -> usize {
    (events.iter())
        .filter(|e| {
            matches!(
                e,
                Event::Error {
                    code: ErrorCode::InvalidInput,
                    ..
                }
            )
        })
        .count()
}

#[test]
fn a_snapshot_is_one_upload_and_waits_for_the_chart() {
    // Before the chart: kept, applied on decode, uploaded once.
    let mut e = Engine::new(Fake::default());
    let mut state = vec![0; 20];
    state[3] = 1;
    e.handle(Command::SetSnapshot { seq: 7, state });
    let dynamic = shown(&mut e, hall_ccf());
    assert_eq!(status(&e)[..5], [0, 0, 0, 1, 0]);
    assert_eq!(uploads(&e, dynamic, 0), [(0, 80)]);
    assert_eq!(e.state.seq, Some(7));

    // Wrong count, a code above 1, a malformed frame: nothing changes.
    let writes = e.backend.buffer_writes.len();
    for state in [vec![0; 19], vec![2; 20]] {
        e.handle(Command::SetSnapshot { seq: 8, state });
        assert_eq!(errors(&e.drain_events().collect::<Vec<_>>()), 1);
    }
    let frame = wire::encode_snapshot(View::Public, 9, &[0; 20], &[]).unwrap();
    assert!(Command::from_frame(&frame[..frame.len() - 1], Some(20)).is_err());
    tick(&mut e, 16.0);
    assert_eq!(
        (e.backend.buffer_writes.len(), e.state.seq),
        (writes, Some(7))
    );
    assert_eq!(status(&e)[3], 1);

    // A new snapshot replaces everything, in one upload.
    let mut statuses = [0; 20];
    statuses[10] = 4;
    let frame = wire::encode_snapshot(View::Public, 9, &statuses, &[]).unwrap();
    let command = Command::from_frame(&frame, Some(20)).unwrap().unwrap();
    e.handle(command);
    tick(&mut e, 32.0);
    assert_eq!(uploads(&e, dynamic, writes), [(0, 80)]);
    assert_eq!(status(&e).iter().position(|&s| s == 1), Some(10));
    assert_eq!(e.state.seq, Some(9));
}

#[test]
fn deltas_apply_only_in_sequence() {
    let mut e = Engine::new(Fake::default());
    shown(&mut e, hall_ccf());
    // Before any snapshot: resync, once, and nothing changes.
    let resync = |events: &[Event]| -> Vec<Option<u64>> {
        (events.iter())
            .filter_map(|e| match e {
                Event::Resync { seq } => Some(*seq),
                _ => None,
            })
            .collect()
    };
    assert_eq!(resync(&delta(&mut e, 0, &[(1, 1)])), [None]);
    assert!(resync(&delta(&mut e, 4, &[(1, 1)])).is_empty());
    snapshot(&mut e, 4, &[]);
    // In sequence, including a delta with no changes.
    assert!(delta(&mut e, 4, &[(1, 1), (2, 1)]).is_empty());
    assert!(delta(&mut e, 5, &[]).is_empty());
    // A gap or a repeat: resync from the state shown.
    assert_eq!(resync(&delta(&mut e, 5, &[(3, 1)])), [Some(6)]);
    assert!(resync(&delta(&mut e, 9, &[(3, 1)])).is_empty());
    assert_eq!(status(&e)[..4], [0, 1, 1, 0]);
    // Malformed: refused whole.
    let bad = [&[(20, 1)][..], &[(2, 0), (1, 0)], &[(1, 3)]];
    for changes in bad {
        assert_eq!(errors(&delta(&mut e, 6, changes)), 1);
    }
    assert_eq!(status(&e)[..4], [0, 1, 1, 0]);
    assert_eq!(e.state.seq, Some(6));
    // A snapshot resets the request: the next gap asks again.
    snapshot(&mut e, 10, &[]);
    assert_eq!(resync(&delta(&mut e, 11, &[])), [Some(10)]);
}

#[test]
fn frames_decode_to_state_commands() {
    let delta = wire::encode_delta(View::Public, 3, 4, &[Change { idx: 2, code: 9 }], 20, &[]);
    assert_eq!(
        Command::from_frame(&delta.unwrap(), Some(20)),
        Ok(Some(Command::ApplyDelta {
            from_seq: 3,
            to_seq: 4,
            changes: vec![StatusChange { idx: 2, status: 1 }],
        }))
    );
    let private = wire::encode_private(99, &[1, 5], 20, &[]).unwrap();
    assert_eq!(
        Command::from_frame(&private, Some(20)),
        Ok(Some(Command::SetPrivateHolds {
            objects: vec![1, 5],
            expires_at_ms: 99,
        }))
    );
    // Any `idx` past the receiver's objects refuses the whole frame, even in
    // the area entries the engine ignores.
    let area = [wire::Area { idx: 99, value: 1 }];
    let delta = wire::encode_delta(View::Public, 3, 4, &[], 100, &area).unwrap();
    assert_eq!(
        Command::from_frame(&delta, Some(20)),
        Err(wire::WireError::OutOfRange)
    );
    assert!(Command::from_frame(&delta, Some(100)).is_ok());
    // Before the chart, area entries could never be checked: refused. A
    // frame without them is kept for the engine to check.
    let private = wire::encode_private(9, &[0], 100, &area).unwrap();
    assert_eq!(
        Command::from_frame(&private, None),
        Err(wire::WireError::OutOfRange)
    );
    let private = wire::encode_private(9, &[0], 100, &[]).unwrap();
    assert!(matches!(Command::from_frame(&private, None), Ok(Some(_))));
    // The adapter's frames and unknown types are not state.
    for frame in [wire::encode_ping(1), wire::encode_resync(), vec![0x7f, 1]] {
        assert_eq!(Command::from_frame(&frame, Some(20)), Ok(None));
    }
}

/// The engine's status array equals the direct application of seeded random
/// snapshots and deltas, gaps included, and the buffer shows it.
#[test]
fn state_matches_a_direct_model() {
    let mut e = Engine::new(Fake::default());
    let ccf = cavea_compiler::compile(&cavea_testkit::synthetic::stadium(2000, 3))
        .unwrap()
        .public;
    let dynamic = shown(&mut e, ccf);
    let n = e.objects().unwrap();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut model: Vec<u8> = Vec::new();
    let mut seq = 0;
    for round in 0..300 {
        let r = rng.range(0.0, 1.0);
        if round == 0 || r < 0.05 {
            model = (0..n)
                .map(|_| u8::from(rng.range(0.0, 1.0) < 0.3))
                .collect();
            seq += 1;
            e.handle(Command::SetSnapshot {
                seq,
                state: model.clone(),
            });
        } else {
            let mut changes: Vec<StatusChange> = (0..rng.range(0.0, 60.0) as u32)
                .map(|_| StatusChange {
                    idx: rng.range(0.0, n as f64) as u32,
                    status: u8::from(rng.range(0.0, 1.0) < 0.5),
                })
                .collect();
            changes.sort_by_key(|c| c.idx);
            changes.dedup_by_key(|c| c.idx);
            let gap = r < 0.1;
            if !gap {
                changes
                    .iter()
                    .for_each(|c| model[c.idx as usize] = c.status);
            }
            let from_seq = if gap { seq + 1 } else { seq };
            seq += u64::from(!gap);
            e.handle(Command::ApplyDelta {
                from_seq,
                to_seq: from_seq + 1,
                changes,
            });
        }
        if r > 0.5 {
            tick(&mut e, f64::from(round) * 16.0);
        }
    }
    settle(&mut e, 10_000.0);
    assert_eq!(status(&e), &model[..]);
    let buffer = &e.backend.buffers[&dynamic];
    let shown: Vec<u8> = buffer.chunks(4).map(|d| d[2]).collect();
    assert_eq!(shown, model, "visual 1 is unavailable, 0 available");
    assert!(buffer.chunks(4).all(|d| d[3] == 0), "pulses ended");
}

fn fx(e: &E, idx: usize) -> u8 {
    e.scene.as_ref().unwrap().scene.fx[idx]
}

#[test]
fn seats_taken_by_a_delta_pulse_for_400_ms() {
    let mut e = Engine::new(Fake::default());
    shown(&mut e, hall_ccf());
    snapshot(&mut e, 1, &[]);
    tick(&mut e, 10.0);
    // Two deltas before one frame are one batch; a freed seat does not pulse.
    delta(&mut e, 1, &[(2, 1), (3, 1)]);
    delta(&mut e, 2, &[(3, 0), (4, 1)]);
    assert_eq!((fx(&e, 2), fx(&e, 3), fx(&e, 4)), (4, 4, 4));
    assert_eq!(e.state.pulses.len(), 1);
    assert!(wants_frame(&tick(&mut e, 100.0)));
    assert!(wants_frame(&tick(&mut e, 499.0)));
    assert_eq!(fx(&e, 4), 4);
    // Over at 400 ms: cleared, drawn once more, then idle.
    let draws = e.backend.draws.len();
    assert!(!wants_frame(&tick(&mut e, 500.0)));
    assert_eq!(
        (fx(&e, 2), fx(&e, 4), e.backend.draws.len()),
        (0, 0, draws + 1)
    );
    assert!(tick(&mut e, 600.0).is_empty());

    // No pulse on a snapshot, under reduced motion, or for the buyer's seat.
    snapshot(&mut e, 5, &[7]);
    e.handle(Command::Select {
        objects: vec![crate::protocol::ObjectRef::Idx(8)],
    });
    delta(&mut e, 5, &[(8, 1), (9, 1)]);
    assert_eq!((fx(&e, 7), fx(&e, 8), fx(&e, 9)), (0, 0, 4));
    e.handle(Command::SetReducedMotion(true));
    assert_eq!(fx(&e, 9), 0, "reduced motion ends running pulses");
    delta(&mut e, 6, &[(10, 1)]);
    assert_eq!((fx(&e, 10), e.state.pulses.len()), (0, 0));
    settle(&mut e, 700.0);

    // 10,000 seats taken at once: one batch.
    let mut e = Engine::new(Fake::default());
    let ccf = cavea_compiler::compile(&cavea_testkit::synthetic::stadium(20_000, 1))
        .unwrap()
        .public;
    shown(&mut e, ccf);
    snapshot(&mut e, 1, &[]);
    let changes: Vec<(u32, u8)> = (0..10_000).map(|i| (i * 2, 1)).collect();
    delta(&mut e, 1, &changes);
    assert_eq!(e.state.pulses.len(), 1);
    let end = settle(&mut e, 16.0);
    assert!((400.0..=432.0).contains(&end), "{end}");
}

/// Snapshot and delta application at 100,000 seats. Run with `cargo test
/// --release -p cavea-render measure_ -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, not a test"]
fn measure_state() {
    let mut e = Engine::new(Fake::default());
    let ccf = cavea_compiler::compile(&cavea_testkit::synthetic::stadium(100_000, 1))
        .unwrap()
        .public;
    let dynamic = shown(&mut e, ccf);
    let n = e.objects().unwrap();
    let mut rng = Rng(11);
    let state: Vec<u8> = (0..n)
        .map(|_| u8::from(rng.range(0.0, 1.0) < 0.3))
        .collect();
    let frame = wire::encode_snapshot(View::Public, 1, &state, &[]).unwrap();
    let start = std::time::Instant::now();
    let command = Command::from_frame(&frame, Some(n as u32))
        .unwrap()
        .unwrap();
    let decoded = start.elapsed();
    e.handle(command);
    let applied = start.elapsed();
    tick(&mut e, 16.0);
    println!(
        "snapshot of {n} ({} B frame): decode {decoded:?}, decode and apply {applied:?}, with upload {:?}",
        frame.len(),
        start.elapsed()
    );
    // 10,000 changes spread over the chart, as one coalesced delta.
    let mut changes: Vec<Change> = (0..10_000)
        .map(|_| Change {
            idx: rng.range(0.0, n as f64) as u32,
            code: u8::from(rng.range(0.0, 1.0) < 0.5),
        })
        .collect();
    wire::coalesce(&mut changes);
    let frame = wire::encode_delta(View::Public, 1, 2, &changes, n as u32, &[]).unwrap();
    let writes = e.backend.buffer_writes.len();
    let start = std::time::Instant::now();
    let command = Command::from_frame(&frame, Some(n as u32))
        .unwrap()
        .unwrap();
    e.handle(command);
    let applied = start.elapsed();
    tick(&mut e, 32.0);
    let upload: usize = uploads(&e, dynamic, writes).iter().map(|u| u.1).sum();
    println!(
        "delta of {} changes: decode and apply {applied:?}, with the frame's encode and upload {:?}, {upload} B uploaded",
        changes.len(),
        start.elapsed()
    );
}
