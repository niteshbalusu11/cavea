//! Deterministic simulation (13 §3.2): many short runs, and the same seed run
//! twice giving the same trace.

use cavea_sim::Stats;
use cavea_sim::sim::simulate;

#[test]
fn short_runs_hold_every_invariant_and_replay_identically() {
    let mut stats = Stats::default();
    for seed in 1..=300 {
        let first = simulate(seed, 100, &mut stats).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
        if seed <= 30 {
            let again = simulate(seed, 100, &mut Stats::default());
            assert_eq!(
                Ok(first),
                again,
                "seed {seed}: the same seed gave another trace"
            );
        }
    }
    println!("{stats}");
}
