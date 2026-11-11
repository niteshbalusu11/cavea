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
