//! Model-based property tests (13 §3.1) and the saved regression cases.

use cavea_sim::Stats;
use cavea_sim::case::{Case, explore};

#[test]
fn generated_sequences_match_the_model() {
    let mut stats = Stats::default();
    if let Err(report) = explore(1, 1_000, 150, &mut stats) {
        panic!("{report}");
    }
    println!("{stats}");
}

/// Every file in `cases/` replays clean.
#[test]
fn saved_cases_replay() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("cases");
    let mut stats = Stats::default();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let src = std::fs::read_to_string(&path).unwrap();
        let case = Case::parse(&src).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        if let Err(f) = case.check(&mut stats) {
            panic!("{}: step {}: {}", path.display(), f.step, f.violation);
        }
    }
}

/// The replay refuses a booking whose details name an object twice or one
/// not being booked (02 §6): the reconciler must not accept such a log.
#[test]
fn replay_refuses_malformed_booking_details() {
    use cavea_engine::Replay;
    use cavea_engine::{LogDetail, LogRecord, Op};
    let book = |details: Vec<LogDetail>| LogRecord {
        seq: 1,
        op: Op::Book,
        actor: "a".into(),
        hold_id: None,
        oids: vec![cavea_sim::oid(0)],
        expires_at: None,
        max_objects: None,
        order_ref: Some("o".into()),
        status: None,
        details,
    };
    let (o0, o1) = (cavea_sim::oid(0), cavea_sim::oid(1));
    assert!(
        Replay::default()
            .apply(&book(vec![(o0, None, None)]))
            .is_ok()
    );
    assert!(
        Replay::default()
            .apply(&book(vec![(o0, None, None), (o0, None, None)]))
            .is_err()
    );
    assert!(
        Replay::default()
            .apply(&book(vec![(o1, None, None)]))
            .is_err()
    );
}
