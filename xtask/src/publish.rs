//! The irreversible half of a release (D-85): push the image for both
//! architectures under one index, sign it, attach the SBOMs as signed
//! attestations, verify all of it from the registry, publish the npm packages
//! with provenance, move the minor tag, create the GitHub release.
//!
//! `cargo xtask release publish [--dir <dir>]` runs it for real, in the
//! release workflow's second job only, from the files the first job built
//! (`target/release/<version>/`). `cargo xtask release --dry-run` rehearses
//! it (`rehearse`): the same steps against a registry container of its own on
//! a free port, signed with a key generated for the run and deleted after it,
//! and `npm publish --dry-run`; it removes the registry, the key and every
//! image it pulled.
//!
//! Every step can run twice: a failed publish job is re-run with GitHub's
//! "Re-run failed jobs", which reuses the first job's files, so the images
//! push to the same digests, signing and attesting again add a second
//! signature, published packages are skipped, an existing GitHub release is
//! left. The one thing it refuses is a version tag in the registry that
//! names other images (a rebuilt release): that needs a new version.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail, ensure};
use serde_json::Value;

use super::root;
use crate::release::{
    ARCHES, Checklist, Outcome, PACKAGES, capture, logged, prerelease, sh, workspace_version,
};

/// The registry the rehearsal pushes to, pinned by digest.
const REGISTRY_IMAGE: &str = "docker.io/library/registry:3@sha256:82f6ff4a7f982960c8aa51617fea3aabc485df6e55cce80d22deb1de36f1bc3c";
/// The issuer of the workflow's identity tokens, which keyless signing binds.
const ISSUER: &str = "https://token.actions.githubusercontent.com";

/// Where the release goes and how it is signed.
struct Target {
    /// `registry/path/name`.
    repository: String,
    version: String,
    out: PathBuf,
    /// The rehearsal's throwaway key pair (`cosign.key`, `cosign.pub`); none
    /// for a real release, which signs keylessly as the workflow.
    key: Option<PathBuf>,
    /// The repository's URL, for the keyless identity and npm provenance.
    source: String,
}

impl Target {
    fn image(&self, reference: &str) -> String {
        format!("{}{reference}", self.repository)
    }

    fn crane(&self, args: &[&str]) -> Command {
        let mut command = sh(&["crane"]);
        command.args(args);
        if self.key.is_some() {
            command.arg("--insecure");
        }
        command
    }

    fn cosign(&self, args: &[&str], verify: bool) -> Command {
        let mut command = sh(&["cosign"]);
        command.args(args);
        match (&self.key, verify) {
            (Some(dir), false) => {
                command
                    .arg("--key")
                    .arg(dir.join("cosign.key"))
                    .args([
                        "--use-signing-config=false",
                        "--tlog-upload=false",
                        "--allow-http-registry",
                    ])
                    .env("COSIGN_PASSWORD", "");
            }
            (Some(dir), true) => {
                command
                    .arg("--key")
                    .arg(dir.join("cosign.pub"))
                    .args(["--insecure-ignore-tlog=true", "--allow-http-registry"]);
            }
            (None, true) => {
                command
                    .arg("--certificate-identity")
                    .arg(identity(&self.source, &self.version))
                    .args(["--certificate-oidc-issuer", ISSUER]);
            }
            (None, false) => {}
        }
        command
    }
}

/// The workflow's identity in a keyless signature: what a customer pins
/// when verifying (apps/docs/content/reference/verify-release.md).
pub fn identity(source: &str, version: &str) -> String {
    let path = source
        .trim_end_matches('/')
        .trim_start_matches("https://github.com/");
    format!("https://github.com/{path}/.github/workflows/release.yml@refs/tags/v{version}")
}

fn read_manifest(dir: &Path) -> Result<Value> {
    let path = dir.join("release.json");
    let text = fs::read_to_string(&path)
        .with_context(|| format!("no {}: run `cargo xtask release` first", path.display()))?;
    Ok(serde_json::from_str(&text)?)
}

fn site_repository() -> Result<String> {
    let site: Value =
        serde_json::from_str(&fs::read_to_string(root().join("apps/docs/site.json"))?)?;
    site["vars"]["repository"]
        .as_str()
        .map(str::to_owned)
        .context("apps/docs/site.json has no vars.repository")
}

/// `release publish [--dir <dir>]`: the real thing, in the workflow only.
pub fn publish(args: &[&str]) -> Result<()> {
    let dir = match args {
        [] => root().join("target/release").join(workspace_version()?),
        ["--dir", dir] => PathBuf::from(dir),
        _ => bail!("usage: cargo xtask release publish [--dir <dir>]"),
    };
    ensure!(
        std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
            && std::env::var_os("ACTIONS_ID_TOKEN_REQUEST_URL").is_some(),
        "publishing runs only in the release workflow's publish job, with `id-token: write` \
         (keyless signing and npm provenance are issued to it); `cargo xtask release --dry-run` \
         rehearses it here"
    );
    let manifest = read_manifest(&dir)?;
    let version = manifest["version"]
        .as_str()
        .context("release.json has no version")?
        .to_owned();
    ensure!(
        manifest["dry_run"] == false,
        "{} is a dry run's output",
        dir.display()
    );
    ensure!(
        manifest["failures"] == 0,
        "the release's checks did not all pass"
    );
    let tag = format!("v{version}");
    ensure!(
        std::env::var("GITHUB_REF_NAME").as_deref() == Ok(tag.as_str()),
        "the workflow runs for {:?}, not {tag}",
        std::env::var("GITHUB_REF_NAME").ok()
    );
    let target = Target {
        repository: manifest["image"]
            .as_str()
            .context("release.json has no image")?
            .to_owned(),
        version: version.clone(),
        out: dir.clone(),
        key: None,
        source: site_repository()?,
    };
    // The images the first job built and saved.
    let mut images = BTreeMap::new();
    for arch in ARCHES {
        let file = dir
            .join("images")
            .join(format!("cavea-server-{version}-{arch}.tar"));
        capture(sh(&["docker", "load", "-q", "-i"]).arg(&file))?;
        let local = manifest["images"][arch]
            .as_str()
            .context("release.json lacks an image")?;
        images.insert(arch.to_owned(), local.to_owned());
    }
    let mut list = Checklist::new(dir.clone(), false, Vec::new());
    steps(&mut list, &target, &images, prerelease(&version))?;
    list.write()?;
    println!("\nCavea {version} is published.");
    Ok(())
}

/// The publishing steps, against `target`.
fn steps(
    list: &mut Checklist,
    target: &Target,
    images: &BTreeMap<String, String>,
    pre: bool,
) -> Result<()> {
    let version = &target.version;
    let out = target.out.clone();
    let rehearsal = target.key.is_some();
    let mut digests: BTreeMap<String, String> = BTreeMap::new();
    list.step("push", "Push each architecture's image", || {
        let mut text = String::new();
        for arch in ARCHES {
            let local = images
                .get(arch)
                .with_context(|| format!("no {arch} image"))?;
            let remote = target.image(&format!(":{version}-{arch}"));
            capture(&mut sh(&["docker", "tag", local, &remote]))?;
            logged(&out, "push", &mut sh(&["docker", "push", &remote]))?;
            let digest = capture(&mut target.crane(&["digest", &remote]))?;
            writeln!(text, "linux/{arch}: {}@{digest}", target.repository)?;
            digests.insert(arch.to_owned(), digest);
            let _ = Command::new("docker")
                .args(["rmi", &remote])
                .stdout(Stdio::null())
                .status();
        }
        Ok(Outcome::Pass(text.trim().to_owned()))
    })?;
    let mut index = String::new();
    list.step(
        "index",
        "One multi-architecture index under the version's tag",
        || {
            let tagged = target.image(&format!(":{version}"));
            let mine: Vec<&String> = digests.values().collect();
            ensure!(mine.len() == ARCHES.len(), "an image was not pushed");
            // A tag already there must name these very images (a re-run of the
            // publish job); anything else is another build of this version.
            if let Ok(existing) = capture(&mut target.crane(&["manifest", &tagged])) {
                let existing: Value = serde_json::from_str(&existing)?;
                let theirs: Vec<&str> = existing["manifests"]
                    .as_array()
                    .map(|m| m.iter().filter_map(|m| m["digest"].as_str()).collect())
                    .unwrap_or_default();
                ensure!(
                    mine.iter().all(|d| theirs.contains(&d.as_str())),
                    "{tagged} already names other images: a version is published once; release the \
                 next patch version instead"
                );
            }
            let mut append = target.crane(&["index", "append", "-t", &tagged]);
            for digest in &mine {
                append.args(["-m", &target.image(&format!("@{digest}"))]);
            }
            logged(&out, "index", &mut append)?;
            index = capture(&mut target.crane(&["digest", &tagged]))?;
            let manifest: Value =
                serde_json::from_str(&capture(&mut target.crane(&["manifest", &tagged]))?)?;
            let platforms: Vec<String> = manifest["manifests"]
                .as_array()
                .map(|m| {
                    m.iter()
                        .map(|m| {
                            format!(
                                "{}/{}",
                                m["platform"]["os"].as_str().unwrap_or("?"),
                                m["platform"]["architecture"].as_str().unwrap_or("?")
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            for arch in ARCHES {
                ensure!(
                    platforms.contains(&format!("linux/{arch}")),
                    "the index lacks linux/{arch}: {platforms:?}"
                );
            }
            Ok(Outcome::Pass(format!(
                "{tagged} = {index} ({})",
                platforms.join(", ")
            )))
        },
    )?;
    let at = |digest: &str| target.image(&format!("@{digest}"));
    list.step("sign", "Sign the index and each image", || {
        let how = if rehearsal {
            "with the run's throwaway key"
        } else {
            "keyless, as the release workflow"
        };
        logged(
            &out,
            "sign",
            &mut target.cosign(&["sign", "--yes", "--recursive", &at(&index)], false),
        )?;
        Ok(Outcome::Pass(format!("{} {how}", at(&index))))
    })?;
    list.step("attest", "Attach the SBOMs as signed attestations", || {
        let sbom = out.join("sbom");
        let source = sbom.join(format!("cavea-{version}-source.spdx.json"));
        let mut text = String::new();
        for (arch, digest) in &digests {
            for predicate in [
                sbom.join(format!("cavea-server-{version}-{arch}.spdx.json")),
                source.clone(),
            ] {
                let path = predicate.to_string_lossy().into_owned();
                logged(
                    &out,
                    "attest",
                    &mut target.cosign(
                        &[
                            "attest",
                            "--yes",
                            "--type",
                            "spdxjson",
                            "--predicate",
                            &path,
                            &at(digest),
                        ],
                        false,
                    ),
                )?;
            }
            writeln!(text, "linux/{arch}: the image's SBOM and the source SBOM")?;
        }
        Ok(Outcome::Pass(text.trim().to_owned()))
    })?;
    list.step("verify", "Verify from the registry: signatures, attestations, pull by digest", || {
        let mut text = String::new();
        logged(&out, "verify", &mut target.cosign(&["verify", &at(&index)], true))?;
        for (arch, digest) in &digests {
            logged(&out, "verify", &mut target.cosign(&["verify", &at(digest)], true))?;
            logged(&out, "verify", &mut target.cosign(&["verify-attestation", "--type", "spdxjson", &at(digest)], true))?;
            // The image pulled by digest is the one built.
            let pulled = at(digest);
            logged(&out, "verify", &mut sh(&["docker", "pull", "--platform", &format!("linux/{arch}"), &pulled]))?;
            let theirs = capture(&mut sh(&["docker", "image", "inspect", "-f", "{{.Id}}", &pulled]))?;
            let local = images.get(arch.as_str()).context("no local image")?;
            let ours = capture(&mut sh(&["docker", "image", "inspect", "-f", "{{.Id}}", local]))?;
            let _ = Command::new("docker").args(["rmi", &pulled]).stdout(Stdio::null()).stderr(Stdio::null()).status();
            ensure!(theirs == ours, "linux/{arch}: pulled {theirs}, built {ours}");
            writeln!(text, "linux/{arch}: signature and SBOM attestation verify; pulled by digest, image {ours}")?;
        }
        Ok(Outcome::Pass(format!("index signature verifies\n{}", text.trim())))
    })?;
    list.step("npm", "Publish the npm packages with provenance", || {
        let dist_tag = if pre { "next" } else { "latest" };
        let mut text = String::new();
        for (name, _) in PACKAGES {
            let tarball = fs::read_dir(out.join("npm"))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| {
                    let file = p
                        .file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    file.ends_with(&format!("{name}-{version}.tgz"))
                })
                .with_context(|| format!("no tarball for {name}"))?;
            let pkg: Value = serde_json::from_str(&capture(
                Command::new("tar")
                    .arg("-xzOf")
                    .arg(&tarball)
                    .arg("package/package.json"),
            )?)?;
            let id = format!("{}@{version}", pkg["name"].as_str().unwrap_or(name));
            let file = tarball.to_string_lossy().into_owned();
            if rehearsal {
                logged(
                    &out,
                    "npm",
                    &mut sh(&[
                        "npm",
                        "publish",
                        &file,
                        "--dry-run",
                        "--access",
                        "public",
                        "--tag",
                        dist_tag,
                    ]),
                )?;
                writeln!(
                    text,
                    "{id}: `npm publish --dry-run` accepts it (tag {dist_tag})"
                )?;
            } else if capture(&mut sh(&["npm", "view", &id, "version"]))
                .is_ok_and(|v| v == *version)
            {
                writeln!(text, "{id}: already published (a re-run)")?;
            } else {
                logged(
                    &out,
                    "npm",
                    &mut sh(&[
                        "npm",
                        "publish",
                        &file,
                        "--provenance",
                        "--access",
                        "public",
                        "--tag",
                        dist_tag,
                    ]),
                )?;
                writeln!(text, "{id}: published with provenance (tag {dist_tag})")?;
            }
        }
        Ok(Outcome::Pass(text.trim().to_owned()))
    })?;
    list.step(
        "minor-tag",
        "Point the minor version's image tag at this release",
        || {
            if pre {
                return Ok(Outcome::NotApplicable(
                    "a pre-release moves no tag".to_owned(),
                ));
            }
            let minor: Vec<&str> = version.split('.').take(2).collect();
            let minor = minor.join(".");
            logged(
                &out,
                "minor-tag",
                &mut target.crane(&["tag", &at(&index), &minor]),
            )?;
            Ok(Outcome::Pass(format!(
                "{} -> {index}",
                target.image(&format!(":{minor}"))
            )))
        },
    )?;
    list.step("github-release", "GitHub release with the notes, SBOMs and checksums", || {
        let tag = format!("v{version}");
        let mut files: Vec<String> = vec!["SHA256SUMS".to_owned(), "checklist.md".to_owned()];
        for entry in fs::read_dir(out.join("sbom"))? {
            files.push(format!("sbom/{}", entry?.file_name().to_string_lossy()));
        }
        let mut create = sh(&["gh", "release", "create", &tag, "--verify-tag", "--title", &format!("Cavea {version}"), "--notes-file", "notes.md"]);
        if pre {
            create.arg("--prerelease");
        }
        create.args(&files).current_dir(&out);
        if rehearsal {
            return Ok(Outcome::NotApplicable(format!(
                "not in a dry run (irreversible): `gh release create {tag} --verify-tag …` with {}",
                files.join(", ")
            )));
        }
        if capture(&mut sh(&["gh", "release", "view", &tag])).is_ok() {
            return Ok(Outcome::Pass(format!("{tag} exists (a re-run)")));
        }
        logged(&out, "github-release", &mut create)?;
        Ok(Outcome::Pass(format!("{tag} with {}", files.join(", "))))
    })?;
    Ok(())
}

/// The dry run's rehearsal of `steps`: a registry container of its own, a
/// throwaway key, `npm publish --dry-run`; all removed afterwards.
pub fn rehearse(list: &mut Checklist, out: &Path, images: &BTreeMap<String, String>) -> Result<()> {
    let version = workspace_version()?;
    if images.len() != ARCHES.len() {
        list.record(
            "rehearsal",
            "Publishing rehearsed locally",
            Outcome::Fail("an image was not built".to_owned()),
            0.0,
        );
        return Ok(());
    }
    let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let work = root()
        .join("target/tmp")
        .join(format!("release-rehearsal-{}", std::process::id()));
    fs::create_dir_all(&work)?;
    fs::write(
        work.join("compose.yaml"),
        format!(
            "services:\n  registry:\n    image: {REGISTRY_IMAGE}\n    ports: [\"127.0.0.1:{port}:5000\"]\n"
        ),
    )?;
    let project = format!("cavea-release-{}", std::process::id());
    let compose = |args: &[&str]| {
        let mut command = sh(&["docker", "compose", "-p", &project, "-f"]);
        command.arg(work.join("compose.yaml")).args(args);
        command
    };
    let result = (|| -> Result<()> {
        logged(out, "rehearsal", &mut compose(&["up", "-d", "--wait"]))?;
        logged(
            out,
            "rehearsal",
            sh(&["cosign", "generate-key-pair", "--output-key-prefix"])
                .arg(work.join("cosign"))
                .env("COSIGN_PASSWORD", ""),
        )?;
        let site: Value =
            serde_json::from_str(&fs::read_to_string(root().join("apps/docs/site.json"))?)?;
        let name = site["vars"]["image"]
            .as_str()
            .unwrap_or("cavea/cavea-server:0");
        let path = name.rsplit_once(':').map_or(name, |(r, _)| r);
        let path = path.split_once('/').map_or(path, |(_, p)| p);
        let target = Target {
            repository: format!("127.0.0.1:{port}/{path}"),
            version: version.clone(),
            out: out.to_path_buf(),
            key: Some(work.clone()),
            source: site_repository()?,
        };
        println!(
            "\n=== Rehearsal of the publishing against {} (throwaway key)",
            target.repository
        );
        steps(list, &target, images, prerelease(&version))
    })();
    // Removed whatever happened: the registry, its volume, the key.
    let _ = compose(&["down", "-v", "--remove-orphans"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = fs::remove_dir_all(&work);
    if let Err(error) = result {
        list.record(
            "rehearsal",
            "Publishing rehearsed locally",
            Outcome::Fail(format!("{error:#}")),
            0.0,
        );
    }
    let site: Value =
        serde_json::from_str(&fs::read_to_string(root().join("apps/docs/site.json"))?)?;
    println!(
        "\nNot done in a dry run, because they cannot be undone: pushing to {}, signing \
         keylessly as {} (issuer {ISSUER}), publishing the packages to npm with provenance, \
         moving the minor tag there, and creating the GitHub release. The git tag v{version} \
         is the owner's, pushed before the workflow runs.",
        site["vars"]["image"].as_str().unwrap_or("the registry"),
        identity(&site_repository()?, &version),
    );
    Ok(())
}
