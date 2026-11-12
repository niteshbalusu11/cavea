//! `cargo xtask release`: a release as one command (01 §6, 13 §13, D-85).
//!
//! - `release prepare <version>`: sets the one version everywhere it is
//!   written (the workspace `Cargo.toml` and `Cargo.lock`, the three
//!   published packages, the docs site's variables, the OpenAPI document),
//!   marks the docs site published and drafts the changelog's section. Its
//!   result is the release pull request.
//! - `release [--dry-run] [--skip id,…]`: the checks and the build. Refuses a
//!   dirty tree, a commit not on `origin/main`, versions that disagree, a
//!   missing changelog section, licence file, security policy or registry
//!   name; runs the audits and the test suites 13 §13 names; builds the image
//!   for x86-64 and arm64, runs the image's smoke test and the quick start
//!   against it, packs the npm packages and checks their contents, writes the
//!   SBOMs and checksums; prints the checklist with each item's result and
//!   writes everything under `target/release/<version>/`. Without
//!   `--dry-run` it runs only in the release workflow, stops at the first
//!   failure, and leaves the publishing to `release publish`. With
//!   `--dry-run` it reports every item, then rehearses the publishing
//!   (`publish.rs`) against a local registry with a throwaway key.
//! - `release publish [--dir <dir>]`: the irreversible steps, in the
//!   workflow's second job only (`publish.rs`).

use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Context as _, Result, bail, ensure};
use serde_json::Value;

use super::{cargo, root, run};
use crate::publish;

/// The packages published to npm, in publishing order (a dependency first),
/// each with its directory.
pub const PACKAGES: [(&str, &str); 3] = [
    ("renderer", "packages/renderer"),
    ("server-sdk", "packages/server-sdk"),
    ("react", "packages/react"),
];
/// The architectures the image is built for, as Docker names them.
pub const ARCHES: [&str; 2] = ["amd64", "arm64"];
/// What `release prepare` writes in a fresh changelog section, and what the
/// release refuses while it is still there.
const UNRELEASED: &str = "(unreleased)";
/// The database the test suites use when `CAVEA_TEST_DATABASE_URL` is unset:
/// the shared development PostgreSQL (`deploy/compose.yaml`); every suite
/// makes and drops databases of its own on it.
const DEFAULT_DATABASE: &str = "postgres://cavea:cavea@127.0.0.1:5432/cavea";

pub fn release(args: &[&str]) -> Result<()> {
    match args {
        ["prepare", version] => prepare(version),
        ["publish", rest @ ..] => publish::publish(rest),
        rest => build(rest),
    }
}

// ------------------------------------------------------------------ the checklist

#[derive(Clone)]
pub enum Outcome {
    Pass(String),
    Fail(String),
    /// Passes, with something the releaser must read.
    Warn(String),
    /// Left out with `--skip` (a dry run only).
    Skipped,
    /// Does not apply to this release (the first has no previous one).
    NotApplicable(String),
    /// A person's to do and to record; the command cannot.
    Person(String),
}

pub struct Checklist {
    pub items: Vec<(String, String, Outcome, f64)>,
    pub dry_run: bool,
    pub out: PathBuf,
    skip: Vec<String>,
}

impl Checklist {
    pub fn new(out: PathBuf, dry_run: bool, skip: Vec<String>) -> Self {
        Self {
            items: Vec::new(),
            dry_run,
            out,
            skip,
        }
    }

    pub fn record(&mut self, id: &str, title: &str, outcome: Outcome, seconds: f64) {
        let mark = match &outcome {
            Outcome::Pass(_) => "pass",
            Outcome::Fail(_) => "FAIL",
            Outcome::Warn(_) => "warn",
            Outcome::Skipped => "skipped",
            Outcome::NotApplicable(_) => "n/a",
            Outcome::Person(_) => "person",
        };
        println!("[{mark}] {id}: {title} ({seconds:.0} s)");
        if let Outcome::Fail(why) | Outcome::Warn(why) = &outcome {
            println!("       {}", why.replace('\n', "\n       "));
        }
        self.items
            .push((id.to_owned(), title.to_owned(), outcome, seconds));
    }

    /// Runs `check` as item `id` unless skipped; returns whether it passed.
    /// A real release stops at the first failure.
    pub fn step(
        &mut self,
        id: &str,
        title: &str,
        check: impl FnOnce() -> Result<Outcome>,
    ) -> Result<bool> {
        if self.skip.iter().any(|s| s == id) {
            self.record(id, title, Outcome::Skipped, 0.0);
            return Ok(false);
        }
        println!("\n--- {id}: {title}");
        let started = Instant::now();
        let outcome = check().unwrap_or_else(|e| Outcome::Fail(format!("{e:#}")));
        let passed = !matches!(outcome, Outcome::Fail(_));
        self.record(id, title, outcome, started.elapsed().as_secs_f64());
        if !passed && !self.dry_run {
            self.write()?;
            bail!("release stopped at `{id}`: see {}", self.out.display());
        }
        Ok(passed)
    }

    pub fn failures(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i.2, Outcome::Fail(_) | Outcome::Skipped))
            .count()
    }

    /// Prints the checklist and writes it to `checklist.md`.
    pub fn write(&self) -> Result<()> {
        let mut text =
            String::from("| Item | Check | Result | Seconds | Detail |\n|---|---|---|---|---|\n");
        for (id, title, outcome, seconds) in &self.items {
            let (mark, detail) = match outcome {
                Outcome::Pass(d) => ("pass", d.as_str()),
                Outcome::Fail(d) => ("**FAIL**", d.as_str()),
                Outcome::Warn(d) => ("warn", d.as_str()),
                Outcome::Skipped => ("**skipped**", "left out with --skip"),
                Outcome::NotApplicable(d) => ("n/a", d.as_str()),
                Outcome::Person(d) => ("a person", d.as_str()),
            };
            let detail = detail.replace('\n', "<br>").replace('|', "\\|");
            writeln!(
                text,
                "| `{id}` | {title} | {mark} | {seconds:.0} | {detail} |"
            )?;
        }
        fs::create_dir_all(&self.out)?;
        fs::write(self.out.join("checklist.md"), &text)?;
        println!("\n{text}");
        Ok(())
    }
}

// ------------------------------------------------------------------ helpers

/// A command's standard output, failing with its standard error.
pub fn capture(command: &mut Command) -> Result<String> {
    let output = command
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("cannot run {command:?}"))?;
    ensure!(
        output.status.success(),
        "{command:?} failed with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Runs `command` with its output in `logs/<id>.log`; on failure, the log's
/// last lines are the error.
pub fn logged(out: &Path, id: &str, command: &mut Command) -> Result<()> {
    let dir = out.join("logs");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{id}.log"));
    let file = File::options().create(true).append(true).open(&path)?;
    let status = command
        .stdout(file.try_clone()?)
        .stderr(file)
        .status()
        .with_context(|| format!("cannot run {command:?}"))?;
    if status.success() {
        return Ok(());
    }
    let log = fs::read_to_string(&path).unwrap_or_default();
    let tail: Vec<&str> = log.lines().rev().take(25).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    bail!(
        "{command:?} failed with {status} (log: {}):\n{}",
        path.display(),
        tail.join("\n")
    )
}

/// A command run from the root, with the release's environment: scratch
/// files in `target/tmp`, eight build jobs, the shared PostgreSQL for the
/// suites (each makes and drops databases of its own), unless set.
pub fn sh(args: &[&str]) -> Command {
    let root = root();
    let mut command = Command::new(args.first().copied().unwrap_or("false"));
    command
        .args(args.get(1..).unwrap_or_default())
        .current_dir(&root)
        .env("TMPDIR", root.join("target/tmp"))
        .env("SYFT_CHECK_FOR_APP_UPDATE", "false");
    for (name, value) in [
        ("CARGO_BUILD_JOBS", "8"),
        ("CAVEA_TEST_DATABASE_URL", DEFAULT_DATABASE),
    ] {
        if env::var_os(name).is_none() {
            command.env(name, value);
        }
    }
    command
}

fn git(args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    capture(command.args(args).current_dir(root()))
}

fn read_json(path: &Path) -> Result<Value> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("{} is not JSON", path.display()))
}

/// The workspace version: `version` under `[workspace.package]` in `Cargo.toml`.
pub fn workspace_version() -> Result<String> {
    let text = fs::read_to_string(root().join("Cargo.toml"))?;
    let mut in_package = false;
    for line in text.lines() {
        if line.starts_with('[') {
            in_package = line.trim() == "[workspace.package]";
        } else if in_package && let Some(v) = line.strip_prefix("version = \"") {
            return Ok(v.trim_end_matches('"').to_owned());
        }
    }
    bail!("Cargo.toml has no version under [workspace.package]")
}

/// `X.Y.Z`, or `X.Y.Z-pre` with a pre-release of letters, digits, dots and hyphens.
pub fn valid_version(v: &str) -> bool {
    let (core, pre) = v.split_once('-').map_or((v, None), |(c, p)| (c, Some(p)));
    let numbers: Vec<&str> = core.split('.').collect();
    numbers.len() == 3
        && numbers.iter().all(|n| {
            !n.is_empty()
                && n.bytes().all(|b| b.is_ascii_digit())
                && (n == &"0" || !n.starts_with('0'))
        })
        && pre.is_none_or(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        })
}

pub fn prerelease(v: &str) -> bool {
    v.contains('-')
}

/// The docs site's variables (`apps/docs/site.json`), where the image's and
/// the packages' public names live (D-83, D-85).
fn site() -> Result<Value> {
    read_json(&root().join("apps/docs/site.json"))
}

fn site_var(site: &Value, name: &str) -> Result<String> {
    site["vars"][name]
        .as_str()
        .map(str::to_owned)
        .with_context(|| format!("apps/docs/site.json has no vars.{name}"))
}

/// The image's repository (`registry/path/name`) and tag from `vars.image`.
pub fn image_name(site: &Value) -> Result<(String, String)> {
    let image = site_var(site, "image")?;
    let (repository, tag) = image
        .rsplit_once(':')
        .filter(|(r, t)| !t.contains('/') && !r.is_empty())
        .with_context(|| format!("vars.image `{image}` has no tag"))?;
    Ok((repository.to_owned(), tag.to_owned()))
}

/// Release tags (`vX.Y.Z`, no pre-releases) reachable from HEAD, newest first.
fn release_tags() -> Result<Vec<String>> {
    let tags = git(&[
        "tag",
        "--merged",
        "HEAD",
        "--list",
        "v*",
        "--sort=-v:refname",
    ])?;
    Ok(tags
        .lines()
        .filter(|t| {
            t.strip_prefix('v')
                .is_some_and(|v| valid_version(v) && !prerelease(v))
        })
        .map(str::to_owned)
        .collect())
}

fn minor(tag: &str) -> String {
    tag.trim_start_matches('v')
        .split('.')
        .take(2)
        .collect::<Vec<_>>()
        .join(".")
}

/// The previous release's tag, and the newest tag of each of the two minor
/// versions before this one (the renderers the server must still serve, 01 §6).
pub fn previous(version: &str) -> Result<(Option<String>, Vec<String>)> {
    let current = format!("v{version}");
    let tags: Vec<String> = release_tags()?
        .into_iter()
        .filter(|t| *t != current)
        .collect();
    let mut minors: Vec<String> = Vec::new();
    for tag in &tags {
        if minor(tag) != minor(&current) && !minors.iter().any(|t| minor(t) == minor(tag)) {
            minors.push(tag.clone());
        }
    }
    minors.truncate(2);
    Ok((tags.first().cloned(), minors))
}

/// The changelog's section for `version`: its heading line and its body.
fn changelog_section(version: &str) -> Result<Option<(String, String)>> {
    let text = fs::read_to_string(root().join("CHANGELOG.md"))?;
    let heading = format!("## {version}");
    let mut lines = text.lines();
    let Some(head) = lines.find(|l| *l == heading || l.starts_with(&format!("{heading} "))) else {
        return Ok(None);
    };
    let body: Vec<&str> = lines.take_while(|l| !l.starts_with("## ")).collect();
    Ok(Some((head.to_owned(), body.join("\n").trim().to_owned())))
}

fn hex_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("{:x}{:05x}", std::process::id(), nanos & 0xfffff)
}

/// Every place the version is written must agree with `Cargo.toml`'s.
fn versions_agree(version: &str) -> Result<Vec<String>> {
    let root = root();
    let mut wrong = Vec::new();
    for (_, dir) in PACKAGES {
        let pkg = read_json(&root.join(dir).join("package.json"))?;
        if pkg["version"] != version {
            wrong.push(format!("{dir}/package.json has {}", pkg["version"]));
        }
    }
    let site = site()?;
    if site_var(&site, "version")? != version {
        wrong.push(format!(
            "apps/docs/site.json vars.version is {}",
            site["vars"]["version"]
        ));
    }
    let (_, tag) = image_name(&site)?;
    if tag != version {
        wrong.push(format!("apps/docs/site.json vars.image has tag {tag}"));
    }
    if !site_var(&site, "renderer_script")?.contains(&format!("@{version}/")) {
        wrong.push("apps/docs/site.json vars.renderer_script names another version".to_owned());
    }
    let openapi = read_json(&root.join("crates/cavea-server/openapi.json"))?;
    if openapi["info"]["version"] != version {
        wrong.push(format!(
            "crates/cavea-server/openapi.json info.version is {}",
            openapi["info"]["version"]
        ));
    }
    let lock = fs::read_to_string(root.join("Cargo.lock"))?;
    let members = lock
        .split("[[package]]")
        .filter(|p| p.contains("name = \"cavea-") && !p.contains("source = "));
    for member in members {
        if !member.contains(&format!("version = \"{version}\"")) {
            wrong.push("Cargo.lock has a workspace crate at another version".to_owned());
            break;
        }
    }
    Ok(wrong)
}

// ------------------------------------------------------------------ prepare

fn prepare(version: &str) -> Result<()> {
    ensure!(
        valid_version(version),
        "`{version}` is not a version: X.Y.Z, or X.Y.Z-rc.N for a pre-release"
    );
    let root = root();
    let old = workspace_version()?;
    // Cargo.toml: the one line under [workspace.package].
    let manifest = fs::read_to_string(root.join("Cargo.toml"))?;
    let from = format!("[workspace.package]\nversion = \"{old}\"");
    ensure!(manifest.contains(&from), "Cargo.toml: expected `{from}`");
    fs::write(
        root.join("Cargo.toml"),
        manifest.replacen(
            &from,
            &format!("[workspace.package]\nversion = \"{version}\""),
            1,
        ),
    )?;
    run(cargo()
        .current_dir(&root)
        .args(["update", "--workspace", "--offline"]))?;
    for (_, dir) in PACKAGES {
        let path = root.join(dir).join("package.json");
        let text = fs::read_to_string(&path)?;
        let pkg: Value = serde_json::from_str(&text)?;
        let line = format!("\"version\": {},", pkg["version"]);
        ensure!(
            text.matches(&line).count() == 1,
            "{}: expected one `{line}`",
            path.display()
        );
        fs::write(
            &path,
            text.replacen(&line, &format!("\"version\": \"{version}\","), 1),
        )?;
    }
    // The docs site: the version, the image's tag, the script's URL, published.
    let path = root.join("apps/docs/site.json");
    let mut site = read_json(&path)?;
    let (repository, _) = image_name(&site)?;
    let script = site_var(&site, "renderer_script")?;
    let package = site_var(&site, "renderer_package")?;
    let (before, after) = script
        .split_once(&format!("{package}@"))
        .context("vars.renderer_script does not name vars.renderer_package@version")?;
    let rest = after.split_once('/').map_or("", |(_, r)| r);
    let vars = &mut site["vars"];
    vars["version"] = version.into();
    vars["image"] = format!("{repository}:{version}").into();
    vars["renderer_script"] = format!("{before}{package}@{version}/{rest}").into();
    site["published"] = true.into();
    fs::write(&path, format!("{}\n", serde_json::to_string_pretty(&site)?))?;
    let openapi = capture(cargo().current_dir(&root).args([
        "run",
        "-q",
        "-p",
        "cavea-server",
        "--",
        "openapi",
    ]))?;
    fs::write(
        root.join("crates/cavea-server/openapi.json"),
        format!("{openapi}\n"),
    )?;
    // The changelog: a draft section from the merged titles, unless one exists.
    let drafted = if changelog_section(version)?.is_none() {
        let titles = capture(sh(&["bun", "tools/changelog.ts", "HEAD"]).current_dir(&root))?;
        let path = root.join("CHANGELOG.md");
        let text = fs::read_to_string(&path)?;
        let at = text.find("\n## ").map_or(text.len(), |i| i + 1);
        let section = format!(
            "## {version} {UNRELEASED}\n\nCondense these pull-request titles, add `### Upgrading` \
             (what an operator must do; breaking changes first), and replace `{UNRELEASED}` with \
             the date:\n\n{titles}\n\n"
        );
        fs::write(&path, format!("{}{section}{}", &text[..at], &text[at..]))?;
        true
    } else {
        false
    };
    println!(
        "version {old} -> {version}: Cargo.toml, Cargo.lock, {} package.json files, \
         apps/docs/site.json (published), crates/cavea-server/openapi.json.",
        PACKAGES.len()
    );
    println!(
        "\nStill to do in this pull request:\n\
         - CHANGELOG.md: {} the `## {version}` section (no `{UNRELEASED}`; `### Upgrading` from \
           the second release on).\n\
         - The compiler writes its version into every compiled chart, so the pinned chart \
           hashes change: run `cargo test -p cavea-compiler -p cavea-testkit` and copy each \
           new hash from the failures into crates/cavea-testkit/src/presets/tests.rs and \
           crates/cavea-compiler/src/compile/tests.rs.\n\
         - At the first release: delete apps/docs/content/before-release.md and its line in \
           apps/docs/site.json; set vars.image to the real registry.\n\
         - `cargo xtask release --dry-run`, then file the pull request. After it merges, tag \
           its commit on main: `git tag -s v{version} <commit> && git push origin v{version}`.",
        if drafted { "write" } else { "check" }
    );
    Ok(())
}

// ------------------------------------------------------------------ the build

fn build(args: &[&str]) -> Result<()> {
    let mut dry_run = false;
    let mut skip = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match *arg {
            "--dry-run" => dry_run = true,
            "--skip" => skip.extend(
                rest.next()
                    .context("--skip needs a comma-separated list of items")?
                    .split(',')
                    .map(str::to_owned),
            ),
            other => bail!(
                "usage: cargo xtask release [--dry-run] [--skip item,…] | prepare <version> | \
                 publish [--dir <dir>] (unknown: {other})"
            ),
        }
    }
    let in_workflow = env::var("GITHUB_ACTIONS").as_deref() == Ok("true");
    ensure!(
        dry_run || in_workflow,
        "a release runs only in the release workflow (.github/workflows/release.yml), where \
         keyless signing and npm provenance have the workflow's identity; run \
         `cargo xtask release --dry-run` here"
    );
    ensure!(dry_run || skip.is_empty(), "--skip is for dry runs only");
    let root = root();
    let version = workspace_version()?;
    let out = root.join("target/release").join(&version);
    if out.exists() {
        fs::remove_dir_all(&out).with_context(|| format!("cannot clear {}", out.display()))?;
    }
    fs::create_dir_all(out.join("logs"))?;
    fs::create_dir_all(root.join("target/tmp"))?;
    let started = Instant::now();
    let mut list = Checklist::new(out.clone(), dry_run, skip);
    println!(
        "Cavea {version}: {} into {}",
        if dry_run { "dry run" } else { "release" },
        out.display()
    );

    let commit = git(&["rev-parse", "HEAD"])?;
    let (previous, minors) = previous(&version)?;
    preflight(
        &mut list,
        &version,
        &commit,
        previous.as_deref(),
        in_workflow,
    )?;
    audits(&mut list)?;
    suites(&mut list, previous.as_deref(), &minors)?;
    let images = artefacts(&mut list, &version, &commit)?;

    for (id, title, why) in PERSON {
        list.record(id, title, Outcome::Person((*why).to_owned()), 0.0);
    }
    let manifest = serde_json::json!({
        "version": version,
        "commit": commit,
        "image": image_name(&site()?)?.0,
        "images": images,
        "packages": PACKAGES.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        "prerelease": prerelease(&version),
        "failures": list.failures(),
        "dry_run": dry_run,
    });
    fs::write(
        out.join("release.json"),
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;
    if let Some((_, body)) = changelog_section(&version)? {
        fs::write(out.join("notes.md"), format!("{body}\n"))?;
    }
    list.write()?;

    if dry_run {
        publish::rehearse(&mut list, &out, &images)?;
        list.write()?;
        let failures = list.failures();
        println!(
            "\nDry run of {version} in {:.0} min: {failures} item(s) failed or skipped; a real \
             release {}. Built: {}",
            started.elapsed().as_secs_f64() / 60.0,
            if failures == 0 {
                "would go ahead"
            } else {
                "would stop at the first"
            },
            out.display()
        );
        for tag in images.values() {
            let _ = Command::new("docker")
                .args(["rmi", "-f", tag])
                .stdout(Stdio::null())
                .status();
        }
    } else {
        println!(
            "\n{version} checked and built in {:.0} min: {}. `release publish` is next.",
            started.elapsed().as_secs_f64() / 60.0,
            out.display()
        );
    }
    Ok(())
}

/// What the command cannot do: a person does it and records the result in
/// the release's pull request (13 §13).
const PERSON: &[(&str, &str, &str)] = &[
    (
        "devices",
        "Device benchmarks within every budget; results committed",
        "13 §6.2 on the reference devices",
    ),
    (
        "load",
        "L1 and L2 pass on reference hardware; reconciler clean",
        "cavea-loadgen (13 §7.1) on the reference machine, then `cavea-server verify --all`",
    ),
    ("chaos", "Chaos experiments pass", "13 §7.3"),
    (
        "nightly",
        "Nightly suites green: long simulation, fuzzing, L2 for two minutes",
        "13 §12; the release ran the short simulation and the full golden and end-to-end checks",
    ),
    (
        "accessibility",
        "Manual accessibility matrix",
        "11; screen readers on real devices",
    ),
    (
        "arm64-run",
        "The arm64 image started on arm64 hardware",
        "built and signed here, never executed without arm64 emulation",
    ),
    (
        "notes",
        "Changelog and upgrade notes read for accuracy; breaking changes called out",
        "the section in CHANGELOG.md this release checked exists",
    ),
];

fn preflight(
    list: &mut Checklist,
    version: &str,
    commit: &str,
    previous: Option<&str>,
    in_workflow: bool,
) -> Result<()> {
    let root = root();
    list.step("tools", "Tool versions recorded", || {
        let mut text = String::new();
        for (name, args) in [
            ("rustc", vec!["rustc", "--version"]),
            ("cargo", vec!["cargo", "--version"]),
            ("bun", vec!["bun", "--version"]),
            ("wasm-opt", vec!["wasm-opt", "--version"]),
            (
                "docker",
                vec!["docker", "version", "-f", "{{.Server.Version}}"],
            ),
            ("buildx", vec!["docker", "buildx", "version"]),
            ("cargo-deny", vec!["cargo", "deny", "--version"]),
            ("cargo-audit", vec!["cargo", "audit", "--version"]),
            ("syft", vec!["syft", "version"]),
            ("cosign", vec!["cosign", "version"]),
            ("crane", vec!["crane", "version"]),
            ("npm", vec!["npm", "--version"]),
            ("actionlint", vec!["actionlint", "--version"]),
        ] {
            let found = capture(&mut sh(&args)).with_context(|| {
                format!("{name} is missing: run the release in `nix develop .#release`")
            })?;
            let line = found
                .lines()
                .find(|l| {
                    l.contains("ersion") || l.chars().next().is_some_and(|c| c.is_ascii_digit())
                })
                .or_else(|| found.lines().next())
                .unwrap_or("");
            writeln!(text, "{name}: {}", line.trim())?;
        }
        fs::write(list_out(&root, version).join("tools.txt"), &text)?;
        Ok(Outcome::Pass(text.trim().to_owned()))
    })?;

    list.step("tree", "Clean checkout of a commit on origin/main", || {
        let dirty = git(&["status", "--porcelain"])?;
        ensure!(dirty.is_empty(), "the working tree has changes:\n{dirty}");
        if !in_workflow {
            let _ = git(&["fetch", "-q", "origin", "main"]);
        }
        let on_main = Command::new("git")
            .args(["merge-base", "--is-ancestor", "HEAD", "origin/main"])
            .current_dir(&root)
            .status()?
            .success();
        ensure!(on_main, "HEAD ({commit}) is not on origin/main");
        Ok(Outcome::Pass(format!("{commit}, clean, on origin/main")))
    })?;

    let dry_run = list.dry_run;
    list.step("version", "One version everywhere, tagged on this commit", || {
        ensure!(valid_version(version), "`{version}` is not X.Y.Z or X.Y.Z-pre");
        ensure!(
            version != "0.0.0",
            "the version is 0.0.0: run `cargo xtask release prepare <version>` in a pull request first"
        );
        let wrong = versions_agree(version)?;
        ensure!(wrong.is_empty(), "the version is {version} in Cargo.toml, but:\n{}", wrong.join("\n"));
        let tag = format!("v{version}");
        let tagged = git(&["tag", "--points-at", "HEAD"])?;
        if tagged.lines().any(|t| t == tag) {
            Ok(Outcome::Pass(format!("{version} everywhere; {tag} is this commit")))
        } else if dry_run {
            Ok(Outcome::Warn(format!("{version} everywhere; not tagged {tag} (a dry run need not be)")))
        } else {
            bail!("{tag} does not point at this commit")
        }
    })?;

    list.step(
        "changelog",
        "Changelog section and upgrade notes written",
        || {
            let (head, body) = changelog_section(version)?
                .with_context(|| format!("CHANGELOG.md has no `## {version}` section"))?;
            ensure!(
                !head.contains(UNRELEASED),
                "`{head}`: write the section and date it"
            );
            ensure!(!body.is_empty(), "the `## {version}` section is empty");
            if previous.is_some() {
                ensure!(
                    body.contains("### Upgrading"),
                    "the section has no `### Upgrading` notes"
                );
            }
            let check = capture(&mut sh(&["bun", "tools/changelog.ts", "--check", "HEAD"]))?;
            Ok(Outcome::Pass(format!("`{head}`; {check}")))
        },
    )?;

    list.step("licence-file", "The packages' licence (LICENSE) exists", || {
        ensure!(
            root.join("LICENSE").exists(),
            "no LICENSE at the repository's root: the licence agreement is the owner's to write \
             (12 §8); until it exists the packages build as private and npm refuses them"
        );
        Ok(Outcome::Pass("LICENSE, copied into each package".to_owned()))
    })?;

    list.step(
        "security-policy",
        "SECURITY.md with a contact address",
        || {
            let text = fs::read_to_string(root.join("SECURITY.md"))
                .context("no SECURITY.md (SEC-007: needs the owner's contact address)")?;
            ensure!(
                !text.to_ascii_lowercase().contains("placeholder"),
                "SECURITY.md still has a placeholder"
            );
            Ok(Outcome::Pass("present".to_owned()))
        },
    )?;

    list.step("names", "Image and package names are real and agree", || {
        let site = site()?;
        let (repository, _) = image_name(&site)?;
        let mut problems = Vec::new();
        let registry = repository.split('/').next().unwrap_or("");
        if registry.contains("example") || !registry.contains('.') && registry != "localhost" {
            problems.push(format!(
                "vars.image in apps/docs/site.json is `{repository}`: set the registry the owner chose"
            ));
        }
        for (key, (name, dir)) in [("renderer_package", PACKAGES[0]), ("server_sdk_package", PACKAGES[1])] {
            let pkg = read_json(&root.join(dir).join("package.json"))?;
            if site["vars"][key] != pkg["name"] {
                problems.push(format!("vars.{key} is not {dir}'s name ({name})"));
            }
        }
        let repo = site_var(&site, "repository")?;
        for (_, dir) in PACKAGES {
            let pkg = read_json(&root.join(dir).join("package.json"))?;
            let url = pkg["repository"]["url"].as_str().unwrap_or("");
            if url != format!("git+{repo}.git") {
                problems.push(format!("{dir}/package.json repository.url is not git+{repo}.git"));
            }
        }
        if site["published"] != true {
            problems.push("apps/docs/site.json is not `published` (release prepare sets it)".to_owned());
        }
        ensure!(problems.is_empty(), "{}", problems.join("\n"));
        Ok(Outcome::Pass(format!("image {repository}; packages and docs agree")))
    })?;

    list.step("licence-keys", "The server trusts a licence key", || {
        Ok(if cavea_licence::TRUSTED.is_empty() {
            Outcome::Warn(
                "TRUSTED in crates/cavea-licence/src/keys.rs is empty: this release treats every \
                 licence as invalid (an evaluation release; holding and booking still work)"
                    .to_owned(),
            )
        } else {
            Outcome::Pass(format!("{} key(s)", cavea_licence::TRUSTED.len()))
        })
    })?;

    list.step(
        "actions",
        "Workflow actions pinned by commit; workflows lint clean",
        || {
            let pins = capture(&mut sh(&["bun", "tools/actions.ts", "--check"]))?;
            capture(&mut sh(&["actionlint"]))?;
            Ok(Outcome::Pass(format!("{pins}; actionlint clean")))
        },
    )?;

    list.step(
        "migrations",
        "Released migrations unchanged (forward-only)",
        || {
            let Some(previous) = previous else {
                return Ok(Outcome::NotApplicable("the first release".to_owned()));
            };
            let changed = git(&[
                "diff",
                "--name-only",
                "--diff-filter=MDR",
                previous,
                "HEAD",
                "--",
                "crates/cavea-server/migrations/",
            ])?;
            ensure!(
                changed.is_empty(),
                "migrations released in {previous} were changed:\n{changed}"
            );
            let added = git(&[
                "diff",
                "--name-only",
                "--diff-filter=A",
                previous,
                "HEAD",
                "--",
                "crates/cavea-server/migrations/",
            ])?;
            Ok(Outcome::Pass(format!(
                "none changed since {previous}; added: {}",
                if added.is_empty() {
                    "none".to_owned()
                } else {
                    added.replace('\n', ", ")
                }
            )))
        },
    )?;

    list.step("compatibility", "What changed in each public surface since the last release", || {
        let Some(previous) = previous else {
            return Ok(Outcome::NotApplicable("the first release".to_owned()));
        };
        let mut text = String::new();
        for (what, path) in [
            ("renderer API", "packages/renderer/api.md"),
            ("server SDK API", "packages/server-sdk/api.md"),
            ("React component", "packages/react/src"),
            ("REST API", "crates/cavea-server/openapi.json"),
            ("chart format", "crates/cavea-format/src/ccf.rs"),
            ("wire protocol", "crates/cavea-format/src/wire.rs"),
        ] {
            let stat = git(&["diff", "--shortstat", previous, "HEAD", "--", path])?;
            writeln!(text, "{what}: {}", if stat.is_empty() { "unchanged" } else { &stat })?;
        }
        Ok(Outcome::Warn(format!(
            "{}read each changed surface against the version policy (docs/01-architecture.md §6)",
            text
        )))
    })?;
    Ok(())
}

fn list_out(root: &Path, version: &str) -> PathBuf {
    root.join("target/release").join(version)
}

fn audits(list: &mut Checklist) -> Result<()> {
    let out = list.out.clone();
    list.step("cargo-deny", "cargo deny check (deny.toml)", || {
        logged(
            &out,
            "cargo-deny",
            &mut sh(&["cargo", "deny", "--locked", "check"]),
        )?;
        let log = fs::read_to_string(out.join("logs/cargo-deny.log")).unwrap_or_default();
        let duplicates = log.matches("warning[duplicate]").count();
        Ok(Outcome::Pass(format!(
            "advisories, bans, licences, sources ok; {duplicates} duplicate-version warnings"
        )))
    })?;
    list.step("cargo-audit", "cargo audit (.cargo/audit.toml)", || {
        let db = root().join("target/tmp/advisory-db");
        let db = db.to_string_lossy();
        logged(
            &out,
            "cargo-audit",
            &mut sh(&["cargo", "audit", "--db", &db, "--deny", "warnings"]),
        )?;
        Ok(Outcome::Pass(
            "no advisory but the documented exceptions".to_owned(),
        ))
    })?;
    list.step("bun-audit", "bun audit", || {
        logged(
            &out,
            "bun-audit",
            &mut sh(&["bun", "install", "--frozen-lockfile"]),
        )?;
        logged(&out, "bun-audit", &mut sh(&["bun", "audit"]))?;
        Ok(Outcome::Pass("no vulnerabilities".to_owned()))
    })?;
    Ok(())
}

fn suites(list: &mut Checklist, previous: Option<&str>, minors: &[String]) -> Result<()> {
    let out = list.out.clone();
    let cmd = |id: &str, commands: &[&[&str]]| -> Result<Outcome> {
        for args in commands {
            logged(&out, id, &mut sh(args))?;
        }
        Ok(Outcome::Pass(format!("logs/{id}.log")))
    };
    list.step(
        "rust-checks",
        "cargo fmt, clippy, build; check-deps; types --check",
        || {
            cmd(
                "rust-checks",
                &[
                    &["cargo", "fmt", "--all", "--check"],
                    &[
                        "cargo",
                        "clippy",
                        "--workspace",
                        "--all-targets",
                        "--locked",
                        "--",
                        "-D",
                        "warnings",
                    ],
                    &["cargo", "build", "--workspace", "--locked"],
                    &["cargo", "xtask", "check-deps"],
                    &["cargo", "xtask", "types", "--check"],
                ],
            )
        },
    )?;
    list.step("payload", "cargo xtask build-web: the payload gate", || {
        cmd("payload", &[&["cargo", "xtask", "build-web"]])?;
        let log = fs::read_to_string(out.join("logs/payload.log")).unwrap_or_default();
        let gate = log
            .lines()
            .rfind(|l| l.contains("payload"))
            .unwrap_or("")
            .to_owned();
        Ok(Outcome::Pass(gate))
    })?;
    list.step(
        "web-checks",
        "bun lint, format:check, typecheck, build, test",
        || {
            cmd(
                "web-checks",
                &[
                    &["bun", "install", "--frozen-lockfile"],
                    &["bun", "run", "lint"],
                    &["bun", "run", "format:check"],
                    &["bun", "run", "typecheck"],
                    &["cargo", "xtask", "build-designer"],
                    &["bun", "run", "build"],
                    &["bun", "run", "test"],
                ],
            )
        },
    )?;
    list.step(
        "rust-tests",
        "cargo test --workspace, with the database tests",
        || {
            cmd(
                "rust-tests",
                &[
                    &["cargo", "test", "--workspace", "--locked"],
                    &[
                        "cargo",
                        "test",
                        "--locked",
                        "-p",
                        "cavea-server",
                        "--",
                        "--include-ignored",
                        "--skip",
                        "measure_",
                    ],
                ],
            )
        },
    )?;
    list.step(
        "simulation",
        "Engine model and simulation runs (13 §3)",
        || {
            cmd(
                "simulation",
                &[
                    &[
                        "cargo",
                        "run",
                        "--release",
                        "--locked",
                        "-p",
                        "cavea-sim",
                        "--example",
                        "soak",
                        "--",
                        "model",
                        "--seeds",
                        "1..20001",
                        "--steps",
                        "400",
                        "--jobs",
                        "4",
                    ],
                    &[
                        "cargo",
                        "run",
                        "--release",
                        "--locked",
                        "-p",
                        "cavea-sim",
                        "--example",
                        "soak",
                        "--",
                        "sim",
                        "--seeds",
                        "1..2001",
                        "--steps",
                        "1000",
                        "--jobs",
                        "4",
                    ],
                ],
            )
        },
    )?;
    list.step("determinism", "Compiled hashes native and wasm32", || {
        cmd("determinism", &[&["cargo", "xtask", "determinism"]])
    })?;
    list.step("server-sdk", "@cavea/server-sdk against a server", || {
        cmd(
            "server-sdk",
            &[&["bun", "test", "packages/server-sdk/test"]],
        )
    })?;
    list.step("golden", "Renderer golden images", || {
        cmd("golden", &[&["cargo", "xtask", "golden"]])
    })?;
    list.step("e2e", "End-to-end check (13 §9.1)", || {
        cmd("e2e", &[&["cargo", "xtask", "e2e"]])
    })?;
    list.step("fixtures", "Compatibility fixtures (13 §8)", || {
        // The golden and end-to-end checks rebuilt the WASM; the fixtures
        // need the gated release build, which is what ships.
        cmd(
            "fixtures",
            &[
                &["cargo", "xtask", "build-web"],
                &["bun", "fixtures/run.ts"],
            ],
        )
    })?;
    list.step("upgrade", "Upgrade from the previous release with data; older renderers", || {
        let mut args = vec!["cargo", "xtask", "upgrade"];
        if let Some(previous) = previous {
            args.extend(["--from", previous]);
        }
        for tag in minors {
            args.extend(["--renderer", tag.as_str()]);
        }
        if args.len() == 3 {
            return Ok(Outcome::NotApplicable(
                "the first release: no earlier one to upgrade from (`cargo xtask upgrade --from <rev>` \
                 tests any earlier commit)"
                    .to_owned(),
            ));
        }
        cmd("upgrade", &[&args])
    })?;
    Ok(())
}

/// Builds the images and packages; returns each architecture's local image tag.
fn artefacts(
    list: &mut Checklist,
    version: &str,
    commit: &str,
) -> Result<BTreeMap<String, String>> {
    let out = list.out.clone();
    let site = site()?;
    let repository = site_var(&site, "repository")?;
    let local = format!("cavea-release-{}", hex_id());
    let created = git(&["show", "-s", "--format=%cI", "HEAD"])?;
    let mut images = BTreeMap::new();
    for arch in ARCHES {
        let tag = format!("{local}:{version}-{arch}");
        let built = list.step(
            &format!("image-{arch}"),
            &format!("Image for linux/{arch}"),
            || {
                let jobs = env::var("CARGO_BUILD_JOBS").unwrap_or_else(|_| "8".to_owned());
                let labels = [
                    format!("org.opencontainers.image.version={version}"),
                    format!("org.opencontainers.image.revision={commit}"),
                    format!("org.opencontainers.image.source={repository}"),
                    format!("org.opencontainers.image.created={created}"),
                    "org.opencontainers.image.title=cavea-server".to_owned(),
                ];
                let mut build = sh(&["docker", "buildx", "build", "-f", "deploy/Dockerfile"]);
                build.args(["--platform", &format!("linux/{arch}")]);
                build.args(["--build-arg", &format!("CARGO_BUILD_JOBS={jobs}")]);
                for label in &labels {
                    build.args(["--label", label]);
                }
                build.args(["-t", &tag, "--load", "."]);
                logged(&out, &format!("image-{arch}"), &mut build)?;
                let dir = out.join("images");
                fs::create_dir_all(&dir)?;
                let file = dir.join(format!("cavea-server-{version}-{arch}.tar"));
                run(sh(&["docker", "save", "-o"]).arg(&file).arg(&tag))?;
                let id = capture(&mut sh(&[
                    "docker",
                    "image",
                    "inspect",
                    "-f",
                    "{{.Id}} {{.Size}}",
                    &tag,
                ]))?;
                Ok(Outcome::Pass(format!(
                    "{tag}: image {id} B; saved to images/"
                )))
            },
        )?;
        if built {
            images.insert(arch.to_owned(), tag);
        }
    }
    list.step(
        "packages",
        "npm packages packed and their contents checked",
        || packages(&out, version, &repository),
    )?;
    let amd64 = images.get("amd64").cloned();
    list.step("smoke", "deploy/smoke.ts against the x86-64 image", || {
        let tag = amd64.as_deref().context("no x86-64 image")?;
        logged(
            &out,
            "smoke",
            &mut sh(&["bun", "deploy/smoke.ts", "--image", tag]),
        )?;
        Ok(Outcome::Pass(
            "setup, hold and book, console, headers, reconciler, stop, restart, log".to_owned(),
        ))
    })?;
    list.step(
        "quickstart",
        "The docs' quick start against the release's image",
        || {
            let tag = amd64.as_deref().context("no x86-64 image")?;
            logged(
                &out,
                "quickstart",
                &mut sh(&["bun", "apps/docs/quickstart/check.ts", "--image", tag]),
            )?;
            Ok(Outcome::Pass("console and API paths".to_owned()))
        },
    )?;
    list.step(
        "sbom",
        "SBOMs (SPDX 2.3 JSON): the source and each image",
        || {
            let dir = out.join("sbom");
            fs::create_dir_all(&dir)?;
            let mut text = String::new();
            let source = dir.join(format!("cavea-{version}-source.spdx.json"));
            let mut scan = sh(&["syft", "scan", "dir:."]);
            for exclude in [
                "./target/**",
                "./**/node_modules/**",
                "./fixtures/**",
                "./fuzz/**",
                "./.github/**",
                "./.git/**",
            ] {
                scan.args(["--exclude", exclude]);
            }
            scan.args([
                "--source-name",
                "cavea",
                "--source-version",
                version,
                "-q",
                "-o",
            ])
            .arg(format!("spdx-json={}", source.display()));
            logged(&out, "sbom", &mut scan)?;
            writeln!(text, "source: {}", sbom_summary(&source)?)?;
            for (arch, tag) in &images {
                let file = dir.join(format!("cavea-server-{version}-{arch}.spdx.json"));
                let mut scan = sh(&[
                    "syft",
                    "scan",
                    &format!("docker:{tag}"),
                    "--source-name",
                    "cavea-server",
                ]);
                scan.args(["--source-version", version, "-q", "-o"])
                    .arg(format!("spdx-json={}", file.display()));
                logged(&out, "sbom", &mut scan)?;
                writeln!(text, "image {arch}: {}", sbom_summary(&file)?)?;
            }
            ensure!(
                images.len() == ARCHES.len(),
                "an image is missing, so its SBOM is"
            );
            Ok(Outcome::Pass(text.trim().to_owned()))
        },
    )?;
    list.step("checksums", "SHA256SUMS of the release files", || {
        let mut files = Vec::new();
        for dir in ["npm", "sbom", "images"] {
            let Ok(entries) = fs::read_dir(out.join(dir)) else {
                continue;
            };
            for entry in entries {
                let name = entry?.file_name().to_string_lossy().into_owned();
                if name.ends_with(".tgz") || name.ends_with(".json") || name.ends_with(".tar") {
                    files.push(format!("{dir}/{name}"));
                }
            }
        }
        files.sort();
        ensure!(!files.is_empty(), "nothing to sum");
        let sums = capture(Command::new("sha256sum").args(&files).current_dir(&out))?;
        fs::write(out.join("SHA256SUMS"), format!("{sums}\n"))?;
        Ok(Outcome::Pass(format!("{} files", files.len())))
    })?;
    Ok(images)
}

/// The package count and the count by ecosystem of an SPDX document.
fn sbom_summary(path: &Path) -> Result<String> {
    let doc = read_json(path)?;
    let packages = doc["packages"].as_array().context("no packages")?;
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for package in packages {
        let purl = package["externalRefs"]
            .as_array()
            .and_then(|r| r.iter().find(|r| r["referenceType"] == "purl"))
            .and_then(|r| r["referenceLocator"].as_str())
            .and_then(|p| p.strip_prefix("pkg:"))
            .and_then(|p| p.split('/').next())
            .unwrap_or("other")
            .to_owned();
        *kinds.entry(purl).or_default() += 1;
    }
    Ok(format!(
        "{} packages ({}), {}",
        packages.len(),
        kinds
            .iter()
            .map(|(k, n)| format!("{n} {k}"))
            .collect::<Vec<_>>()
            .join(", "),
        doc["spdxVersion"].as_str().unwrap_or("?")
    ))
}

/// Packs each package's `dist/` and checks what the tarball holds.
fn packages(out: &Path, version: &str, repository: &str) -> Result<Outcome> {
    let root = root();
    let dir = out.join("npm");
    fs::create_dir_all(&dir)?;
    // Built here, from the gated WASM: earlier suites rebuild the WASM and
    // the renderer's package for their own purposes.
    logged(out, "packages", &mut sh(&["cargo", "xtask", "build-web"]))?;
    for (_, path) in PACKAGES {
        logged(
            out,
            "packages",
            sh(&["bun", "run", "build"]).current_dir(root.join(path)),
        )?;
    }
    let mut report = String::new();
    let mut problems = Vec::new();
    for (name, path) in PACKAGES {
        let dist = root.join(path).join("dist");
        ensure!(
            dist.join("package.json").exists(),
            "{path}/dist is not built"
        );
        let packed = capture(
            Command::new("bun")
                .args(["pm", "pack", "--quiet", "--destination"])
                .arg(&dir)
                .current_dir(&dist),
        )?;
        let tarball = dir.join(
            Path::new(packed.lines().last().unwrap_or(""))
                .file_name()
                .context("bun pm pack printed no file")?,
        );
        let files = capture(Command::new("tar").arg("-tzf").arg(&tarball))?;
        let files: Vec<&str> = files.lines().collect();
        fs::write(
            dir.join(format!("{name}.contents.txt")),
            capture(Command::new("tar").arg("-tvzf").arg(&tarball))?,
        )?;
        let pkg: Value = serde_json::from_str(&capture(
            Command::new("tar")
                .arg("-xzOf")
                .arg(&tarball)
                .arg("package/package.json"),
        )?)?;
        let mut bad = |what: String| problems.push(format!("{name}: {what}"));
        for file in &files {
            let f = file.trim_start_matches("package/");
            if !file.starts_with("package/")
                || f.ends_with(".map")
                || f.contains(".test.")
                || f.starts_with("src/")
                || f.starts_with("test/")
                || f.contains("node_modules")
                || f.contains("tsconfig")
                || (f.ends_with(".ts") && !f.ends_with(".d.ts"))
            {
                bad(format!("ships {file}"));
            }
        }
        if pkg["version"] != version {
            bad(format!("version {}", pkg["version"]));
        }
        if pkg["private"] == true {
            bad("private (no LICENSE: npm would refuse it)".to_owned());
        }
        if pkg["repository"]["url"] != format!("git+{repository}.git") {
            bad("repository.url does not name this repository".to_owned());
        }
        if pkg["publishConfig"]["access"] != "public" {
            bad("publishConfig.access is not public".to_owned());
        }
        // Every file `exports`, `main` and `types` name is in the tarball.
        let mut targets = Vec::new();
        collect_strings(&pkg["exports"], &mut targets);
        for key in ["main", "module", "types"] {
            if let Some(t) = pkg[key].as_str() {
                targets.push(t.to_owned());
            }
        }
        for target in targets {
            let file = format!("package/{}", target.trim_start_matches("./"));
            if !files.contains(&file.as_str()) {
                bad(format!("names {target}, which it does not ship"));
            }
        }
        let needed: &[&str] = match name {
            "renderer" => &[
                "index.js",
                "index.d.ts",
                "cavea.min.js",
                "cavea_web_bg.wasm",
                "glyphs.png",
                "OFL.txt",
            ],
            _ => &["index.js", "index.d.ts"],
        };
        for file in needed {
            if !files.contains(&format!("package/{file}").as_str()) {
                bad(format!("does not ship {file}"));
            }
        }
        if name == "react" && pkg["dependencies"]["@cavea/renderer"] != version {
            bad(format!(
                "depends on @cavea/renderer {}",
                pkg["dependencies"]["@cavea/renderer"]
            ));
        }
        let size = fs::metadata(&tarball)?.len();
        writeln!(
            report,
            "{} {}: {} files, {size} B ({})",
            pkg["name"].as_str().unwrap_or(name),
            pkg["version"].as_str().unwrap_or("?"),
            files.len(),
            tarball
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default()
        )?;
    }
    ensure!(
        problems.is_empty(),
        "{}\n{}",
        report.trim(),
        problems.join("\n")
    );
    Ok(Outcome::Pass(report.trim().to_owned()))
}

fn collect_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => out.push(s.clone()),
        Value::Object(map) => map.values().for_each(|v| collect_strings(v, out)),
        Value::Array(items) => items.iter().for_each(|v| collect_strings(v, out)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        for good in ["0.1.0", "1.0.0", "10.20.30", "1.0.0-rc.1", "0.2.0-beta-2"] {
            assert!(valid_version(good), "{good}");
        }
        for bad in [
            "1.0",
            "01.0.0",
            "1.0.0-",
            "v1.0.0",
            "1.0.0+build",
            "1.0.0-rc_1",
            "",
        ] {
            assert!(!valid_version(bad), "{bad}");
        }
        assert!(prerelease("1.0.0-rc.1") && !prerelease("1.0.0"));
        assert_eq!(minor("v1.2.3"), "1.2");
    }
}
