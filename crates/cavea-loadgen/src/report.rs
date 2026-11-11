//! The run's report: JSON for machines, a summary for people, and the exit
//! code. Keys, hold tokens and order references are never in it.

use std::fmt::Write as _;

use serde_json::{Value, json};

use crate::hist::Hist;
use crate::run::{Config, Outcome};

pub const EXIT_OK: u8 = 0;
/// A correctness check failed: a double hold, a wrong refusal, a protocol
/// error, a view that did not converge.
pub const EXIT_CORRECTNESS: u8 = 1;
pub const EXIT_USAGE: u8 = 2;
pub const EXIT_COULD_NOT_RUN: u8 = 3;
/// Correct, but a budget of 01 §5 was missed.
pub const EXIT_BUDGET: u8 = 4;

fn ms(micros: u64) -> f64 {
    (micros as f64 / 10.0).round() / 100.0
}

fn latency(h: &Hist) -> Value {
    json!({
        "count": h.count(),
        "p50_ms": ms(h.quantile(0.5)), "p95_ms": ms(h.quantile(0.95)),
        "p99_ms": ms(h.quantile(0.99)), "max_ms": ms(h.max()),
    })
}

/// Server budgets (01 §5): name, the operation, the quantile, milliseconds.
/// A hold is measured from when it was sent: its arrival's schedule is
/// charged to the token, the arrival's first request (`start_lag` shows how
/// far the generator itself fell behind, `hold_from_due` the whole arrival).
const BUDGETS: [(&str, &str, f64, f64); 3] = [
    ("hold request p99", "hold", 0.99, 15.0),
    ("snapshot request p99", "snapshot", 0.99, 3.0),
    ("commit to delta delivered p95", "delta", 0.95, 250.0),
];

/// The JSON report and the exit code.
pub fn build(cfg: &Config, profile: &str, o: &Outcome) -> (Value, u8) {
    let ops: serde_json::Map<String, Value> = o
        .stats
        .ops
        .iter()
        .map(|(name, op)| {
            let mut v = latency(&op.hist);
            v["ok"] = json!(op.ok);
            v["errors"] = json!(op.errors);
            ((*name).to_owned(), v)
        })
        .collect();
    let mut budget_missed = false;
    let budgets: Vec<Value> = BUDGETS
        .iter()
        .map(|&(name, op, q, limit)| {
            let h = if op == "delta" {
                Some(&o.stats.delta)
            } else {
                o.stats.ops.get(op).map(|x| &x.hist)
            };
            let measured = h.filter(|h| h.count() > 0).map(|h| ms(h.quantile(q)));
            let ok = measured.map(|m| m <= limit);
            budget_missed |= ok == Some(false);
            json!({"name": name, "budget_ms": limit, "measured_ms": measured, "ok": ok})
        })
        .collect();
    let s = &o.schedule;
    let span = s.span.as_secs_f64().max(1e-3);
    let elapsed = s.elapsed.as_secs_f64().max(1e-3);
    let holds = o
        .stats
        .ops
        .get("hold")
        .map_or(0, |h| h.ok + h.errors.values().sum::<u64>());
    let (equal, differ, unverified) = o.converged;
    let lag_p99 = o
        .stats
        .ops
        .get("start_lag")
        .map_or(0.0, |l| ms(l.hist.quantile(0.99)));
    let cpu = &o.cpu;
    let wall = cpu.wall_s.max(1e-3);
    let (objects_seen, tokens_read, resolution) = match &o.audit {
        Some((a, b, r)) => (Some(*a), Some(*b), Some(r.clone())),
        None => (None, None, None),
    };
    // A view never compared is a convergence not shown.
    let exit = if o.violations_total > 0 || differ > 0 || unverified > 0 {
        EXIT_CORRECTNESS
    } else if budget_missed {
        EXIT_BUDGET
    } else {
        EXIT_OK
    };
    let report = json!({
        "version": 1,
        "profile": profile,
        "settings": {
            "url": cfg.url, "workspace": o.workspace, "event": o.event, "objects": o.objects,
            "subscribers": cfg.subscribers, "connect_rate": cfg.connect_rate, "buyers": cfg.buyers, "ramp_s": cfg.ramp.as_secs_f64(),
            "rate": cfg.rate, "duration_s": cfg.duration.as_secs_f64(),
            "pick": if cfg.clustered { "clustered" } else { "random" },
            "release_share": cfg.release_share, "book_share": cfg.book_share,
            "retry_book_share": cfg.retry_book_share, "blocks_per_sec": cfg.blocks_per_sec,
            "unbook_after_s": cfg.unbook_after.as_secs_f64(),
            "source": format!("{:?}", cfg.source).to_lowercase(), "addresses": cfg.addresses,
            "max_in_flight": cfg.max_in_flight, "probes": cfg.probes, "seed": cfg.seed,
        },
        "wall_s": o.wall.as_secs_f64(),
        "load": {
            "arrivals_offered": s.offered, "arrivals_started": s.started,
            "arrivals_skipped_in_flight_bound": s.skipped,
            "offered_per_s": s.offered as f64 / span, "started_per_s": s.started as f64 / span,
            "holds_per_s": holds as f64 / elapsed, "elapsed_s": elapsed,
            "start_lag_p99_ms": lag_p99,
            // The generator started arrivals late: its figures understate the server's.
            "generator_behind": lag_p99 > 5.0,
        },
        "requests": {
            "retried_after_outage": o.stats.retried,
            "gave_up_after_outage": o.stats.gave_up,
            "unknown_outcomes": o.unknown,
        },
        "ops": ops,
        "delta_delivery": latency(&o.stats.delta),
        "connections": {
            "opened": o.stats.opened, "failed": o.stats.failed, "dropped": o.stats.dropped,
            "resyncs": o.stats.resyncs, "closes": o.stats.closes, "frames": o.stats.frames,
            "reconnects": o.stats.reconnects,
        },
        "checks": {
            "violations_total": o.violations_total,
            "violations": o.violations.iter().map(|v| json!({"check": v.check, "detail": v.detail})).collect::<Vec<_>>(),
            "convergence": {"equal": equal, "differ": differ, "unverified": unverified},
        },
        "budgets": budgets,
        "outages": o.outages.iter().map(|(a, b)| json!({"start_unix_ms": a, "end_unix_ms": b, "s": (b - a) as f64 / 1_000.0})).collect::<Vec<_>>(),
        "subscriber_gaps": o.gaps.iter().map(|(a, b)| json!({"start_unix_ms": a, "end_unix_ms": b, "s": (b - a) as f64 / 1_000.0})).collect::<Vec<_>>(),
        "cpu": {
            "load_wall_s": cpu.wall_s, "generator_cpu_s": cpu.generator_s, "server_cpu_s": cpu.server_s,
            "generator_cores": cpu.generator_s / wall, "server_cores": cpu.server_s.map(|s| s / wall),
        },
        "audit": {
            "objects_not_free": objects_seen, "tokens_read": tokens_read,
            "unknown_resolved": resolution.map(|r| r.into_iter().map(|(k, [a, n, u])| (k.to_owned(), json!({"applied": a, "not_applied": n, "undecided": u}))).collect::<serde_json::Map<_, _>>()),
        },
        "timeseries": o.series,
        "exit": exit,
    });
    (report, exit)
}

pub fn summary(r: &Value) -> String {
    let mut s = String::new();
    let st = &r["settings"];
    let _ = writeln!(
        s,
        "cavea-loadgen {} on {} ({} objects), {:.1} s",
        r["profile"],
        st["event"],
        st["objects"],
        r["wall_s"].as_f64().unwrap_or(0.0)
    );
    let l = &r["load"];
    let _ = writeln!(
        s,
        "  arrivals: {} offered at {:.0}/s, {} started, {} skipped at the in-flight bound; {:.0} holds/s achieved",
        l["arrivals_offered"],
        l["offered_per_s"].as_f64().unwrap_or(0.0),
        l["arrivals_started"],
        l["arrivals_skipped_in_flight_bound"],
        l["holds_per_s"].as_f64().unwrap_or(0.0)
    );
    if let Some(ops) = r["ops"].as_object() {
        for (name, op) in ops {
            let _ = writeln!(
                s,
                "  {name:<10} ok {:>7}  p50 {:>7} ms  p95 {:>7} ms  p99 {:>7} ms  max {:>8} ms  errors {}",
                op["ok"], op["p50_ms"], op["p95_ms"], op["p99_ms"], op["max_ms"], op["errors"]
            );
        }
    }
    let d = &r["delta_delivery"];
    let _ = writeln!(
        s,
        "  delta delivery (hold reply to probe seeing it): n {} p50 {} ms p95 {} ms p99 {} ms max {} ms",
        d["count"], d["p50_ms"], d["p95_ms"], d["p99_ms"], d["max_ms"]
    );
    let c = &r["connections"];
    let _ = writeln!(
        s,
        "  connections: {} opened, {} failed, {} given up, {} reconnects, {} resyncs, closes {}, {} frames",
        c["opened"],
        c["failed"],
        c["dropped"],
        c["reconnects"],
        c["resyncs"],
        c["closes"],
        c["frames"]
    );
    let q = &r["requests"];
    let _ = writeln!(
        s,
        "  outages: {} seen; requests retried {}, given up {}, unknown outcomes {}; subscriber gaps {}",
        r["outages"].as_array().map_or(0, Vec::len),
        q["retried_after_outage"],
        q["gave_up_after_outage"],
        q["unknown_outcomes"],
        r["subscriber_gaps"].as_array().map_or(0, Vec::len)
    );
    let cpu = &r["cpu"];
    let _ = writeln!(
        s,
        "  cpu over {:.1} s of load: generator {:.2} cores, server {} cores; start lag p99 {} ms{}",
        cpu["load_wall_s"].as_f64().unwrap_or(0.0),
        cpu["generator_cores"].as_f64().unwrap_or(0.0),
        cpu["server_cores"]
            .as_f64()
            .map_or("?".into(), |c| format!("{c:.2}")),
        l["start_lag_p99_ms"],
        if l["generator_behind"] == true {
            " (the generator fell behind its schedule)"
        } else {
            ""
        }
    );
    let a = &r["audit"];
    let _ = writeln!(
        s,
        "  audit: {} objects not free, {} tokens read; unknown outcomes resolved {}",
        a["objects_not_free"], a["tokens_read"], a["unknown_resolved"]
    );
    for b in r["budgets"].as_array().into_iter().flatten() {
        let _ = writeln!(
            s,
            "  budget {}: {} ms measured, {} ms allowed: {}",
            b["name"].as_str().unwrap_or(""),
            b["measured_ms"],
            b["budget_ms"],
            match b["ok"].as_bool() {
                Some(true) => "within",
                Some(false) => "MISSED",
                None => "not measured",
            }
        );
    }
    let k = &r["checks"];
    let cv = &k["convergence"];
    let _ = writeln!(
        s,
        "  checks: {} violation(s); views equal to a fresh snapshot {}, differing {}, unverified {}",
        k["violations_total"], cv["equal"], cv["differ"], cv["unverified"]
    );
    for v in k["violations"].as_array().into_iter().flatten().take(20) {
        let _ = writeln!(
            s,
            "    - {}: {}",
            v["check"].as_str().unwrap_or(""),
            v["detail"].as_str().unwrap_or("")
        );
    }
    let _ = writeln!(s, "  exit {}", r["exit"]);
    s
}
