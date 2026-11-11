//! `cargo xtask build-server [--target <triple>]`: the server with the console
//! compiled in (09 §9, CON-015). Builds the WebAssembly modules and the
//! console (`bun run build` in `apps/console`; `bun install` first is the
//! caller's), writes every file of `apps/console/dist` and the brotli form of
//! each that it shrinks into `target/console/`, with `table.rs` listing them
//! by `include_bytes!`, then builds `cavea-server` in release with
//! `CAVEA_CONSOLE_EMBED` naming the table (`crates/cavea-server/build.rs`).
//! The table is sorted and brotli is deterministic, so a source tree always
//! embeds the same bytes.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use std::{env, fs};

use anyhow::{Context as _, Result, bail, ensure};

use super::{Mode, build_designer, build_web, cargo, root, run};

/// Brotli forms are kept only when at most this share (in tenths) of the file.
const KEEP_TENTHS: usize = 9;

pub fn build_server(target: Option<&str>) -> Result<()> {
    build_designer()?;
    build_web(Mode::Gate)?;
    let root = root();
    run(Command::new("bun")
        .args(["run", "build"])
        .current_dir(root.join("apps/console")))?;
    let table = write_table(
        &root.join("apps/console/dist"),
        &root.join("target/console"),
    )?;
    let started = Instant::now();
    let mut build = cargo();
    build
        .current_dir(&root)
        .args(["build", "--locked", "--release", "-p", "cavea-server"])
        .env("CAVEA_CONSOLE_EMBED", &table.path);
    if let Some(target) = target {
        build.args(["--target", target]);
    }
    run(&mut build)?;
    let target_dir =
        env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    let binary = match target {
        Some(target) => target_dir.join(target).join("release/cavea-server"),
        None => target_dir.join("release/cavea-server"),
    };
    let size = fs::metadata(&binary)
        .with_context(|| format!("cannot read {}", binary.display()))?
        .len();
    println!(
        "\ncavea-server: {size} bytes ({}), built in {:.1} s; the console adds {} files, {} \
         bytes with their brotli forms",
        binary.display(),
        started.elapsed().as_secs_f64(),
        table.files,
        table.bytes
    );
    Ok(())
}

struct Table {
    path: PathBuf,
    files: usize,
    bytes: usize,
}

/// Copy `dist` into `out` (cleared first) with brotli forms, and write
/// `out/table.rs`.
fn write_table(dist: &Path, out: &Path) -> Result<Table> {
    let mut files = Vec::new();
    walk(dist, "", &mut files)?;
    ensure!(
        files.iter().any(|f| f == "index.html"),
        "{} has no index.html: build the console first",
        dist.display()
    );
    files.sort();
    if out.exists() {
        fs::remove_dir_all(out).with_context(|| format!("cannot clear {}", out.display()))?;
    }
    let mut table = String::from("&[\n");
    let mut bytes = 0;
    for name in &files {
        let body = fs::read(dist.join(name))?;
        let copy = out.join(name);
        if let Some(parent) = copy.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&copy, &body)?;
        let compressed = brotli(&body)?;
        bytes += body.len();
        let br = if compressed.len() * 10 <= body.len() * KEEP_TENTHS {
            let path = PathBuf::from(format!("{}.br", copy.display()));
            fs::write(&path, &compressed)?;
            bytes += compressed.len();
            format!("Some(include_bytes!({path:?}))")
        } else {
            "None".to_owned()
        };
        writeln!(
            table,
            "    File {{ path: \"/{name}\", body: include_bytes!({copy:?}), br: {br} }},"
        )?;
    }
    table.push(']');
    let path = out.join("table.rs");
    fs::write(&path, table)?;
    Ok(Table {
        path,
        files: files.len(),
        bytes,
    })
}

/// Every file under `dir`, as a path relative to the root with `/`.
fn walk(dir: &Path, prefix: &str, files: &mut Vec<String>) -> Result<()> {
    let entries = fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))?;
    for entry in entries {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            bail!("{} is not UTF-8", entry.path().display());
        };
        // A URL path as it is, without percent-encoding.
        let plain = name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        ensure!(plain, "{}: not a plain file name", entry.path().display());
        let relative = format!("{prefix}{name}");
        if entry.file_type()?.is_dir() {
            walk(&entry.path(), &format!("{relative}/"), files)?;
        } else {
            files.push(relative);
        }
    }
    Ok(())
}

fn brotli(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut writer = brotli::CompressorWriter::new(&mut out, 4096, 11, 22);
        writer.write_all(bytes)?;
    }
    Ok(out)
}
