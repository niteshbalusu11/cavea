//! The `cavea` binary on good and bad input: exit codes, output, files.

#![cfg(test)]

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use cavea_testkit::presets::{DEFAULT_SEED, PRESETS, WAITING, find};

/// Runs `cavea` with `args`, feeding `stdin`.
fn cavea(args: &[&str], stdin: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cavea"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    child.wait_with_output().unwrap()
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

/// A fresh directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The number on the line of `output` that starts with `name`.
fn count(output: &str, name: &str) -> u64 {
    let line = output
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with(name) && l[name.len()..].starts_with("  "))
        .unwrap_or_else(|| panic!("no {name:?} in\n{output}"));
    line[name.len()..].trim().parse().unwrap()
}

#[test]
fn gen_lists_and_writes_presets() {
    let list = cavea(&["gen", "--list"], b"");
    assert_eq!(code(&list), 0);
    for preset in &PRESETS {
        assert!(stdout(&list).contains(preset.name));
    }
    for (name, _) in WAITING {
        assert!(stdout(&list).contains(name));
    }

    let theatre = find("theatre").unwrap();
    let json = theatre.generate(DEFAULT_SEED).to_json().unwrap();
    let out = cavea(&["gen", "theatre"], b"");
    assert_eq!(code(&out), 0);
    assert_eq!(out.stdout.strip_suffix(b"\n"), Some(json.as_slice()));
    let seeded = cavea(&["gen", "theatre", "--seed", "9"], b"");
    assert_eq!(
        seeded.stdout,
        cavea(&["gen", "theatre", "--seed", "9"], b"").stdout
    );
    assert_ne!(seeded.stdout, out.stdout);

    for bad in [
        &["gen", "nope"][..],
        &["gen", "banquet"],
        &["gen"],
        &["gen", "theatre", "--seed", "x"],
        &["gen", "theatre", "--colour"],
        &[],
        &["frobnicate"],
    ] {
        assert_eq!(code(&cavea(bad, b"")), 2, "{bad:?}");
    }
}

#[test]
fn compile_then_inspect_round_trips_counts() {
    let dir = scratch("round-trip");
    let json = cavea(&["gen", "theatre"], b"").stdout;
    let compiled = cavea(&["compile", "-o", dir.to_str().unwrap()], &json);
    assert_eq!(code(&compiled), 0, "{}", stderr(&compiled));
    let compiled = stdout(&compiled);

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    assert_eq!(files.len(), 2);
    for file in &files {
        let bytes = std::fs::read(file).unwrap();
        let name = file.file_name().unwrap().to_str().unwrap();
        let full = name.ends_with(".full.ccf");
        let hash: String = bytes[16..32].iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(name.split('.').next(), Some(hash.as_str()));

        let inspected = cavea(&["inspect", file.to_str().unwrap()], b"");
        assert_eq!(code(&inspected), 0, "{}", stderr(&inspected));
        let inspected = stdout(&inspected);
        assert!(inspected.starts_with(if full { "full" } else { "public" }));
        assert!(inspected.contains(&hash));
        for name in ["seats", "rows", "segments", "sections", "floors"] {
            assert_eq!(count(&compiled, name), count(&inspected, name), "{name}");
        }
        assert_eq!(
            count(&compiled, "chunks"),
            count(&inspected, "render chunks")
        );
        assert_eq!(count(&inspected, "seats"), 2_000);
    }
}

#[test]
fn documents_with_errors_are_reported_and_not_compiled() {
    let mut doc = find("theatre").unwrap().generate(DEFAULT_SEED);
    doc.categories[0].color = "blue".into();
    let json = doc.to_json().unwrap();

    let validated = cavea(&["validate"], &json);
    assert_eq!(code(&validated), 1);
    assert!(stdout(&validated).contains("error E005"));
    let as_json = cavea(&["validate", "--json", "-"], &json);
    assert_eq!(code(&as_json), 1);
    let findings: serde_json::Value = serde_json::from_slice(&as_json.stdout).unwrap();
    assert_eq!(findings[0]["code"], "E005");
    assert_eq!(findings[0]["severity"], "error");

    let dir = scratch("errors");
    let compiled = cavea(&["compile", "-o", dir.to_str().unwrap()], &json);
    assert_eq!(code(&compiled), 1);
    assert!(stderr(&compiled).contains("E005"));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);

    // A clean document validates with no findings at all.
    let clean = cavea(
        &["validate", "--json"],
        &cavea(&["gen", "theatre"], b"").stdout,
    );
    assert_eq!(code(&clean), 0);
    assert_eq!(stdout(&clean).trim(), "[]");

    assert_eq!(code(&cavea(&["validate"], b"{ not json")), 1);
    assert_eq!(
        code(&cavea(
            &["compile", "-o", dir.to_str().unwrap()],
            b"{ not json"
        )),
        1
    );
    assert_eq!(code(&cavea(&["validate", "/nonexistent/doc.json"], b"")), 3);
}

#[test]
fn corrupt_files_fail_inspect_without_a_panic() {
    let dir = scratch("corrupt");
    let json = cavea(&["gen", "theatre"], b"").stdout;
    assert_eq!(
        code(&cavea(&["compile", "-o", dir.to_str().unwrap()], &json)),
        0
    );
    let file = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| !p.to_str().unwrap().ends_with(".full.ccf"))
        .unwrap();
    let bytes = std::fs::read(&file).unwrap();

    let mut cases = vec![
        Vec::new(),
        bytes[..100].to_vec(),
        b"CCF1 not a chart".to_vec(),
    ];
    // A directory offset pointing past the end of the file.
    let mut bad_offset = bytes.clone();
    bad_offset[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
    cases.push(bad_offset);
    for (i, case) in cases.iter().enumerate() {
        let path = dir.join(format!("bad-{i}.ccf"));
        std::fs::write(&path, case).unwrap();
        let out = cavea(&["inspect", path.to_str().unwrap()], b"");
        assert_eq!(code(&out), 1, "case {i}: {}", stderr(&out));
        assert!(stderr(&out).contains("CCF"), "case {i}: {}", stderr(&out));
    }
    assert_eq!(code(&cavea(&["inspect"], b"")), 2);
}

/// A failed write to standard output (here a full disk) is exit 3, not a panic.
#[test]
fn output_failures_exit_3() {
    let Ok(full) = std::fs::OpenOptions::new().write(true).open("/dev/full") else {
        return; // Not Linux.
    };
    let json = cavea(&["gen", "theatre"], b"").stdout;
    for (args, stdin) in [
        (&["gen", "--list"][..], &b""[..]),
        (&["gen", "theatre"], b""),
        (&["validate"], &json),
        (&["--help"], b""),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_cavea"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(full.try_clone().unwrap())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(code(&output), 3, "{args:?}: {}", stderr(&output));
        assert!(stderr(&output).contains("cannot write standard output"));
    }
}
