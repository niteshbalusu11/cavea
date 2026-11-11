//! Writes the committed seed corpora, `corpus/<target>/seed-*`. Run after a
//! change to the chart format, the wire protocol or the document schema:
//!
//! ```sh
//! nix develop .#fuzz -c cargo run --manifest-path fuzz/Cargo.toml --example seeds
//! ```
//!
//! Seeds are small on purpose: libFuzzer mutates within the largest seed's
//! length, and short inputs run fast.

use std::fs;
use std::path::Path;

use cavea_compiler::ChartDocument;
use cavea_format::wire::{self, Area, Change, CloseReason, Hello, Subscribe, View};
use cavea_fuzz::Op;

fn write(target: &str, name: &str, bytes: &[u8]) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("corpus")
        .join(target);
    fs::create_dir_all(&dir).expect("corpus directory");
    fs::write(dir.join(format!("seed-{name}")), bytes).expect("seed written");
    println!("{target}/seed-{name}: {} B", bytes.len());
}

fn compiled(doc: &ChartDocument) -> cavea_compiler::Compiled {
    cavea_compiler::compile(doc).expect("seed document compiles")
}

fn main() {
    let samples =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/cavea-compiler/samples/schema-1");
    let mut docs: Vec<(&str, Vec<u8>)> = ["small", "full"]
        .into_iter()
        .map(|name| {
            (
                name,
                fs::read(samples.join(format!("{name}.json"))).expect("sample"),
            )
        })
        .collect();
    docs.push(("fuzz-chart", cavea_fuzz::CHART.as_bytes().to_vec()));
    // The synthetic stadium generator, trimmed to 240 seats (10 per section).
    let stadium = cavea_testkit::synthetic::stadium(240, 1);
    docs.push(("stadium-240", stadium.to_json().expect("serialises")));

    for (name, json) in &docs {
        // The stadium's JSON (14 KB) would raise libFuzzer's input length
        // for little new coverage: its compiled form seeds `ccf_decode` only.
        if *name != "stadium-240" {
            write("document_parse", name, json);
        }
        let c = compiled(&ChartDocument::from_json(json).expect("sample parses"));
        write("ccf_decode", &format!("{name}-public"), &c.public);
        write("ccf_decode", &format!("{name}-full"), &c.full);
    }
    // Refusals the parser must give cleanly.
    write(
        "document_parse",
        "schema-2",
        br#"{"schema":2,"name":"x","nodes":[]}"#,
    );
    write(
        "document_parse",
        "unknown-field",
        br#"{"schema":1,"name":"x","nodes":[],"extra":1}"#,
    );
    write(
        "document_parse",
        "deep",
        format!("{}{}", "[".repeat(200), "]".repeat(200)).as_bytes(),
    );
    // A header with nothing behind it.
    let c = compiled(&ChartDocument::from_json(&docs[0].1).expect("parses"));
    write(
        "ccf_decode",
        "header-only",
        &c.public[..cavea_format::HEADER_LEN],
    );

    // One valid frame of each type, both views where they differ.
    let statuses: Vec<u8> = (0..102u32)
        .map(|i| [0, 0, 2, 1, 0, 7][i as usize % 6])
        .collect();
    let blocks: Vec<u8> = (0..102u32)
        .map(|i| u8::from((40..70).contains(&i)) * 2)
        .collect();
    let changes = [
        Change { idx: 3, code: 2 },
        Change { idx: 50, code: 0 },
        Change { idx: 101, code: 9 },
    ];
    let areas = [Area { idx: 101, value: 4 }];
    let frames: Vec<(&str, Vec<u8>)> = vec![
        (
            "hello",
            wire::encode_hello(&Hello {
                server_time_ms: 1_762_000_000_000,
                chart_hash: [7; 16],
                objects: 102,
            })
            .unwrap(),
        ),
        (
            "snapshot-bits",
            wire::encode_snapshot(View::Public, 9, &statuses, &[]).unwrap(),
        ),
        (
            "snapshot-runs",
            wire::encode_snapshot(View::Public, 9, &blocks, &areas).unwrap(),
        ),
        (
            "snapshot-raw",
            wire::encode_snapshot(View::Manager, 9, &statuses, &[]).unwrap(),
        ),
        (
            "delta-public",
            wire::encode_delta(View::Public, 9, 10, &changes, 102, &[]).unwrap(),
        ),
        (
            "delta-manager",
            wire::encode_delta(View::Manager, 9, 10, &changes, 102, &areas).unwrap(),
        ),
        (
            "private",
            wire::encode_private(1_762_000_600_000, &[3, 4, 5], 102, &areas).unwrap(),
        ),
        ("resync", wire::encode_resync()),
        ("ping", wire::encode_ping(42)),
        ("close", wire::encode_close(CloseReason::CHART_UPGRADED)),
        (
            "subscribe",
            wire::encode_subscribe(&Subscribe {
                version: 1,
                event_key: "concert-2026-11-11",
                last_seq: 9,
                hold_token: Some(b"hold-token"),
                session_token: None,
            })
            .unwrap(),
        ),
        ("pong", wire::encode_pong(42)),
    ];
    for (name, frame) in &frames {
        write("wire_decode", name, frame);
    }

    // The engine: state before and after the chart, selection and holds.
    let objects = 102;
    let free = vec![0u8; objects];
    let some: Vec<u8> = (0..objects).map(|i| u8::from(i % 5 == 0)).collect();
    let snapshot =
        |seq, state: &[u8]| wire::encode_snapshot(View::Public, seq, state, &[]).unwrap();
    let delta = |from, idx: &[u32]| {
        let changes: Vec<Change> = idx.iter().map(|&idx| Change { idx, code: 1 }).collect();
        wire::encode_delta(View::Public, from, from + 1, &changes, objects as u32, &[]).unwrap()
    };
    let private =
        |idx: &[u32]| wire::encode_private(1_762_000_600_000, idx, objects as u32, &[]).unwrap();
    let sequences: Vec<(&str, Vec<Op>)> = vec![
        (
            "state",
            vec![
                Op::Load,
                Op::Tick,
                Op::Frame(snapshot(1, &some)),
                Op::Tick,
                Op::Frame(delta(1, &[1, 2, 3, 60, 101])),
                Op::Tick,
                Op::Frame(delta(5, &[7])),
                Op::Frame(snapshot(6, &free)),
                Op::Tick,
            ],
        ),
        (
            "early-state",
            vec![
                Op::Frame(snapshot(3, &some)),
                Op::Frame(private(&[1, 2])),
                Op::Load,
                Op::Tick,
            ],
        ),
        (
            "hold",
            vec![
                Op::Load,
                Op::Frame(snapshot(1, &free)),
                Op::Tick,
                Op::Select(4),
                Op::Select(5),
                Op::Tick,
                Op::HoldResult {
                    request: None,
                    failure: None,
                },
                Op::Frame(delta(1, &[4, 5, 6])),
                Op::Frame(private(&[4, 5])),
                Op::Tick,
                Op::Deselect(5),
                Op::Tick,
                Op::Frame(private(&[4])),
                Op::Clear,
                Op::Tick,
            ],
        ),
        (
            "refused",
            vec![
                Op::Load,
                Op::Frame(snapshot(1, &free)),
                Op::Select(9),
                Op::Select(10),
                Op::Tick,
                Op::HoldResult {
                    request: None,
                    failure: Some((10, "not_free")),
                },
                Op::Tick,
                Op::Select(11),
                Op::HoldResult {
                    request: None,
                    failure: Some((11, "not_found")),
                },
                Op::Manual(true),
                Op::Select(12),
                Op::Tick,
                Op::Load,
                Op::Tick,
            ],
        ),
    ];
    for (name, ops) in &sequences {
        let bytes = cavea_fuzz::encode(ops);
        assert_eq!(&cavea_fuzz::ops(&bytes), ops, "op encoding round trips");
        cavea_fuzz::run(ops);
        write("snapshot_apply", name, &bytes);
    }
}
