//! `cavea-loadgen`: simulated buyers and a backend against a running
//! `cavea-server` (docs/13-testing-benchmarks.md §7.1).

mod check;
mod hist;
mod http;
mod report;
mod run;
mod schedule;
mod view;

use std::net::{SocketAddr, ToSocketAddrs as _};
use std::time::Duration;

use run::{Config, Source};

const USAGE: &str =
    "usage: cavea-loadgen --url http://HOST:PORT (--chart DOC.json | --event KEY) [options]
  Setting up (--chart, a document from `cavea gen`) needs CAVEA_ADMIN_KEY; an existing
  event needs CAVEA_SECRET_KEY and CAVEA_PUBLISHABLE_KEY. Keys are read from the
  environment only and never printed.
  --profile l1|l2         defaults for 13 §7.2: l1 a stampede (50,000 subscribers,
                          5,000 buyers within 3 s), l2 2,000 holds/s for 600 s
  --subscribers N         watchers that never hold
  --buyers N --ramp-ms MS a stampede: N buyers, each subscribed, holding within MS
  --rate R --duration-s S sustained: R buyer arrivals a second for S seconds
  --pick clustered|random --release-share F --book-share F --retry-book-share F
  --blocks-per-sec F      backend blocks and unblocks free seats
  --hold-ttl S --max-objects N   the event's settings when setting up
  --source loopback|forwarded|single --addresses N   distinct client addresses
  --max-in-flight N --probes N --seed N --interval-ms MS --quiet-ms MS
  --server-pid PID        sample the server's RSS (same machine only)
  --json FILE             write the JSON report there (the summary goes to stdout)
Exit: 0 ok, 1 a correctness check failed, 2 usage, 3 could not run, 4 a budget missed.";

/// The configuration, the profile's name and the JSON report's path.
fn parse(
    args: &[String],
    env: impl Fn(&str) -> Option<String>,
) -> Result<(Config, String, Option<String>), String> {
    let mut map = std::collections::HashMap::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "-h" || a == "--help" {
            return Err(String::new());
        }
        let Some(name) = a.strip_prefix("--") else {
            return Err(format!("unexpected argument {a:?}"));
        };
        let value = it.next().ok_or_else(|| format!("--{name} needs a value"))?;
        map.insert(name.to_owned(), value.clone());
    }
    let profile = map.remove("profile").unwrap_or_else(|| "custom".into());
    let (mut subscribers, mut buyers, mut ramp, mut rate, mut duration) = match profile.as_str() {
        "l1" => (50_000, 5_000, 3_000, 0.0, 0),
        "l2" => (100, 0, 0, 2_000.0, 600),
        "custom" => (100, 300, 3_000, 0.0, 0),
        p => return Err(format!("unknown profile {p:?}")),
    };
    let num = |m: &mut std::collections::HashMap<String, String>,
               k: &str,
               d: f64|
     -> Result<f64, String> {
        m.remove(k).map_or(Ok(d), |v| {
            v.parse::<f64>()
                .ok()
                .filter(|x| x.is_finite() && *x >= 0.0)
                .ok_or_else(|| format!("--{k} takes a number"))
        })
    };
    subscribers = num(&mut map, "subscribers", subscribers as f64)? as u64;
    buyers = num(&mut map, "buyers", buyers as f64)? as u64;
    ramp = num(&mut map, "ramp-ms", ramp as f64)? as u64;
    rate = num(&mut map, "rate", rate)?;
    duration = num(&mut map, "duration-s", duration as f64)? as u64;
    let url = map.remove("url").ok_or("--url is required")?;
    let hostport = url
        .strip_prefix("http://")
        .ok_or(
            "--url must be http://: the generator speaks no TLS, so run it on the server's network",
        )?
        .trim_end_matches('/');
    let server: SocketAddr = hostport
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .ok_or_else(|| format!("cannot resolve {hostport}"))?;
    let source = match map.remove("source").as_deref() {
        None if server.ip().is_loopback() && cfg!(target_os = "linux") => Source::Loopback,
        None | Some("single") => Source::Single,
        Some("loopback") if server.ip().is_loopback() => Source::Loopback,
        Some("loopback") => {
            return Err("--source loopback needs a server on a loopback address".into());
        }
        Some("forwarded") => Source::Forwarded,
        Some(s) => return Err(format!("unknown --source {s:?}")),
    };
    let clustered = match map.remove("pick").as_deref() {
        None | Some("clustered") => true,
        Some("random") => false,
        Some(p) => return Err(format!("unknown --pick {p:?}")),
    };
    let chart = map
        .remove("chart")
        .map(|p| std::fs::read_to_string(&p).map_err(|e| format!("cannot read {p}: {e}")))
        .transpose()?;
    let event = map.remove("event");
    if chart.is_some() == event.is_some() {
        return Err("give one of --chart and --event".into());
    }
    let release_share = num(&mut map, "release-share", 0.2)?;
    let book_share = num(&mut map, "book-share", 0.5)?;
    if release_share + book_share > 1.0 {
        return Err("--release-share plus --book-share is at most 1".into());
    }
    let arrivals = if rate > 0.0 {
        (rate * 30.0) as u64
    } else {
        buyers
    };
    let cfg = Config {
        url: url.clone(),
        server,
        chart,
        event,
        admin_key: env("CAVEA_ADMIN_KEY"),
        secret_key: env("CAVEA_SECRET_KEY"),
        publishable_key: env("CAVEA_PUBLISHABLE_KEY"),
        subscribers,
        buyers,
        ramp: Duration::from_millis(ramp),
        rate,
        duration: Duration::from_secs(duration),
        clustered,
        release_share,
        book_share,
        retry_book_share: num(&mut map, "retry-book-share", 0.1)?,
        blocks_per_sec: num(&mut map, "blocks-per-sec", 0.0)?,
        hold_ttl: num(&mut map, "hold-ttl", 60.0)? as u64,
        max_objects: num(&mut map, "max-objects", 10.0)? as u32,
        source,
        // Each address used by one subscriber or buyer, or (sustained) once in
        // 30 s: well inside 120 holds a minute per address.
        addresses: num(&mut map, "addresses", (subscribers + arrivals + 16) as f64)? as u64,
        max_in_flight: num(&mut map, "max-in-flight", 4_096.0)? as usize,
        probes: num(&mut map, "probes", 8.0)? as u64,
        seed: num(&mut map, "seed", 1.0)? as u64,
        interval: Duration::from_millis(num(&mut map, "interval-ms", 1_000.0)? as u64),
        quiet: Duration::from_millis(num(&mut map, "quiet-ms", 2_000.0)? as u64),
        server_pid: map
            .remove("server-pid")
            .map(|p| {
                p.parse()
                    .map_err(|_| "--server-pid takes a number".to_owned())
            })
            .transpose()?,
    };
    let json = map.remove("json");
    if let Some(k) = map.keys().next() {
        return Err(format!("unknown option --{k}"));
    }
    if cfg.addresses == 0 || cfg.addresses > (1 << 24) - (1 << 16) {
        return Err("--addresses is 1 to 16,711,680".into());
    }
    Ok((cfg, profile, json))
}

/// The open-file limit this process has, from `/proc/self/limits`.
fn open_files_limit() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/limits").ok()?;
    let line = text.lines().find(|l| l.starts_with("Max open files"))?;
    line.split_whitespace().nth(3)?.parse().ok()
}

fn main() {
    std::process::exit(i32::from(real_main()));
}

fn real_main() -> u8 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cfg, profile, json_path) = match parse(&args, |k| std::env::var(k).ok()) {
        Ok(p) => p,
        Err(e) if e.is_empty() => {
            println!("{USAGE}");
            return report::EXIT_OK;
        }
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return report::EXIT_USAGE;
        }
    };
    // Each subscription is a socket here and one on the server; each buyer in
    // flight one more.
    let stampede = cfg.rate <= 0.0;
    let sockets =
        cfg.subscribers + if stampede { cfg.buyers * 2 } else { 0 } + cfg.max_in_flight as u64 + 64;
    if let Some(limit) = open_files_limit()
        && limit < sockets
    {
        eprintln!(
            "this run needs about {sockets} open files and the limit is {limit}: raise it with `ulimit -n {sockets}` \
             here, and for the server too"
        );
        return report::EXIT_COULD_NOT_RUN;
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot start the runtime: {e}");
            return report::EXIT_COULD_NOT_RUN;
        }
    };
    let outcome = match runtime.block_on(run::run(cfg.clone())) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("could not run: {e:#}");
            return report::EXIT_COULD_NOT_RUN;
        }
    };
    let (json, exit) = report::build(&cfg, &profile, &outcome);
    print!("{}", report::summary(&json));
    if let Some(path) = json_path {
        let text = serde_json::to_string_pretty(&json).unwrap_or_default();
        if let Err(e) = std::fs::write(&path, text + "\n") {
            eprintln!("cannot write {path}: {e}");
            return report::EXIT_COULD_NOT_RUN;
        }
    }
    exit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::Violation;
    use crate::run::{Outcome, Stats};
    use crate::schedule::Schedule;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn profiles_scale_down_and_refuse_what_cannot_run() {
        let none = |_: &str| None;
        let (c, p, _) = parse(
            &args(
                "--url http://127.0.0.1:9 --event e --profile l1 --subscribers 2000 --buyers 300",
            ),
            none,
        )
        .unwrap();
        assert_eq!(
            (p.as_str(), c.subscribers, c.buyers, c.rate, c.source),
            ("l1", 2_000, 300, 0.0, Source::Loopback)
        );
        let (c, ..) = parse(
            &args("--url http://127.0.0.1:9 --event e --profile l2 --rate 300 --duration-s 60"),
            none,
        )
        .unwrap();
        assert_eq!(
            (c.rate, c.duration.as_secs(), c.addresses),
            (300.0, 60, 100 + 9_000 + 16)
        );
        for bad in [
            "--url https://127.0.0.1:9 --event e",
            "--url http://127.0.0.1:9",
            "--url http://127.0.0.1:9 --event e --source loopback --profile l9",
            "--url http://127.0.0.1:9 --event e --release-share 0.6 --book-share 0.6",
            "--url http://127.0.0.1:9 --event e --bogus 1",
        ] {
            assert!(parse(&args(bad), none).is_err(), "{bad}");
        }
    }

    /// The JSON report's shape, and the exit codes in order of precedence.
    #[test]
    fn report_shape_and_exit_codes() {
        let none = |_: &str| None;
        let (cfg, ..) = parse(&args("--url http://127.0.0.1:9 --event e"), none).unwrap();
        let outcome = |violations: Vec<Violation>, hold_ms: u64| {
            let mut stats = Stats::default();
            let op = stats.ops.entry("hold").or_default();
            op.ok = 10;
            for _ in 0..10 {
                op.hist.record(hold_ms * 1_000);
            }
            Outcome {
                workspace: None,
                event: "e".into(),
                objects: 10,
                stats,
                schedule: Schedule::default(),
                violations_total: violations.len() as u64,
                violations,
                converged: (3, 0, 0),
                series: vec![],
                wall: Duration::from_secs(1),
            }
        };
        let (r, exit) = report::build(&cfg, "custom", &outcome(vec![], 2));
        assert_eq!(exit, report::EXIT_OK);
        for k in [
            "version",
            "profile",
            "settings",
            "wall_s",
            "load",
            "ops",
            "delta_delivery",
            "connections",
            "checks",
            "budgets",
            "timeseries",
            "exit",
        ] {
            assert!(r.get(k).is_some(), "{k}");
        }
        assert_eq!(r["budgets"][0]["ok"], true);
        assert_eq!(r["budgets"][1]["ok"], serde_json::Value::Null);
        assert!(!r.to_string().contains("CAVEA_"));
        assert_eq!(
            report::build(&cfg, "custom", &outcome(vec![], 20)).1,
            report::EXIT_BUDGET
        );
        let v = Violation {
            check: "double_hold",
            detail: "x".into(),
        };
        assert_eq!(
            report::build(&cfg, "custom", &outcome(vec![v], 20)).1,
            report::EXIT_CORRECTNESS
        );
    }
}
