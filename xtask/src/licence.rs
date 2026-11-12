//! `cargo xtask licence`: the private licence tool (12 §7.2, D-80). It lives
//! here because xtask is never shipped: not in the server, its image, the
//! `cavea` CLI or any package. It works offline.
//!
//! - `keygen --out <dir>`: a new signing key in `<dir>/signing-key` (mode
//!   0600, never overwritten) and its public half in `<dir>/public-key`,
//!   with the line to add to `crates/cavea-licence/src/keys.rs`.
//! - `issue --key <file> --id <id> --customer <name> --plan <plan>
//!   --expires <YYYY-MM-DD> [--issued <YYYY-MM-DD>] [--grace-days <n>]
//!   [--domain <pattern>]… [--feature <name>]… [--max-workspaces <n>]
//!   (--out <file> | --dry-run)`: sign a licence. The key is read from the
//!   file only, never from an argument, and is refused if others can read it.
//! - `inspect <file>`: what a licence file says, without checking it.
//! - `verify <file> [--public-key <file>] [--at <YYYY-MM-DD>]`: check it
//!   against the keys this build trusts (or the one given) and say its state.

use std::fs;
use std::io::Write as _;
use std::path::Path;

use anyhow::{Context as _, Result, bail, ensure};
use cavea_licence::sign::SigningKey;
use cavea_licence::{DAY_MS, Date, Licence, PublicKey, Standing, Verifier, valid_domain};

const USAGE: &str = "usage: cargo xtask licence <keygen --out <dir> | issue --key <file> --id <id> \
--customer <name> --plan <plan> --expires <YYYY-MM-DD> [--issued <YYYY-MM-DD>] [--grace-days <n>] \
[--domain <pattern>]… [--feature <name>]… [--max-workspaces <n>] (--out <file> | --dry-run) | \
inspect <file> | verify <file> [--public-key <file>] [--at <YYYY-MM-DD>]>";

pub fn licence(args: &[&str]) -> Result<()> {
    match args {
        ["keygen", rest @ ..] => {
            let options = Options::parse(rest, &["--out"])?;
            let dir = options.one("--out")?.context(USAGE)?;
            let (key_id, line) = keygen(Path::new(dir))?;
            println!("signing key: {dir}/signing-key (keep it offline; back it up)");
            println!("public key:  {dir}/public-key");
            println!("key id:      {key_id}");
            println!("add to TRUSTED in crates/cavea-licence/src/keys.rs:\n    \"{line}\",");
            Ok(())
        }
        ["issue", rest @ ..] => {
            let options = Options::parse(
                rest,
                &[
                    "--key",
                    "--id",
                    "--customer",
                    "--plan",
                    "--expires",
                    "--issued",
                    "--grace-days",
                    "--domain",
                    "--feature",
                    "--max-workspaces",
                    "--out",
                    "--dry-run",
                ],
            )?;
            let dry_run = options.flag("--dry-run");
            let out = options.one("--out")?;
            ensure!(
                dry_run != out.is_some(),
                "give exactly one of --out <file> and --dry-run"
            );
            let request = Request::from(&options)?;
            let key = read_key(Path::new(
                options.one("--key")?.context("--key is required")?,
            ))?;
            let (licence, file) = issue(&key, &request)?;
            print!("{}", describe(&licence));
            match out {
                Some(out) => {
                    write_new(Path::new(out), &file, 0o644)?;
                    println!("written: {out}");
                }
                None => println!("dry run: nothing written"),
            }
            Ok(())
        }
        ["inspect", file] => {
            let bytes = read_licence(Path::new(file))?;
            let licence = cavea_licence::inspect_unverified(&bytes)
                .map_err(|e| anyhow::anyhow!("not a licence file: {e}"))?;
            println!("NOT VERIFIED: what the file says.");
            print!("{}", describe(&licence));
            Ok(())
        }
        ["verify", file, rest @ ..] => {
            let options = Options::parse(rest, &["--public-key", "--at"])?;
            let verifier = match options.one("--public-key")? {
                Some(path) => Verifier::new([read_public_key(Path::new(path))?]),
                None => Verifier::production(),
            };
            let at = match options.one("--at")? {
                Some(day) => parse_date("--at", day)?,
                None => today(),
            };
            let bytes = read_licence(Path::new(file))?;
            let licence = verifier.verify(&bytes).map_err(|e| {
                anyhow::anyhow!(
                    "invalid: {e} (keys trusted: {})",
                    describe_keys(&verifier.key_ids())
                )
            })?;
            print!("{}", describe(&licence));
            let state = match licence.standing(at.start_ms() + DAY_MS / 2) {
                Standing::Valid => "valid",
                Standing::Grace => "grace",
                Standing::Expired => "expired",
            };
            println!("state on {at}: {state}");
            Ok(())
        }
        _ => bail!(USAGE),
    }
}

/// `--name value` pairs (a name may repeat) and `--flag`s.
struct Options<'a>(Vec<(&'a str, Option<&'a str>)>);

impl<'a> Options<'a> {
    fn parse(args: &[&'a str], known: &[&str]) -> Result<Self> {
        let mut pairs = Vec::new();
        let mut args = args.iter();
        while let Some(&name) = args.next() {
            ensure!(known.contains(&name), "unknown option {name}\n{USAGE}");
            if name == "--dry-run" {
                pairs.push((name, None));
            } else {
                let value = args
                    .next()
                    .with_context(|| format!("{name} needs a value"))?;
                pairs.push((name, Some(*value)));
            }
        }
        Ok(Self(pairs))
    }

    fn one(&self, name: &str) -> Result<Option<&'a str>> {
        let mut found = self.0.iter().filter(|(n, _)| *n == name).map(|(_, v)| *v);
        let first = found.next().flatten();
        ensure!(found.next().is_none(), "{name} given twice");
        Ok(first)
    }

    fn all(&self, name: &str) -> Vec<&'a str> {
        self.0
            .iter()
            .filter(|(n, _)| *n == name)
            .filter_map(|(_, v)| *v)
            .collect()
    }

    fn flag(&self, name: &str) -> bool {
        self.0.iter().any(|(n, _)| *n == name)
    }
}

/// What to issue, checked.
struct Request {
    id: String,
    customer: String,
    plan: String,
    features: Vec<String>,
    domains: Vec<String>,
    max_workspaces: Option<u32>,
    issued: Date,
    expires: Date,
    grace_days: u32,
}

/// Names (an id, a plan, a feature): lowercase letters, digits, `_`, `-`.
fn check_name(what: &str, value: &str) -> Result<String> {
    ensure!(
        !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'),
        "{what} {value:?} must be 1 to 64 lowercase letters, digits, `_` or `-`"
    );
    Ok(value.to_owned())
}

fn parse_date(what: &str, value: &str) -> Result<Date> {
    Date::parse(value).with_context(|| format!("{what} {value:?} is not a date (YYYY-MM-DD)"))
}

fn today() -> Date {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
    Date::of_ms(ms)
}

impl Request {
    fn from(options: &Options<'_>) -> Result<Self> {
        let required = |name: &str| -> Result<&str> {
            options
                .one(name)?
                .with_context(|| format!("{name} is required\n{USAGE}"))
        };
        let customer = required("--customer")?.trim().to_owned();
        ensure!(
            !customer.is_empty() && customer.len() <= 200,
            "--customer must be 1 to 200 characters"
        );
        let issued = match options.one("--issued")? {
            Some(day) => parse_date("--issued", day)?,
            None => today(),
        };
        let expires = parse_date("--expires", required("--expires")?)?;
        ensure!(
            expires >= issued,
            "--expires {expires} is before --issued {issued}"
        );
        ensure!(
            expires.days() - issued.days() <= 3 * 366,
            "--expires is more than three years after --issued"
        );
        let grace_days = match options.one("--grace-days")? {
            Some(n) => n
                .parse()
                .ok()
                .filter(|n| *n <= 365)
                .context("--grace-days must be 0 to 365")?,
            None => 30,
        };
        let domains = options.all("--domain");
        for domain in &domains {
            ensure!(
                valid_domain(domain),
                "--domain {domain:?} must be a lowercase host name, or `*.` and one"
            );
        }
        let max_workspaces = options
            .one("--max-workspaces")?
            .map(|n| n.parse::<u32>().ok().filter(|n| *n > 0))
            .map(|n| n.context("--max-workspaces must be a positive number"))
            .transpose()?;
        Ok(Self {
            id: check_name("--id", required("--id")?)?,
            customer,
            plan: check_name("--plan", required("--plan")?)?,
            features: options
                .all("--feature")
                .into_iter()
                .map(|f| check_name("--feature", f))
                .collect::<Result<_>>()?,
            domains: domains.into_iter().map(str::to_owned).collect(),
            max_workspaces,
            issued,
            expires,
            grace_days,
        })
    }
}

fn issue(key: &SigningKey, request: &Request) -> Result<(Licence, Vec<u8>)> {
    let payload = serde_json::json!({
        "licence_id": request.id,
        "customer": request.customer,
        "plan": request.plan,
        "features": request.features,
        "domains": request.domains,
        "max_workspaces": request.max_workspaces,
        "issued_at": request.issued.to_string(),
        "expires_at": request.expires.to_string(),
        "grace_days": request.grace_days,
    });
    let (licence, file) = key
        .issue(payload.to_string().as_bytes())
        .map_err(|e| anyhow::anyhow!("cannot issue: {e}"))?;
    // What was written must verify with the key's public half.
    Verifier::new([key.public()])
        .verify(&file)
        .map_err(|e| anyhow::anyhow!("the issued licence does not verify: {e}"))?;
    Ok((licence, file))
}

fn describe(licence: &Licence) -> String {
    let list = |items: &[String]| {
        if items.is_empty() {
            "(none)".to_owned()
        } else {
            items.join(", ")
        }
    };
    format!(
        "licence:        {}\ncustomer:       {}\nplan:           {}\nfeatures:       {}\n\
domains:        {}\nmax workspaces: {}\nissued:         {}\nexpires:        {} (end of day, UTC)\n\
grace:          {} days, to the end of {}\nkey id:         {}\n",
        licence.id,
        licence.customer,
        licence.plan,
        list(&licence.features),
        list(&licence.domains),
        licence
            .max_workspaces
            .map_or("unlimited".to_owned(), |n| n.to_string()),
        licence.issued_at,
        licence.expires_at,
        licence.grace_days,
        licence.last_grace_day(),
        licence.key_id,
    )
}

fn describe_keys(ids: &[String]) -> String {
    if ids.is_empty() {
        "none".to_owned()
    } else {
        ids.join(", ")
    }
}

/// Create `path` with `mode`; never replace a file.
fn write_new(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, mode);
    #[cfg(not(unix))]
    let _ = mode;
    let mut file = options.open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            anyhow::anyhow!("{} exists: refusing to overwrite it", path.display())
        } else {
            anyhow::anyhow!("cannot create {}: {e}", path.display())
        }
    })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("cannot write {}", path.display()))
}

const PUBLIC_HEADER: &str = "# Cavea licence public key";

/// A new key pair in `dir`: the key id and the public key's base64 line.
fn keygen(dir: &Path) -> Result<(String, String)> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(dir)
        .with_context(|| format!("cannot create {}", dir.display()))?;
    let (secret, public) = (dir.join("signing-key"), dir.join("public-key"));
    for path in [&secret, &public] {
        ensure!(
            !path.exists(),
            "{} exists: refusing to overwrite a key",
            path.display()
        );
    }
    let (key, text) = SigningKey::generate().map_err(|e| anyhow::anyhow!("{e}"))?;
    write_new(&secret, text.as_bytes(), 0o600)?;
    let line = key.public().to_base64();
    let id = key.public().id();
    let public_text = format!("{PUBLIC_HEADER}, id {id}.\n{line}\n");
    write_new(&public, public_text.as_bytes(), 0o644)?;
    Ok((id, line))
}

/// The signing key in `path`, refused when anyone but its owner may read it.
fn read_key(path: &Path) -> Result<SigningKey> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = fs::metadata(path)
            .with_context(|| format!("cannot read {}", path.display()))?
            .permissions()
            .mode();
        ensure!(
            mode & 0o077 == 0,
            "{} may be read by others (mode {:o}): chmod 600 it first",
            path.display(),
            mode & 0o777
        );
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    SigningKey::from_file(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
}

fn read_public_key(path: &Path) -> Result<PublicKey> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .and_then(PublicKey::from_base64)
        .with_context(|| format!("{} holds no public key", path.display()))
}

fn read_licence(path: &Path) -> Result<Vec<u8>> {
    let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    ensure!(
        bytes.len() <= cavea_licence::MAX_BYTES,
        "{} is larger than a licence file can be",
        path.display()
    );
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]

    use std::path::PathBuf;

    use super::*;

    /// A fresh directory under the temporary directory, removed by the test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cavea-xtask-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// keygen, issue, verify against the public key file; the key is never
    /// overwritten, never read when others can, and leaves the directory with
    /// the test.
    #[test]
    fn keygen_issue_verify_round_trip() {
        let dir = scratch("licence");
        let keys = dir.join("keys");
        let (id, line) = keygen(&keys).unwrap();
        assert!(keygen(&keys).is_err(), "a second keygen overwrote the key");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(keys.join("signing-key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let key = read_key(&keys.join("signing-key")).unwrap();
        let public = read_public_key(&keys.join("public-key")).unwrap();
        assert_eq!((public.id(), public.to_base64()), (id, line));
        let args = [
            "--key",
            "k",
            "--id",
            "lic_2027_0001",
            "--customer",
            "Acme Tickets Ltd",
            "--plan",
            "growth",
            "--issued",
            "2027-01-10",
            "--expires",
            "2028-01-10",
            "--domain",
            "tickets.acme.example",
            "--domain",
            "*.acme.example",
            "--feature",
            "embed",
            "--dry-run",
        ];
        let options = Options::parse(
            &args,
            &[
                "--key",
                "--id",
                "--customer",
                "--plan",
                "--issued",
                "--expires",
                "--domain",
                "--feature",
                "--dry-run",
            ],
        )
        .unwrap();
        let request = Request::from(&options).unwrap();
        let (licence, file) = issue(&key, &request).unwrap();
        assert_eq!(licence.grace_days, 30);
        let out = dir.join("licence.json");
        write_new(&out, &file, 0o644).unwrap();
        assert!(
            write_new(&out, &file, 0o644).is_err(),
            "a licence was overwritten"
        );
        let read = read_licence(&out).unwrap();
        assert_eq!(Verifier::new([public]).verify(&read).unwrap(), licence);
        assert!(Verifier::production().verify(&read).is_err());
        assert_eq!(
            cavea_licence::inspect_unverified(&read).unwrap().customer,
            "Acme Tickets Ltd"
        );
        // A key others may read is refused.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let path = keys.join("signing-key");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(read_key(&path).is_err());
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn requests_are_checked() {
        let known = [
            "--id",
            "--customer",
            "--plan",
            "--issued",
            "--expires",
            "--domain",
            "--grace-days",
            "--max-workspaces",
        ];
        let base = [
            "--id",
            "lic_1",
            "--customer",
            "C",
            "--plan",
            "starter",
            "--issued",
            "2027-01-10",
            "--expires",
            "2028-01-10",
        ];
        let with = |extra: &[&'static str]| {
            let mut args = base.to_vec();
            args.extend_from_slice(extra);
            Options::parse(&args, &known).and_then(|o| Request::from(&o).map(drop))
        };
        assert!(with(&[]).is_ok());
        for bad in [
            &["--domain", "Tickets.example.com"][..],
            &["--domain", "https://x.example"],
            &["--grace-days", "400"],
            &["--max-workspaces", "0"],
            &["--id", "again"],
            &["--unknown", "x"],
        ] {
            assert!(with(bad).is_err(), "{bad:?}");
        }
        let swap = |from: &'static str, to: &'static str| {
            let args: Vec<&str> = base
                .iter()
                .map(|a| if *a == from { to } else { a })
                .collect();
            Options::parse(&args, &known).and_then(|o| Request::from(&o).map(drop))
        };
        assert!(
            swap("2028-01-10", "2026-01-10").is_err(),
            "expires before issued"
        );
        assert!(swap("2028-01-10", "2028-02-30").is_err(), "no such day");
        assert!(
            swap("2028-01-10", "2031-01-10").is_err(),
            "more than three years"
        );
        assert!(swap("starter", "Starter").is_err());
    }
}
