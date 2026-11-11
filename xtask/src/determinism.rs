//! `cargo xtask determinism` (`docs/13-testing-benchmarks.md` §4): the tests
//! that pin compiled hashes, run natively and in wasm32 under bun. Both legs
//! run the same release test binaries of the crates in [`PINNED`]; the command
//! passes only if both legs ran and passed.
//!
//! A wasm32-unknown-unknown test binary has no output and no arguments: it runs
//! every test that is not ignored, returns 0 from `main` when all pass, and
//! traps on the first failure. So each binary is first checked to contain the
//! pinned tests, and the leg fails if `bun` or the target is missing.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context as _, Result, bail, ensure};

use crate::{WASM_TARGET, cargo, root, run};

/// The crates whose library tests pin compiled hashes, and those tests.
const PINNED: [(&str, &[&str]); 2] = [
    (
        "cavea_compiler",
        &["compile::tests::compiled_bytes_are_deterministic"],
    ),
    (
        "cavea_testkit",
        &[
            "presets::tests::theatre",
            "presets::tests::arena",
            "presets::tests::stadium_60k",
            "presets::tests::stadium_100k",
            "presets::tests::stress_250k",
            "presets::tests::stress_500k",
        ],
    ),
];

/// Instantiates a wasm32 test binary with no imports and calls its `main`.
const BUN_RUNNER: &str = "const path = process.argv.at(-1); \
    const module = new WebAssembly.Module(await Bun.file(path).arrayBuffer()); \
    const code = new WebAssembly.Instance(module).exports.main(0, 0); \
    if (code !== 0) { console.error(`main returned ${code}`); process.exit(1); }";

pub fn determinism() -> Result<()> {
    let native = test_binaries(None).and_then(|bins| {
        check_pinned(&bins)?;
        bins.iter()
            .try_for_each(|(_, exe)| run(&mut Command::new(exe)))
    });
    let wasm = test_binaries(Some(WASM_TARGET)).and_then(|bins| wasm_leg("bun", &bins));
    let tests: usize = PINNED.iter().map(|(_, names)| names.len()).sum();
    println!("\ndeterminism ({tests} pinned-hash tests, release builds):");
    for (leg, result) in [("native", &native), ("wasm32 under bun", &wasm)] {
        match result {
            Ok(()) => println!("  {leg:<18}passed"),
            Err(e) => println!("  {leg:<18}FAILED: {e:#}"),
        }
    }
    ensure!(
        native.is_ok() && wasm.is_ok(),
        "the compiled bytes are not shown identical on both legs"
    );
    println!("native and wasm32 compile the pinned documents to the pinned bytes");
    Ok(())
}

/// Builds the release library tests of the [`PINNED`] crates for `target`
/// (the host for `None`) and returns each crate's test binary.
fn test_binaries(target: Option<&str>) -> Result<Vec<(String, PathBuf)>> {
    let mut build = cargo();
    build.current_dir(root()).args([
        "test",
        "--release",
        "--lib",
        "--no-run",
        "--message-format=json",
    ]);
    for (krate, _) in PINNED {
        build.args(["--package", &krate.replace('_', "-")]);
    }
    if let Some(target) = target {
        build.args(["--target", target]);
    }
    let output = build
        .stderr(std::process::Stdio::inherit())
        .output()
        .context("cannot run cargo")?;
    ensure!(output.status.success(), "building the tests failed");
    let mut bins = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let name = message["target"]["name"].as_str();
        if let (Some(name), Some(exe)) = (name, message["executable"].as_str()) {
            bins.push((name.to_owned(), PathBuf::from(exe)));
        }
    }
    Ok(bins)
}

/// Fails unless every [`PINNED`] crate has exactly one binary, and that
/// binary contains its pinned tests.
fn check_pinned(bins: &[(String, PathBuf)]) -> Result<()> {
    for (krate, tests) in PINNED {
        let exes: Vec<&Path> = bins
            .iter()
            .filter(|(name, _)| name == krate)
            .map(|(_, exe)| exe.as_path())
            .collect();
        let [exe] = exes.as_slice() else {
            bail!("expected one test binary of {krate}, found {}", exes.len());
        };
        let bytes = fs::read(exe).with_context(|| format!("cannot read {}", exe.display()))?;
        for test in tests.iter() {
            let found = bytes.windows(test.len()).any(|w| w == test.as_bytes());
            ensure!(found, "{} does not contain the test {test}", exe.display());
        }
    }
    Ok(())
}

/// Runs each wasm32 test binary under `bun`.
fn wasm_leg(bun: &str, bins: &[(String, PathBuf)]) -> Result<()> {
    check_pinned(bins)?;
    let version = Command::new(bun).arg("--version").output();
    ensure!(
        version.is_ok_and(|v| v.status.success()),
        "cannot run {bun}: the wasm32 leg needs bun (`nix develop` provides it)"
    );
    for (_, wasm) in bins {
        let status = Command::new(bun)
            .args(["-e", BUN_RUNNER])
            .arg(wasm)
            .status();
        ensure!(
            status.is_ok_and(|s| s.success()),
            "{} failed under bun: a test failed (it traps) or bun could not run it",
            wasm.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::env;

    use super::*;

    /// A file holding `contents`, in a directory of its own.
    fn file(name: &str, contents: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("xtask-determinism-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, contents).unwrap();
        path
    }

    /// The wasm32 leg refuses to pass when it cannot run what it must.
    #[test]
    fn wasm_leg_cannot_pass_without_running() {
        let every = |krate: &str| {
            let names = PINNED.iter().find(|(k, _)| *k == krate).unwrap().1;
            file(krate, &names.join("\n"))
        };
        let both = vec![
            ("cavea_compiler".to_owned(), every("cavea_compiler")),
            ("cavea_testkit".to_owned(), every("cavea_testkit")),
        ];
        let err =
            |bun: &str, bins: &[(String, PathBuf)]| wasm_leg(bun, bins).unwrap_err().to_string();
        assert!(err("bun", &[]).contains("expected one test binary"));
        assert!(err("bun", &both[..1]).contains("cavea_testkit"));
        // A binary without the pinned tests (renamed, removed, or a crate
        // built with none) is refused before anything runs.
        let empty = vec![
            both[0].clone(),
            (
                "cavea_testkit".to_owned(),
                file("empty", "presets::tests::theatre"),
            ),
        ];
        assert!(err("bun", &empty).contains("presets::tests::arena"));
        assert!(err("/nonexistent/bun", &both).contains("needs bun"));
    }
}
