//! Long runs of the model-based test, for nightly and release budgets
//! (13 §3, §12):
//!
//! ```text
//! cargo run --release -p cavea-sim --example soak -- model \
//!     [--seeds FIRST..END] [--steps STEPS_PER_RUN] [--jobs N]
//! ```
//!
//! Seeds are split across `--jobs` threads (default 4). Prints every fault and
//! check count, the wall time and steps per second; exits 1 on the first
//! failure, with the shrunk case.

use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cavea_sim::Stats;
use cavea_sim::case::explore;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().cloned().unwrap_or_default();
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let (first, end) = flag("--seeds")
        .and_then(|s| {
            let (a, b) = s.split_once("..")?;
            Some((a.parse::<u64>().ok()?, b.parse::<u64>().ok()?))
        })
        .unwrap_or((1, 1_001));
    let steps: usize = flag("--steps").and_then(|s| s.parse().ok()).unwrap_or(500);
    let jobs: u64 = flag("--jobs")
        .and_then(|s| s.parse().ok())
        .unwrap_or(4)
        .max(1);
    if mode != "model" {
        eprintln!("usage: soak model [--seeds A..B] [--steps N] [--jobs N]");
        return ExitCode::from(2);
    }

    let started = Instant::now();
    let total = Arc::new(Mutex::new(Stats::default()));
    let failure = Arc::new(Mutex::new(None::<String>));
    let threads: Vec<_> = (0..jobs)
        .map(|job| {
            let (total, failure) = (total.clone(), failure.clone());
            std::thread::spawn(move || {
                let mut stats = Stats::default();
                let mut seed = first + job;
                while seed < end {
                    if failure.lock().map(|f| f.is_some()).unwrap_or(true) {
                        break;
                    }
                    if let Err(report) = explore(seed, 1, steps, &mut stats) {
                        if let Ok(mut f) = failure.lock() {
                            f.get_or_insert(format!("seed {seed}: {report}"));
                        }
                        break;
                    }
                    seed += jobs;
                }
                if let Ok(mut t) = total.lock() {
                    t.merge(&stats);
                }
            })
        })
        .collect();
    for t in threads {
        let _ = t.join();
    }
    let secs = started.elapsed().as_secs_f64();
    let stats = total.lock().map(|s| s.clone()).unwrap_or_default();
    println!("{mode}: seeds {first}..{end}, {steps} steps per run, {jobs} jobs");
    print!("{stats}");
    println!(
        "wall {secs:.1} s, {:.0} steps/s",
        stats.steps as f64 / secs.max(1e-9)
    );
    match failure.lock().ok().and_then(|f| f.clone()) {
        Some(report) => {
            println!("FAILED {report}");
            ExitCode::from(1)
        }
        None => {
            println!("ok");
            ExitCode::SUCCESS
        }
    }
}
