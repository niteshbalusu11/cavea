//! Build orchestration for the workspace, run as `cargo xtask <task>`.
//!
//! - `build-web`: build `cavea-web` for `wasm32-unknown-unknown` with the
//!   size-tuned `wasm-release` profile, generate the JavaScript and TypeScript
//!   bindings into `packages/renderer/wasm/`, run `wasm-opt` when it is on `PATH`,
//!   and print raw and brotli sizes. `--demo` adds the debug-only exports used
//!   by `apps/demo` (cargo feature `cavea-web/demo`); its sizes are not the
//!   production payload.
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

fn main() -> Result<()> {
    let task = env::args().nth(1);
    match task.as_deref() {
        Some("build-web") => build_web(env::args().nth(2).as_deref() == Some("--demo")),
        Some("check-deps") => check_deps(),
        _ => {
            eprintln!("usage: cargo xtask <build-web [--demo] | check-deps>");
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

fn build_web(demo: bool) -> Result<()> {
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
    wasm_opt(&wasm)?;
    size_report(&out_dir)?;
    if demo {
        println!("demo build: includes the synthetic generator; not the production payload");
    }
    Ok(())
}

/// Runs `wasm-opt -Oz` in place when binaryen is installed; otherwise warns.
fn wasm_opt(wasm: &Path) -> Result<()> {
    let probe = Command::new("wasm-opt").arg("--version").output();
    if !probe.is_ok_and(|output| output.status.success()) {
        eprintln!(
            "warning: wasm-opt not found on PATH; sizes below are not optimised (install binaryen)"
        );
        return Ok(());
    }
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
        .arg(wasm))
}

fn size_report(dir: &Path) -> Result<()> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    files.retain(|path| {
        path.extension()
            .is_some_and(|ext| ext == "wasm" || ext == "js")
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
    Ok(())
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
