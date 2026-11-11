//! The `cavea` command-line tool for developer machines and CI: compile,
//! validate and inspect charts, and generate the synthetic presets. See
//! `docs/01-architecture.md` §2. Benchmarks come later (TST-011).
//!
//! Exit codes: 0 success (warnings allowed); 1 the input is invalid (a chart
//! document with errors or that does not parse, a corrupt CCF file); 2 usage
//! error (unknown command, option or preset); 3 any other failure, such as a
//! file that cannot be read or written.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Context as _, Result};
use cavea_compiler::{ChartDocument, CompileError, Finding, Severity};
use cavea_format::{Artefact, FLAG_FULL, HEADER_LEN};
use cavea_testkit::presets::{self, DEFAULT_SEED, PRESETS, WAITING};

const INVALID: u8 = 1;
const USAGE: u8 = 2;
const FAILURE: u8 = 3;

const HELP: &str = "\
cavea: compile, validate, inspect and generate seat charts

usage:
  cavea compile [<document.json>] [-o <dir>]
  cavea validate [<document.json>] [--json]
  cavea inspect <chart.ccf>
  cavea gen <preset> [--seed <n>] [-o <file>]
  cavea gen --list

A missing path or `-` reads standard input. `cavea <command> --help` explains
a command.

exit codes: 0 success, 1 invalid input, 2 usage error, 3 other failure";

const COMPILE_HELP: &str = "\
usage: cavea compile [<document.json>] [-o <dir>]

Compiles a chart document and writes both artefacts to <dir> (default: the
current directory), named by content hash as the server serves them:
<hash>.ccf (public) and <hash>.full.ccf (full). Prints statistics and any
warnings. On errors prints the findings, writes nothing and exits 1.";

const VALIDATE_HELP: &str = "\
usage: cavea validate [<document.json>] [--json]

Prints the document's findings (docs/03-chart-format.md §3.2), or with --json
the list in their JSON form. Exits 0 when there is no error (warnings allowed),
1 otherwise.";

const INSPECT_HELP: &str = "\
usage: cavea inspect <chart.ccf>

Decodes a public or full artefact and prints its header, content hash, counts,
bounds, and the raw and brotli size of the file and of each chunk. A file that
does not decode gives the decoder's error and exit 1.";

const GEN_HELP: &str = "\
usage: cavea gen <preset> [--seed <n>] [-o <file>]
       cavea gen --list

Writes a synthetic preset's chart document as JSON to standard output or
<file>. The same preset and seed always give the same bytes. The default seed
is 1. --list lists the presets with their seat counts.";

/// A usage error: exit 2 with the message and a pointer to `--help`.
struct Usage(String);

enum Failure {
    Usage(Usage),
    Other(anyhow::Error),
}

impl From<Usage> for Failure {
    fn from(usage: Usage) -> Self {
        Self::Usage(usage)
    }
}

impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Self {
        Self::Other(error)
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => ExitCode::from(code),
        Err(Failure::Usage(Usage(message))) => {
            err(&format!("cavea: {message}\nRun `cavea --help` for usage."));
            ExitCode::from(USAGE)
        }
        Err(Failure::Other(error)) => {
            err(&format!("cavea: {error:#}"));
            ExitCode::from(FAILURE)
        }
    }
}

fn run(args: &[String]) -> Result<u8, Failure> {
    let (command, rest) = match args.split_first() {
        Some((command, rest)) => (command.as_str(), rest),
        None => return Err(Usage("no command given".into()).into()),
    };
    let help = match command {
        "compile" => COMPILE_HELP,
        "validate" => VALIDATE_HELP,
        "inspect" => INSPECT_HELP,
        "gen" => GEN_HELP,
        "-h" | "--help" | "help" => HELP,
        "-V" | "--version" => {
            out(&format!("cavea {}\n", env!("CARGO_PKG_VERSION")))?;
            return Ok(0);
        }
        _ => return Err(Usage(format!("unknown command {command:?}")).into()),
    };
    if help == HELP || rest.iter().any(|a| a == "-h" || a == "--help") {
        out(&format!("{help}\n"))?;
        return Ok(0);
    }
    match command {
        "compile" => compile(&Args::parse(rest, &["-o"], &[])?),
        "validate" => validate(&Args::parse(rest, &[], &["--json"])?),
        "inspect" => inspect(&Args::parse(rest, &[], &[])?),
        _ => generate(&Args::parse(rest, &["--seed", "-o"], &["--list"])?),
    }
}

/// A command's positional arguments and options.
struct Args<'a> {
    positional: Vec<&'a str>,
    options: BTreeMap<&'a str, &'a str>,
}

impl<'a> Args<'a> {
    /// `valued` options take the next argument as their value; `switches`
    /// take none. `-` is positional (standard input).
    fn parse(args: &'a [String], valued: &[&str], switches: &[&str]) -> Result<Self, Usage> {
        let mut out = Self {
            positional: Vec::new(),
            options: BTreeMap::new(),
        };
        let mut args = args.iter().map(String::as_str);
        while let Some(arg) = args.next() {
            let value = if valued.contains(&arg) {
                args.next()
                    .ok_or_else(|| Usage(format!("{arg} needs a value")))?
            } else if switches.contains(&arg) {
                ""
            } else if arg.starts_with('-') && arg != "-" {
                return Err(Usage(format!("unknown option {arg:?}")));
            } else {
                out.positional.push(arg);
                continue;
            };
            if out.options.insert(arg, value).is_some() {
                return Err(Usage(format!("{arg} given twice")));
            }
        }
        Ok(out)
    }

    /// At most one positional argument.
    fn path(&self) -> Result<Option<&'a str>, Usage> {
        match self.positional.as_slice() {
            [] => Ok(None),
            [path] => Ok(Some(path)),
            [_, extra, ..] => Err(Usage(format!("unexpected argument {extra:?}"))),
        }
    }

    fn has(&self, option: &str) -> bool {
        self.options.contains_key(option)
    }
}

/// Reads `path`, or standard input for `None` or `-`.
fn read_input(path: Option<&str>) -> Result<Vec<u8>> {
    match path {
        None | Some("-") => {
            let mut bytes = Vec::new();
            io::stdin()
                .read_to_end(&mut bytes)
                .context("cannot read standard input")?;
            Ok(bytes)
        }
        Some(path) => fs::read(path).with_context(|| format!("cannot read {path}")),
    }
}

/// Parses and compiles a document: the compiled chart or its errors, or why
/// it could not be parsed or encoded.
fn compile_document(
    bytes: &[u8],
) -> Result<
    (
        ChartDocument,
        Result<cavea_compiler::Compiled, Vec<Finding>>,
    ),
    Invalid,
> {
    let doc = ChartDocument::from_json(bytes).map_err(|e| match e.finding() {
        Some(finding) => Invalid::Findings(vec![finding]),
        None => Invalid::Message(e.to_string()),
    })?;
    match cavea_compiler::compile(&doc) {
        Ok(compiled) => Ok((doc, Ok(compiled))),
        Err(CompileError::Invalid(findings)) => Ok((doc, Err(findings))),
        Err(e @ CompileError::Encode(_)) => Err(Invalid::Message(e.to_string())),
    }
}

/// Why a document did not compile.
enum Invalid {
    Findings(Vec<Finding>),
    Message(String),
}

fn compile(args: &Args) -> Result<u8, Failure> {
    let bytes = read_input(args.path()?)?;
    let started = Instant::now();
    let (doc, compiled) = match compile_document(&bytes) {
        Ok(result) => result,
        Err(invalid) => return Ok(report_invalid(invalid)),
    };
    let compiled = match compiled {
        Ok(compiled) => compiled,
        Err(findings) => return Ok(report_invalid(Invalid::Findings(findings))),
    };
    let elapsed = started.elapsed();
    let dir = PathBuf::from(args.options.get("-o").copied().unwrap_or("."));
    fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let public = write_artefact(&dir, &compiled.public_hash, "ccf", &compiled.public)?;
    let full = write_artefact(&dir, &compiled.full_hash, "full.ccf", &compiled.full)?;

    let s = compiled.stats;
    let mut text = format!(
        "compiled {:?} in {:.1} ms\n",
        doc.name,
        elapsed.as_secs_f64() * 1e3
    );
    for (name, value) in [
        ("seats", s.seats),
        ("rows", s.rows),
        ("segments", s.segments),
        ("sections", s.sections),
        ("floors", s.floors),
        ("chunks", s.chunks),
        ("label rules", s.label_rules),
        ("label exceptions", s.label_exceptions),
    ] {
        let _ = writeln!(text, "  {name:<18}{value:>10}");
    }
    for (name, bytes, path) in [
        ("public", &compiled.public, &public),
        ("full", &compiled.full, &full),
    ] {
        let _ = writeln!(
            text,
            "  {name:<8}{:>10} B raw {:>10} B brotli  {}",
            bytes.len(),
            brotli_len(bytes)?,
            path.display()
        );
    }
    out(&text)?;
    print_findings(&compiled.findings);
    Ok(0)
}

fn write_artefact(dir: &Path, hash: &[u8; 16], suffix: &str, bytes: &[u8]) -> Result<PathBuf> {
    let path = dir.join(format!("{}.{suffix}", hex(hash)));
    fs::write(&path, bytes).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(path)
}

fn validate(args: &Args) -> Result<u8, Failure> {
    let bytes = read_input(args.path()?)?;
    let findings = match compile_document(&bytes) {
        Ok((_, Ok(compiled))) => compiled.findings,
        Ok((_, Err(findings))) | Err(Invalid::Findings(findings)) => findings,
        Err(Invalid::Message(message)) => return Ok(report_invalid(Invalid::Message(message))),
    };
    let errors = findings
        .iter()
        .filter(|f| f.severity() == Severity::Error)
        .count();
    if args.has("--json") {
        let json = serde_json::to_string_pretty(&findings).context("cannot write findings")?;
        out(&format!("{json}\n"))?;
    } else {
        let mut text = String::new();
        for finding in &findings {
            let _ = writeln!(text, "{} {finding}", finding.severity().as_str());
        }
        let warnings = findings.len().saturating_sub(errors);
        let _ = writeln!(text, "{errors} errors, {warnings} warnings");
        out(&text)?;
    }
    Ok(if errors == 0 { 0 } else { INVALID })
}

/// Prints why a document is invalid to standard error; returns exit code 1.
fn report_invalid(invalid: Invalid) -> u8 {
    match invalid {
        Invalid::Findings(findings) => {
            print_findings(&findings);
            err("cavea: the document has errors; nothing was compiled");
        }
        Invalid::Message(message) => err(&format!("cavea: {message}")),
    }
    INVALID
}

fn print_findings(findings: &[Finding]) {
    for finding in findings {
        err(&format!("{} {finding}", finding.severity().as_str()));
    }
}

fn inspect(args: &Args) -> Result<u8, Failure> {
    let path = args.path()?;
    if path.is_none() {
        return Err(Usage("inspect needs a .ccf file".into()).into());
    }
    let bytes = read_input(path)?;
    let full = le::<2>(&bytes, 6) & usize::from(FLAG_FULL) != 0;
    let artefact = if full {
        Artefact::Full
    } else {
        Artefact::Public
    };
    let chart = match cavea_format::decode(&bytes, artefact) {
        Ok(chart) => chart,
        Err(e) => {
            err(&format!("cavea: {}: {e}", path.unwrap_or("-")));
            return Ok(INVALID);
        }
    };
    let string = |i: u32| chart.strings.get(i).unwrap_or("");
    let meta = &chart.meta;
    let mut text = String::new();
    let _ = writeln!(
        text,
        "{} artefact, CCF version {}, {} B raw, {} B brotli",
        if full { "full" } else { "public" },
        le::<2>(&bytes, 4),
        bytes.len(),
        brotli_len(&bytes)?,
    );
    let _ = writeln!(
        text,
        "content hash  {}",
        hex(&cavea_format::content_hash(&bytes).unwrap_or_default())
    );
    let _ = writeln!(text, "name          {:?}", string(meta.name));
    let _ = writeln!(text, "compiler      {}", string(meta.compiler));
    let [x0, y0, x1, y1] = meta.bounds.map(|q| f64::from(q) / 4.0);
    let _ = writeln!(
        text,
        "bounds        ({x0}, {y0}) to ({x1}, {y1}) cu, {} × {} cu",
        x1 - x0,
        y1 - y0
    );
    let _ = writeln!(text, "counts");
    for (name, count) in [
        ("floors", chart.floors.len()),
        ("sections", chart.sections.len()),
        ("render chunks", chart.chunks.len()),
        ("rows", chart.rows.len()),
        ("segments", chart.segments.len()),
        ("seats", chart.seat_pos.len()),
        ("categories", chart.categories.len()),
        ("label rules", chart.label_rules.len()),
        ("label exceptions", chart.label_exceptions.len()),
        ("objects", chart.objects.len()),
        ("tables", chart.tables.len()),
        ("mesh vertices", chart.mesh.vertices.len()),
        ("mesh indices", chart.mesh.indices.len()),
        ("mesh draws", chart.mesh.draws.len()),
        ("texts", chart.texts.len()),
        ("icons", chart.icons.len()),
        ("images", chart.images.len()),
        ("strings", chart.strings.len()),
    ] {
        let _ = writeln!(text, "  {name:<18}{count:>10}");
    }
    let _ = writeln!(
        text,
        "chunks        {:>10} {:>10} {:>7}",
        "raw B", "brotli B", "share"
    );
    // The file decoded, so its directory is well formed.
    for i in 0..le::<4>(&bytes, 8) {
        let entry = HEADER_LEN + 16 * i;
        let tag = bytes
            .get(entry..entry + 4)
            .map(String::from_utf8_lossy)
            .unwrap_or_default();
        let offset = le::<4>(&bytes, entry + 8);
        let payload = bytes
            .get(offset..offset + le::<4>(&bytes, entry + 12))
            .unwrap_or_default();
        let _ = writeln!(
            text,
            "  {tag:<12}{:>10} {:>10} {:>6.1}%",
            payload.len(),
            brotli_len(payload)?,
            100.0 * payload.len() as f64 / bytes.len() as f64
        );
    }
    out(&text)?;
    Ok(0)
}

fn generate(args: &Args) -> Result<u8, Failure> {
    if args.has("--list") {
        if !args.positional.is_empty() {
            return Err(Usage("--list takes no preset".into()).into());
        }
        let mut text = String::new();
        for p in &PRESETS {
            let _ = writeln!(text, "{:<14}{:>9} seats  {}", p.name, p.seats, p.shape);
        }
        for (name, note) in WAITING {
            let _ = writeln!(text, "{name:<14}{:>15}  {note}", "not yet");
        }
        out(&text)?;
        return Ok(0);
    }
    let Some(name) = args.path()? else {
        return Err(Usage("gen needs a preset; `cavea gen --list` lists them".into()).into());
    };
    let Some(preset) = presets::find(name) else {
        let reason = match WAITING.iter().find(|(waiting, _)| *waiting == name) {
            Some((_, note)) => format!("preset {name:?} is not available yet: {note}"),
            None => format!("unknown preset {name:?}; `cavea gen --list` lists them"),
        };
        return Err(Usage(reason).into());
    };
    let seed = match args.options.get("--seed") {
        Some(seed) => seed
            .parse()
            .map_err(|_| Usage(format!("--seed {seed:?} is not a whole number")))?,
        None => DEFAULT_SEED,
    };
    let mut json = preset
        .generate(seed)
        .to_json()
        .context("cannot serialise the document")?;
    json.push(b'\n');
    match args.options.get("-o") {
        Some(path) => fs::write(path, &json).with_context(|| format!("cannot write {path}"))?,
        None => out_bytes(&json)?,
    }
    Ok(0)
}

/// Writes to standard output; a failure (a closed pipe, a full disk) is an
/// error, exit 3, not a panic.
fn out(text: &str) -> Result<()> {
    out_bytes(text.as_bytes())
}

fn out_bytes(bytes: &[u8]) -> Result<()> {
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(bytes)
        .and_then(|()| stdout.flush())
        .context("cannot write standard output")
}

/// Writes a line to standard error, best effort: there is nowhere left to
/// report a failure to.
fn err(text: &str) {
    let _ = writeln!(io::stderr().lock(), "{text}");
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Brotli size at quality 11 with a 4 MiB window, as the server stores
/// compiled charts (`docs/03-chart-format.md` §4.6).
fn brotli_len(bytes: &[u8]) -> Result<usize> {
    let mut out = Vec::new();
    {
        let mut writer = brotli::CompressorWriter::new(&mut out, 4096, 11, 22);
        writer.write_all(bytes).context("brotli failed")?;
    }
    Ok(out.len())
}

/// The little-endian integer of `N` bytes at `at`, or 0 past the end.
fn le<const N: usize>(bytes: &[u8], at: usize) -> usize {
    bytes.get(at..at + N).map_or(0, |b| {
        b.iter().rev().fold(0, |n, &b| n << 8 | usize::from(b))
    })
}
