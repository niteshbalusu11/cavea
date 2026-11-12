//! `cargo xtask e2e [--keep] [--headed] [--to n]`: the end-to-end check
//! (`docs/13-testing-benchmarks.md` §9). Builds what a release ships, the
//! server with the console compiled in (`build-server`, so the WASM payload
//! gate runs too) and `@cavea/renderer` as published (`bun run build` in
//! `packages/renderer`, for the buyer's page), always, so a stale binary is
//! never tested; then runs `tools/e2e/run.ts` on that binary and exits with
//! its code (0 passed, 1 a step failed, 2 usage, 3 could not run).

use std::process::Command;

use anyhow::{Context as _, Result};

use super::{root, run, server::build_server};

pub fn e2e(args: &[&str]) -> Result<()> {
    let binary = build_server(None)?;
    let root = root();
    run(Command::new("bun")
        .args(["run", "build"])
        .current_dir(root.join("packages/renderer")))?;
    let status = Command::new("bun")
        .current_dir(&root)
        .arg("tools/e2e/run.ts")
        .arg("--server")
        .arg(&binary)
        .args(args)
        .status()
        .context("cannot run bun tools/e2e/run.ts")?;
    std::process::exit(status.code().unwrap_or(3));
}
