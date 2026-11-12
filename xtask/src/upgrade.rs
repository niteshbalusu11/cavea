//! `cargo xtask upgrade [--from <rev>]… [--renderer <rev>]… [--keep]`: the
//! upgrade test (13 §13, D-85). For each `--from` revision (a release tag, or
//! any earlier commit): builds that revision's `cavea-server` and `cavea` CLI
//! in a git worktree of its own, then `tools/upgrade/run.ts` fills a database
//! with it, stops it with work in flight, starts this tree's server on the
//! same database and checks that the data answers as before and still works.
//! For each `--renderer` revision: builds that revision's `@cavea/renderer`
//! package and checks, in headless Chromium, that it holds a seat and sees a
//! live change against this tree's server. With no revision named, the
//! previous release and the newest release of each of the two minor versions
//! before this one (01 §6); before the first release there is none, and it
//! says so.
//!
//! Both servers are debug builds with `trust-test-key` and a key made for the run,
//! so a licence can be uploaded to the old one and read by the new one; the
//! key is deleted at the end. Each revision builds in a target directory of
//! its own, `target/upgrade/target/<commit>` (`new` for this tree), kept
//! between runs: checkouts of one workspace must not share one, since cargo
//! would take one's crates for the other's. Worktrees and the run's files are removed at
//! the end unless `--keep`. Exit codes: 0 every scenario passed, 1 one
//! failed, 3 the test could not run.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail, ensure};

use super::{cargo, licence, root, run};
use crate::release::{capture, previous, workspace_version};

pub fn upgrade(args: &[&str]) -> Result<()> {
    let (mut from, mut renderers, mut keep) = (Vec::new(), Vec::new(), false);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match *arg {
            "--from" => from.push(rest.next().context("--from needs a revision")?.to_string()),
            "--renderer" => renderers.push(
                rest.next()
                    .context("--renderer needs a revision")?
                    .to_string(),
            ),
            "--keep" => keep = true,
            _ => bail!("usage: cargo xtask upgrade [--from <rev>]… [--renderer <rev>]… [--keep]"),
        }
    }
    if from.is_empty() && renderers.is_empty() {
        let (last, minors) = previous(&workspace_version()?)?;
        from.extend(last);
        renderers = minors;
        if from.is_empty() {
            println!(
                "no earlier release to upgrade from: name a revision, such as \
                 `cargo xtask upgrade --from <commit> --renderer <commit>`"
            );
            return Ok(());
        }
    }
    let root = root();
    let base = root.join("target/upgrade");
    let work = base.join(format!("run-{}", std::process::id()));
    fs::create_dir_all(&work)?;
    let mut worktrees: Vec<PathBuf> = Vec::new();
    let result = scenarios(&root, &base, &work, &from, &renderers, &mut worktrees);
    if !keep {
        for tree in &worktrees {
            let _ = Command::new("git")
                .args(["worktree", "remove", "--force"])
                .arg(tree)
                .current_dir(&root)
                .status();
        }
        let _ = Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(&root)
            .status();
        let _ = fs::remove_dir_all(&work);
    } else {
        println!("--keep: {} and the worktrees stay", work.display());
    }
    let failed = result?;
    if failed > 0 {
        eprintln!("{failed} upgrade scenario(s) failed");
        std::process::exit(1);
    }
    Ok(())
}

/// Runs every scenario; returns how many failed.
fn scenarios(
    root: &Path,
    base: &Path,
    work: &Path,
    from: &[String],
    renderers: &[String],
    worktrees: &mut Vec<PathBuf>,
) -> Result<usize> {
    let target = base.join("target");
    // A licence key for this run only, trusted by test builds only.
    let keys = work.join("key");
    licence::licence(&["keygen", "--out", &keys.to_string_lossy()])?;
    let public = fs::read_to_string(keys.join("public-key"))?;
    let public = public
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .context("empty public key")?
        .trim()
        .to_owned();
    let today = cavea_licence::Date::of_ms(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis()
            .try_into()?,
    );
    let licence_file = work.join("licence.json");
    licence::licence(&[
        "issue",
        "--key",
        &keys.join("signing-key").to_string_lossy(),
        "--id",
        "upgrade-test",
        "--customer",
        "Upgrade test",
        "--plan",
        "growth",
        "--expires",
        &today.plus(365).to_string(),
        "--out",
        &licence_file.to_string_lossy(),
    ])?;

    println!("\n=== building this tree's server and CLI");
    let new = build(root, &target.join("new"), &work.join("new"), Some(&public))?;
    let mut failed = 0;
    for rev in from {
        let (sha, src) = checkout(root, base, rev, worktrees)?;
        println!("\n=== upgrade from {rev} ({})", &sha[..12]);
        let key = supports_test_key(&src).then_some(public.as_str());
        let old = build(&src, &target.join(&sha[..12]), &work.join(&sha[..12]), key)?;
        let mut command = bun_run(root);
        command
            .args(["--label", rev, "--new"])
            .arg(&new)
            .arg("--old")
            .arg(&old)
            .arg("--out")
            .arg(work.join(&sha[..12]));
        if key.is_some() {
            command.arg("--licence").arg(&licence_file);
        }
        failed += usize::from(!command.status()?.success());
    }
    for rev in renderers {
        let (sha, src) = checkout(root, base, rev, worktrees)?;
        println!(
            "\n=== the renderer of {rev} ({}) against this server",
            &sha[..12]
        );
        if !src.join("packages/renderer/build.ts").exists() {
            println!(
                "{rev} has no renderer package (packages/renderer/build.ts came with #64): skipped"
            );
            continue;
        }
        let dist = work.join(format!("renderer-{}", &sha[..12]));
        build_renderer(&src, &target.join(&sha[..12]), &dist)?;
        let mut command = bun_run(root);
        command
            .args(["--label", rev, "--new"])
            .arg(&new)
            .arg("--renderer")
            .arg(&dist)
            .arg("--out")
            .arg(&dist);
        failed += usize::from(!command.status()?.success());
    }
    let _ = fs::remove_dir_all(&keys);
    Ok(failed)
}

fn bun_run(root: &Path) -> Command {
    let mut command = Command::new("bun");
    command.current_dir(root).arg("tools/upgrade/run.ts");
    command
}

fn supports_test_key(src: &Path) -> bool {
    fs::read_to_string(src.join("crates/cavea-server/Cargo.toml"))
        .is_ok_and(|t| t.contains("trust-test-key"))
}

/// A worktree of `rev` under `target/upgrade/src/`; returns its commit and path.
fn checkout(
    root: &Path,
    base: &Path,
    rev: &str,
    worktrees: &mut Vec<PathBuf>,
) -> Result<(String, PathBuf)> {
    let sha = capture(
        Command::new("git")
            .args(["rev-parse", "--verify", &format!("{rev}^{{commit}}")])
            .current_dir(root),
    )?;
    let src = base.join("src").join(&sha[..12]);
    if !src.exists() {
        run(Command::new("git")
            .args(["worktree", "add", "--detach", "--quiet"])
            .arg(&src)
            .arg(&sha)
            .current_dir(root))?;
    }
    if !worktrees.contains(&src) {
        worktrees.push(src.clone());
    }
    Ok((sha, src))
}

/// `cavea-server` and `cavea` from `src`, copied into `out`; with `key`, the
/// server also trusts it (`trust-test-key`, which refuses a release build,
/// so these are debug builds: what is tested is the data, not the speed).
/// Returns `out`.
fn build(src: &Path, target: &Path, out: &Path, key: Option<&str>) -> Result<PathBuf> {
    let mut command = cargo();
    command
        .current_dir(src)
        .env("CARGO_TARGET_DIR", target)
        .args(["build", "--locked", "-p", "cavea-server", "-p", "cavea-cli"]);
    if let Some(key) = key {
        command
            .args(["--features", "cavea-server/trust-test-key"])
            .env("CAVEA_TEST_LICENCE_KEY", key);
    }
    run(&mut command)?;
    fs::create_dir_all(out)?;
    for binary in ["cavea-server", "cavea"] {
        fs::copy(target.join("debug").join(binary), out.join(binary))
            .with_context(|| format!("no {binary} built from {}", src.display()))?;
    }
    Ok(out.to_path_buf())
}

/// `@cavea/renderer` as that revision packaged it, copied to `out`.
fn build_renderer(src: &Path, target: &Path, out: &Path) -> Result<()> {
    run(Command::new("bun")
        .args(["install", "--frozen-lockfile"])
        .current_dir(src))?;
    // That revision's own xtask and payload gate; an older baseline may not
    // match this toolchain's sizes, so the ungated demo build is the
    // fallback (the same renderer with debug-only exports added).
    let gated = cargo()
        .current_dir(src)
        .env("CARGO_TARGET_DIR", target)
        .args(["xtask", "build-web"])
        .status()?
        .success();
    if !gated {
        println!("the gated build failed at this revision; using its ungated build");
        run(cargo()
            .current_dir(src)
            .env("CARGO_TARGET_DIR", target)
            .args(["xtask", "build-web", "--demo"]))?;
    }
    run(Command::new("bun")
        .args(["run", "build"])
        .current_dir(src.join("packages/renderer")))?;
    let dist = src.join("packages/renderer/dist");
    ensure!(
        dist.join("index.js").exists(),
        "no packages/renderer/dist/index.js"
    );
    if out.exists() {
        fs::remove_dir_all(out)?;
    }
    fs::create_dir_all(out)?;
    for entry in fs::read_dir(&dist)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::copy(entry.path(), out.join(entry.file_name()))?;
        }
    }
    Ok(())
}
