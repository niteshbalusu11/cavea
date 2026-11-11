//! Build orchestration for the workspace, run as `cargo xtask <task>`.
//!
//! - `build-web`: build `cavea-web` for `wasm32-unknown-unknown` with the
//!   size-tuned `wasm-release` profile, generate the JavaScript and TypeScript
//!   bindings into `packages/renderer/wasm/`, run `wasm-opt` when it is on `PATH`,
//!   print raw and brotli sizes, and enforce the payload gate
//!   (`docs/13-testing-benchmarks.md` §6.3) against [`BASELINE`].
//!   `--update-baseline` records the measured size as the new baseline.
//!   `--demo` adds the debug-only exports used by `apps/demo` (cargo feature
//!   `cavea-web/demo`); it is not the production payload and is never gated.
//! - `check-deps`: enforce the workspace dependency rule from
//!   `docs/01-architecture.md` §3.

use std::{
    collections::BTreeSet,
    env, fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context as _, Result, bail, ensure};

const WASM_TARGET: &str = "wasm32-unknown-unknown";
const WASM_PROFILE: &str = "wasm-release";

/// The last accepted renderer payload, relative to the workspace root.
const BASELINE: &str = "xtask/payload-baseline.json";
/// The hard cap on the renderer payload, brotli (`docs/01-architecture.md` §5).
const PAYLOAD_CAP: usize = 200_000;
/// Growth over the baseline, in percent, that fails the build.
const MAX_GROWTH_PERCENT: usize = 3;
/// Files of the payload, by extension: the JavaScript glue and the WASM. The
/// font atlas joins them when the renderer has one.
const PAYLOAD: [&str; 2] = ["js", "wasm"];

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// Measure and gate.
    Gate,
    /// Measure, check the cap, and record the size as the new baseline.
    UpdateBaseline,
    /// The demo build: not measured against anything.
    Demo,
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["build-web"] => build_web(Mode::Gate),
        ["build-web", "--update-baseline"] => build_web(Mode::UpdateBaseline),
        ["build-web", "--demo"] => build_web(Mode::Demo),
        ["check-deps"] => check_deps(),
        _ => {
            eprintln!("usage: cargo xtask <build-web [--demo | --update-baseline] | check-deps>");
            std::process::exit(2);
        }
    }
}

fn root() -> PathBuf {
    // xtask/ sits directly under the workspace root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

fn cargo() -> Command {
    Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

fn run(command: &mut Command) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("cannot run {command:?}"))?;
    ensure!(status.success(), "{command:?} failed with {status}");
    Ok(())
}

fn build_web(mode: Mode) -> Result<()> {
    let demo = mode == Mode::Demo;
    let root = root();
    let mut build = cargo();
    build.current_dir(&root).args([
        "build",
        "--package",
        "cavea-web",
        "--lib",
        "--target",
        WASM_TARGET,
        "--profile",
        WASM_PROFILE,
    ]);
    if demo {
        build.args(["--features", "demo"]);
    }
    run(&mut build)?;

    let target_dir =
        env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    let input = target_dir
        .join(WASM_TARGET)
        .join(WASM_PROFILE)
        .join("cavea_web.wasm");
    let out_dir = root.join("packages/renderer/wasm");
    if out_dir.exists() {
        fs::remove_dir_all(&out_dir)
            .with_context(|| format!("cannot clear {}", out_dir.display()))?;
    }
    fs::create_dir_all(&out_dir)?;

    wasm_bindgen_cli_support::Bindgen::new()
        .input_path(&input)
        .web(true)?
        .typescript(true)
        .generate(&out_dir)
        .context("wasm-bindgen failed")?;

    let wasm = out_dir.join("cavea_web_bg.wasm");
    let wasm_opt_version = wasm_opt(&wasm)?;
    let measured = size_report(&out_dir)?;
    if demo {
        println!("demo build: includes the synthetic generator; not the production payload");
        return Ok(());
    }

    let path = root.join(BASELINE);
    let baseline = read_baseline(&path)?;
    let version = wasm_opt_version.as_deref();
    if mode == Mode::UpdateBaseline {
        let fresh = Baseline {
            brotli: measured,
            wasm_opt: version.unwrap_or_default().to_owned(),
        };
        // The new baseline must pass the gate it sets, cap and version included.
        let checked = gate(measured, version, &fresh, PAYLOAD_CAP).map_err(anyhow::Error::msg)?;
        fs::write(&path, fresh.to_json()?)
            .with_context(|| format!("cannot write {}", path.display()))?;
        println!(
            "{checked}\nbaseline updated: {BASELINE} was {} B, now {measured} B",
            baseline.brotli
        );
        return Ok(());
    }
    match gate(measured, version, &baseline, PAYLOAD_CAP) {
        Ok(report) => {
            println!("{report}");
            Ok(())
        }
        Err(problem) => bail!("payload gate: {problem}"),
    }
}

/// The last accepted payload (`xtask/payload-baseline.json`).
#[derive(Debug)]
struct Baseline {
    /// Brotli bytes of the payload files together.
    brotli: usize,
    /// The `wasm-opt` version it was measured with.
    wasm_opt: String,
}

impl Baseline {
    fn to_json(&self) -> Result<String> {
        let value = serde_json::json!({ "brotliBytes": self.brotli, "wasmOpt": self.wasm_opt });
        Ok(serde_json::to_string_pretty(&value)? + "\n")
    }
}

fn read_baseline(path: &Path) -> Result<Baseline> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("{} is not JSON", path.display()))?;
    let brotli = value["brotliBytes"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok());
    let wasm_opt = value["wasmOpt"].as_str();
    match (brotli, wasm_opt) {
        (Some(brotli), Some(wasm_opt)) => Ok(Baseline {
            brotli,
            wasm_opt: wasm_opt.to_owned(),
        }),
        _ => bail!("{} needs brotliBytes and wasmOpt", path.display()),
    }
}

/// The payload gate (`docs/13-testing-benchmarks.md` §6.3): `Ok` with a
/// report, or `Err` saying why the build fails. Only a build optimised by the
/// baseline's `wasm-opt` version is compared: without `wasm-opt` the WASM is
/// about a fifth larger, and other versions give other sizes.
fn gate(
    measured: usize,
    wasm_opt: Option<&str>,
    baseline: &Baseline,
    cap: usize,
) -> Result<String, String> {
    let Some(version) = wasm_opt else {
        return Err(format!(
            "wasm-opt is not on PATH, so the WASM is not optimised and cannot be compared \
             with the baseline; install binaryen {} (`nix develop` provides it)",
            baseline.wasm_opt
        ));
    };
    if version != baseline.wasm_opt {
        return Err(format!(
            "wasm-opt version {version}, but the baseline was measured with version {}; sizes \
             from different versions are not comparable. Use binaryen {} (flake.nix and CI \
             pin it), or upgrade both and refresh the baseline with \
             `cargo xtask build-web --update-baseline`",
            baseline.wasm_opt, baseline.wasm_opt
        ));
    }
    if measured > cap {
        return Err(format!(
            "{measured} B brotli is over the hard cap of {cap} B (docs/01-architecture.md §5)"
        ));
    }
    let base = baseline.brotli;
    let change = (measured as f64 / base.max(1) as f64 - 1.0) * 100.0;
    if measured * 100 > base * (100 + MAX_GROWTH_PERCENT) {
        return Err(format!(
            "{measured} B brotli is {change:+.1}% over the baseline of {base} B in {BASELINE}, \
             more than +{MAX_GROWTH_PERCENT}%. If the growth is intended, run \
             `cargo xtask build-web --update-baseline` and commit {BASELINE} in the same pull \
             request, so that the new size is reviewed"
        ));
    }
    let mut report = format!(
        "payload gate passed: {measured} B brotli, {change:+.1}% against the baseline of {base} B \
         (fails above +{MAX_GROWTH_PERCENT}% or {cap} B); wasm-opt {version}"
    );
    if measured * 100 < base * (100 - MAX_GROWTH_PERCENT) {
        report += &format!(
            "\nthe payload is more than {MAX_GROWTH_PERCENT}% below the baseline: refresh it with \
             `cargo xtask build-web --update-baseline` so that later growth is measured from here"
        );
    }
    Ok(report)
}

/// Runs `wasm-opt -Oz` in place when binaryen is installed and returns its
/// version (`132`); otherwise warns and returns `None`.
fn wasm_opt(wasm: &Path) -> Result<Option<String>> {
    let probe = Command::new("wasm-opt").arg("--version").output();
    let Some(version) = probe
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| wasm_opt_version(&String::from_utf8_lossy(&output.stdout)))
    else {
        eprintln!(
            "warning: wasm-opt not found on PATH; sizes below are not optimised (install binaryen)"
        );
        return Ok(None);
    };
    // These features are on by default for Rust's wasm32-unknown-unknown target.
    run(Command::new("wasm-opt")
        .arg("-Oz")
        .args([
            "--enable-bulk-memory",
            "--enable-mutable-globals",
            "--enable-nontrapping-float-to-int",
            "--enable-sign-ext",
            "--enable-reference-types",
            "--enable-multivalue",
        ])
        .arg(wasm)
        .arg("-o")
        .arg(wasm))?;
    Ok(Some(version))
}

/// The version number in `wasm-opt --version` output: `132` from both
/// `wasm-opt version 132` and `wasm-opt version 132 (version_132)`.
fn wasm_opt_version(output: &str) -> Option<String> {
    let mut words = output.split_whitespace();
    words.find(|&w| w == "version")?;
    words.next().map(str::to_owned)
}

/// Prints the size of each payload file and returns their brotli total.
fn size_report(dir: &Path) -> Result<usize> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    files.retain(|path| {
        path.extension()
            .is_some_and(|ext| PAYLOAD.iter().any(|p| ext == *p))
    });
    files.sort();

    println!("\n{:<28} {:>10} {:>10}", "payload", "raw", "brotli");
    let (mut total_raw, mut total_br) = (0, 0);
    for path in &files {
        let bytes = fs::read(path)?;
        let br = brotli_len(&bytes)?;
        total_raw += bytes.len();
        total_br += br;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        println!("{name:<28} {:>10} {:>10}", bytes.len(), br);
    }
    println!("{:<28} {total_raw:>10} {total_br:>10}", "total (js + wasm)");
    println!("bytes; brotli at quality 11. Output: {}", dir.display());
    Ok(total_br)
}

fn brotli_len(bytes: &[u8]) -> Result<usize> {
    let mut out = Vec::new();
    {
        let mut writer = brotli::CompressorWriter::new(&mut out, 4096, 11, 22);
        writer.write_all(bytes)?;
    }
    Ok(out.len())
}

/// `cavea-core` and `cavea-format` may depend on no workspace crate except each
/// other, `cavea-telemetry` on none, and nothing may depend on `cavea-server`
/// (`docs/01-architecture.md` §3).
fn check_deps() -> Result<()> {
    let output = cargo()
        .current_dir(root())
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .context("cannot run cargo metadata")?;
    ensure!(output.status.success(), "cargo metadata failed");
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let packages = metadata["packages"]
        .as_array()
        .context("no packages in metadata")?;
    let members: BTreeSet<&str> = packages.iter().filter_map(|p| p["name"].as_str()).collect();

    let mut violations = Vec::new();
    for package in packages {
        let name = package["name"].as_str().unwrap_or_default();
        let deps = package["dependencies"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        for dep in deps.iter().filter_map(|d| d["name"].as_str()) {
            if !members.contains(dep) {
                continue;
            }
            let allowed = match name {
                "cavea-core" => dep == "cavea-format",
                "cavea-format" => dep == "cavea-core",
                "cavea-telemetry" => false,
                _ => dep != "cavea-server",
            };
            if !allowed {
                violations.push(format!("{name} -> {dep}"));
            }
        }
    }
    if !violations.is_empty() {
        bail!(
            "dependency rule broken (docs/01-architecture.md §3): {}",
            violations.join(", ")
        );
    }
    println!(
        "dependency rule holds for {} workspace crates",
        members.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline() -> Baseline {
        Baseline {
            brotli: 100_000,
            wasm_opt: "132".into(),
        }
    }

    #[test]
    fn gate_decides_on_cap_growth_and_wasm_opt() {
        let pass = |m| gate(m, Some("132"), &baseline(), PAYLOAD_CAP).is_ok();
        assert!(pass(100_000));
        assert!(pass(103_000), "+3% exactly passes");
        assert!(!pass(103_001), "beyond +3% fails");
        assert!(pass(50_000), "a decrease passes");
        // The cap holds even when the baseline is above it.
        let high = Baseline {
            brotli: 199_000,
            ..baseline()
        };
        assert!(gate(200_000, Some("132"), &high, PAYLOAD_CAP).is_ok());
        assert!(gate(200_001, Some("132"), &high, PAYLOAD_CAP).is_err());
        // No wasm-opt, or another version: refused, whatever the size.
        assert!(gate(100_000, None, &baseline(), PAYLOAD_CAP).is_err());
        assert!(gate(100_000, Some("131"), &baseline(), PAYLOAD_CAP).is_err());
    }

    #[test]
    fn reads_the_wasm_opt_version() {
        let v = |s| wasm_opt_version(s);
        assert_eq!(v("wasm-opt version 132\n").as_deref(), Some("132"));
        assert_eq!(
            v("wasm-opt version 132 (version_132)").as_deref(),
            Some("132")
        );
        assert_eq!(v("wasm-opt"), None);
    }
}
