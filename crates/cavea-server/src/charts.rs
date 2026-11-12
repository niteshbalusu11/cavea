//! Charts (08 §3): what a draft must be to be saved, and compiling a draft for
//! validation or publishing. The HTTP handlers are in `http::charts`.
//!
//! Parsing and compiling are CPU work, tens to hundreds of milliseconds for a
//! large chart and more with brotli, so they run on tokio's blocking threads,
//! never on its workers. Compiles are bounded: at most [`MAX_COMPILES`] run at
//! once, and a request waits at most [`COMPILE_TIMEOUT`] for its result.
//!
//! What a version stores for the server's own use is the full artefact
//! (`chart_versions.full_ccf`): the events slice reads it by version id and
//! decodes it with `cavea_format::decode(.., Artefact::Full)`.

use std::fmt;
use std::time::{Duration, Instant};

use cavea_compiler::document::Node;
use cavea_compiler::{ChartDocument, CompileError, Compiled, Finding, SCHEMA};
use tokio::sync::Semaphore;

/// The largest draft document, in bytes (08 §6.2): room for a chart at the
/// design limit of 1,000,000 objects (02 §14) with about 16 bytes of its own
/// per object, while a request's memory stays bounded.
pub const MAX_DRAFT_BYTES: usize = 16 << 20;

/// Compiles that run at once. More wait for a turn within [`COMPILE_TIMEOUT`].
pub const MAX_COMPILES: usize = 2;

/// The longest a validate or publish waits for its compile, waiting for a turn
/// included. A compile still running then keeps its turn until it finishes,
/// so at most [`MAX_COMPILES`] threads ever compile.
pub const COMPILE_TIMEOUT: Duration = Duration::from_secs(60);

/// Brotli at quality 11 with a 4 MiB window (03 §4.6).
const BROTLI_QUALITY: i32 = 11;
const BROTLI_WINDOW: i32 = 22;

static COMPILES: Semaphore = Semaphore::const_new(MAX_COMPILES);

#[derive(Debug)]
pub enum ChartError {
    /// Not a chart document. The parser's message, with line and column; it
    /// may quote the document, so it goes to the caller and never to the log.
    Document(String),
    /// A chart document of a schema this server does not read.
    Schema(u32),
    /// A `\u0000` escape, which PostgreSQL `jsonb` cannot store.
    Nul,
    /// The draft has errors; nothing was compiled.
    Invalid(Vec<Finding>),
    /// The compile did not finish within [`COMPILE_TIMEOUT`].
    Timeout,
    Internal(Box<dyn std::error::Error + Send + Sync>),
}

impl fmt::Display for ChartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Document(_) => f.write_str("not a chart document"),
            Self::Schema(schema) => write!(f, "unsupported schema {schema}"),
            Self::Nul => f.write_str("the document holds a \\u0000 escape"),
            Self::Invalid(findings) => write!(f, "{} findings", findings.len()),
            Self::Timeout => f.write_str("the compile timed out"),
            Self::Internal(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ChartError {}

fn internal(e: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> ChartError {
    ChartError::Internal(e.into())
}

/// Check a draft before it is stored (08 §6.2): it must parse as a chart
/// document of a schema this server reads, and be storable as `jsonb`. It is
/// not compiled: a draft that does not validate yet is still saved. Runs on a
/// blocking thread; its cost is linear in the size, which the route limits.
pub async fn check_draft(bytes: axum::body::Bytes) -> Result<(), ChartError> {
    tokio::task::spawn_blocking(move || parse(&bytes).map(drop))
        .await
        .map_err(internal)?
}

fn parse(bytes: &[u8]) -> Result<ChartDocument, ChartError> {
    // A document is an object (03 §2): serde's derived `Deserialize` would
    // also take an array, its fields by position, and the draft is stored as
    // sent.
    if bytes.iter().find(|b| !b.is_ascii_whitespace()) != Some(&b'{') {
        return Err(ChartError::Document(
            "chart document: expected an object".to_owned(),
        ));
    }
    let document =
        ChartDocument::from_json(bytes).map_err(|e| ChartError::Document(e.to_string()))?;
    // Only schema 1 exists, so migrating on read (03 §2.8) is the identity.
    if document.schema != SCHEMA {
        return Err(ChartError::Schema(document.schema));
    }
    if has_nul_escape(bytes) {
        return Err(ChartError::Nul);
    }
    Ok(document)
}

/// Whether JSON text holds a `\u0000` escape. In valid JSON every backslash
/// starts an escape, so reading escapes in pairs never mistakes `\\u0000`
/// (a backslash, then `u0000`) for one.
fn has_nul_escape(bytes: &[u8]) -> bool {
    let mut rest = bytes;
    while let Some(at) = rest.iter().position(|&b| b == b'\\') {
        let escape = rest.get(at + 1..).unwrap_or_default();
        if escape.starts_with(b"u0000") {
            return true;
        }
        rest = escape.get(1..).unwrap_or_default();
    }
    false
}

/// A compiled draft.
#[derive(Debug)]
pub struct Output {
    pub compiled: Compiled,
    /// The public artefact in brotli, when asked for.
    pub public_br: Option<Vec<u8>>,
    pub compile_time: Duration,
    pub compress_time: Duration,
    /// The background images the chart shows: each asset hash once, with the
    /// first image node naming it. The compiler holds no assets; whether the
    /// workspace holds these is the server's check (03 §2.7, E007).
    pub images: Vec<(Option<u32>, String)>,
}

/// Compile a stored draft, and compress the public artefact if `compress`.
/// Waits for a turn and runs on a blocking thread; the whole is bounded by
/// [`COMPILE_TIMEOUT`].
pub async fn compile(draft: String, compress: bool) -> Result<Output, ChartError> {
    let work = async {
        let turn = COMPILES.acquire().await.map_err(internal)?;
        tokio::task::spawn_blocking(move || {
            let _turn = turn;
            run(draft.as_bytes(), compress)
        })
        .await
        .map_err(internal)?
    };
    tokio::time::timeout(COMPILE_TIMEOUT, work)
        .await
        .map_err(|_| ChartError::Timeout)?
}

fn run(draft: &[u8], compress: bool) -> Result<Output, ChartError> {
    let started = Instant::now();
    let document = parse(draft)?;
    let compiled = cavea_compiler::compile(&document).map_err(|e| match e {
        CompileError::Invalid(findings) => ChartError::Invalid(findings),
        e @ CompileError::Encode(_) => internal(e),
    })?;
    let compile_time = started.elapsed();
    let images = images(&document, &compiled.public)?;
    let started = Instant::now();
    let public_br = if compress {
        Some(brotli(&compiled.public).map_err(internal)?)
    } else {
        None
    };
    Ok(Output {
        compiled,
        public_br,
        compile_time,
        compress_time: started.elapsed(),
        images,
    })
}

/// The asset hashes of the compiled background images, from the public
/// artefact itself (what a buyer's renderer will fetch), each with the first
/// image node of the document that names it.
fn images(
    document: &ChartDocument,
    public: &[u8],
) -> Result<Vec<(Option<u32>, String)>, ChartError> {
    let chart = cavea_format::decode(public, cavea_format::Artefact::Public).map_err(internal)?;
    let mut out: Vec<(Option<u32>, String)> = Vec::new();
    for image in &chart.images {
        let asset = chart.strings.get(image.asset).unwrap_or_default();
        if !out.iter().any(|(_, a)| a == asset) {
            out.push((node_of(&document.nodes, asset), asset.to_owned()));
        }
    }
    Ok(out)
}

fn node_of(nodes: &[Node], asset: &str) -> Option<u32> {
    nodes.iter().find_map(|node| match node {
        Node::Image(image) if image.asset == asset => Some(image.id),
        Node::Section(section) => node_of(&section.children, asset),
        _ => None,
    })
}

fn brotli(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let params = brotli::enc::BrotliEncoderParams {
        quality: BROTLI_QUALITY,
        lgwin: BROTLI_WINDOW,
        size_hint: bytes.len(),
        ..Default::default()
    };
    let mut out = Vec::with_capacity(bytes.len() / 4);
    brotli::BrotliCompress(&mut &bytes[..], &mut out, &params)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_escapes_are_found_only_where_json_has_them() {
        for (text, nul) in [
            (r#"{"a":"\u0000"}"#, true),
            (r#"{"a":"x\\\u0000"}"#, true),
            (r#"{"a":"\\u0000"}"#, false),
            (r#"{"a":"\\\\u0000"}"#, false),
            (r#"{"a":"\u00001"}"#, true),
            (r#"{"a":"\u0001"}"#, false),
            (r#"{"a":"\""}\"#, false),
        ] {
            assert_eq!(has_nul_escape(text.as_bytes()), nul, "{text}");
        }
    }
}
