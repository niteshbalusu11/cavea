//! A few seconds of each load shape against a real `cavea-server` (the
//! workspace's build, as a child process) on a database of its own, then the
//! reconciler over everything they wrote.

#![allow(
    clippy::unwrap_used,
    reason = "test helpers outside #[test] functions; a panic is a test failure"
)]

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const ADMIN: &str = "admin-key-for-the-loadgen-test-0123456789";

struct Cleanup {
    server: Option<Child>,
    admin_url: String,
    db: String,
    files: Vec<PathBuf>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(mut s) = self.server.take() {
            let _ = s.kill();
            let _ = s.wait();
        }
        let _ = psql(
            &self.admin_url,
            &format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.db),
        );
        for f in &self.files {
            let _ = std::fs::remove_file(f);
        }
    }
}

fn psql(url: &str, sql: &str) -> std::io::Result<std::process::ExitStatus> {
    Command::new("psql")
        .args([url, "-qc", sql])
        .stdout(Stdio::null())
        .status()
}

/// The workspace's `cavea-server`, built next to this test.
fn server_binary() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap();
    let bin = dir.join("cavea-server");
    assert!(
        bin.exists(),
        "build the server first: cargo build -p cavea-server ({})",
        bin.display()
    );
    bin
}

fn healthy(port: u16) -> bool {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    let _ = s.write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out.starts_with("HTTP/1.1 200")
}

/// The server on `port`, once it answers.
fn start(url: &str, port: u16) -> Child {
    let server = Command::new(server_binary())
        .env("CAVEA_DATABASE_URL", url)
        .env("CAVEA_LISTEN", format!("127.0.0.1:{port}"))
        .env("CAVEA_ADMIN_KEY", ADMIN)
        .env("CAVEA_NODE_ID", "loadgen-live-test")
        .env("CAVEA_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !healthy(port) {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "the server did not start"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    server
}

fn generator(port: u16, chart: &PathBuf, report: &PathBuf, extra: &str) -> Child {
    Command::new(env!("CARGO_BIN_EXE_cavea-loadgen"))
        .args(["--url", &format!("http://127.0.0.1:{port}"), "--chart"])
        .arg(chart)
        .arg("--json")
        .arg(report)
        .args(extra.split_whitespace())
        .env("CAVEA_ADMIN_KEY", ADMIN)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn finish(mut child: Child, report: &PathBuf) -> (i32, Value) {
    let status = child.wait().unwrap();
    let report: Value = serde_json::from_str(&std::fs::read_to_string(report).unwrap()).unwrap();
    (status.code().unwrap_or(-1), report)
}

fn loadgen(port: u16, chart: &PathBuf, report: &PathBuf, extra: &str) -> (i32, Value) {
    finish(generator(port, chart, report, extra), report)
}

#[test]
#[ignore = "needs PostgreSQL: set CAVEA_TEST_DATABASE_URL, pass --include-ignored"]
fn short_runs_check_clean_and_the_reconciler_agrees() {
    let admin_url = std::env::var("CAVEA_TEST_DATABASE_URL").unwrap();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    let db = format!("loadgen_{}_{nanos}", std::process::id());
    let (base, _) = admin_url.rsplit_once('/').unwrap();
    let url = format!("{base}/{db}");
    assert!(
        psql(&admin_url, &format!("CREATE DATABASE {db}"))
            .unwrap()
            .success()
    );
    let tmp = std::env::temp_dir();
    let chart = tmp.join(format!("{db}.json"));
    let report = tmp.join(format!("{db}-report.json"));
    let mut guard = Cleanup {
        server: None,
        admin_url,
        db,
        files: vec![chart.clone(), report.clone()],
    };
    let document = cavea_testkit::presets::find("theatre")
        .unwrap()
        .generate(1)
        .to_json()
        .unwrap();
    std::fs::write(&chart, document).unwrap();

    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    guard.server = Some(start(&url, port));

    // A stampede on 2,000 seats, then sustained arrivals with blocks: correct
    // (a debug server may miss the budgets: exit 4), everything checked.
    for shape in [
        "--subscribers 50 --buyers 200 --ramp-ms 1000 --quiet-ms 500",
        "--profile l2 --subscribers 20 --rate 50 --duration-s 2 --blocks-per-sec 5 --quiet-ms 500",
    ] {
        let (code, r) = loadgen(port, &chart, &report, shape);
        assert!(
            code == 0 || code == 4,
            "{shape}: exit {code}: {:#}",
            r["checks"]
        );
        assert_eq!(r["checks"]["violations_total"], 0, "{:#}", r["checks"]);
        assert_eq!(r["checks"]["convergence"]["differ"], 0);
        assert!(r["checks"]["convergence"]["equal"].as_u64().unwrap() > 0);
        assert!(
            r["ops"]["hold"]["ok"].as_u64().unwrap() > 50,
            "{:#}",
            r["ops"]
        );
        assert!(
            r["ops"]["book"]["ok"].as_u64().unwrap() > 0,
            "{:#}",
            r["ops"]
        );
        assert!(r["delta_delivery"]["count"].as_u64().unwrap() > 0);
        assert!(
            !r.to_string().contains("pk_") && !r.to_string().contains("sk_"),
            "a key in the report"
        );
    }

    // The server killed (SIGKILL) during a sustained run and started again on
    // the same port: the generator reconnects its subscribers, retries what
    // met the outage, resolves what it could not, and finds nothing wrong.
    let shape = "--profile l2 --subscribers 20 --rate 100 --duration-s 8 --blocks-per-sec 5 --quiet-ms 3000";
    let run = generator(port, &chart, &report, shape);
    std::thread::sleep(Duration::from_secs(3));
    if let Some(mut s) = guard.server.take() {
        s.kill().unwrap();
        s.wait().unwrap();
    }
    std::thread::sleep(Duration::from_millis(500));
    guard.server = Some(start(&url, port));
    let (code, r) = finish(run, &report);
    assert!(code == 0 || code == 4, "exit {code}: {:#}", r["checks"]);
    assert_eq!(r["checks"]["violations_total"], 0, "{:#}", r["checks"]);
    let cv = &r["checks"]["convergence"];
    assert_eq!((&cv["differ"], &cv["unverified"]), (&0.into(), &0.into()), "{cv:#}");
    assert!(cv["equal"].as_u64().unwrap() > 0);
    assert!(r["connections"]["reconnects"].as_u64().unwrap() > 0, "{:#}", r["connections"]);
    assert!(!r["outages"].as_array().unwrap().is_empty());

    let verify = Command::new(server_binary())
        .args(["verify", "--all"])
        .env("CAVEA_DATABASE_URL", &url)
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&verify.stdout);
    assert_eq!(verify.status.code(), Some(0), "{out}");
    assert!(out.contains("3 event(s) verified: clean"), "{out}");
}
