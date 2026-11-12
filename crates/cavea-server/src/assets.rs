//! Uploaded images (08 §6.2, 12 §3 T7): what an upload must be, and where
//! it is kept.
//!
//! Every upload is hostile. Its type is decided by its bytes, against an
//! allow-list of the formats every renderer host decodes (PNG, JPEG, WebP),
//! never by a header or a file name. The file's structure is walked far
//! enough to find its dimensions and to see that it ends where the format
//! says it ends: a truncated file, or one with data after its end (a
//! polyglot), is refused. Nothing is decoded: the server never holds pixels,
//! so a decompression bomb costs it nothing, and the dimensions in the header
//! are bounded for the browsers that will decode it.
//!
//! The bytes are stored in PostgreSQL (`assets.bytes`, D-10, D-49) by [`put`]
//! and read by [`get`]; S3-compatible storage (SRV-054) would change those
//! two. [`Memory`] bounds the image bytes a node holds at once.

use sqlx::PgPool;
use tracing::Instrument as _;

/// The longest side an image may have, in pixels: room for a detailed floor
/// plan to trace over, while a browser decodes it in at most 256 MiB.
pub const MAX_SIDE: u32 = 8_192;

/// An allowed format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Png,
    Jpeg,
    Webp,
}

impl Kind {
    #[must_use]
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }

    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
        }
    }
}

/// What an upload is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Image {
    pub kind: Kind,
    pub width: u32,
    pub height: u32,
}

/// Why an upload is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// Not PNG, JPEG or WebP: SVG, GIF, HTML, anything else.
    Type,
    /// One of them by its first bytes, but truncated, with data after its
    /// end, or with a header that does not parse.
    Malformed,
    /// A side of 0, or over [`MAX_SIDE`].
    Dimensions,
}

/// Decide what `bytes` are.
pub fn sniff(bytes: &[u8]) -> Result<Image, Refused> {
    let (kind, (width, height)) = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        (Kind::Png, png(bytes).ok_or(Refused::Malformed)?)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        (Kind::Jpeg, jpeg(bytes).ok_or(Refused::Malformed)?)
    } else if bytes.get(..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        (Kind::Webp, webp(bytes).ok_or(Refused::Malformed)?)
    } else {
        return Err(Refused::Type);
    };
    let side = 1..=MAX_SIDE;
    if !side.contains(&width) || !side.contains(&height) {
        return Err(Refused::Dimensions);
    }
    Ok(Image {
        kind,
        width,
        height,
    })
}

fn be16(b: &[u8], at: usize) -> Option<u32> {
    let b = b.get(at..at + 2)?;
    Some(u32::from(b[0]) << 8 | u32::from(b[1]))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(be16(b, at)? << 16 | be16(b, at + 2)?)
}

fn le16(b: &[u8], at: usize) -> Option<u32> {
    let b = b.get(at..at + 2)?;
    Some(u32::from(b[1]) << 8 | u32::from(b[0]))
}

fn le24(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(*b.get(at + 2)?) << 16 | le16(b, at)?)
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(le16(b, at + 2)? << 16 | le16(b, at)?)
}

/// Walk the chunks: `IHDR` first, `IEND` last and ending the file exactly.
/// CRCs are not checked: the browser's decoder does, and a bad one harms
/// nothing here.
fn png(b: &[u8]) -> Option<(u32, u32)> {
    let mut at = 8;
    let mut size = None;
    loop {
        let len = usize::try_from(be32(b, at)?).ok()?;
        let kind = b.get(at + 4..at + 8)?;
        let end = at.checked_add(12)?.checked_add(len)?;
        if end > b.len() {
            return None;
        }
        if at == 8 {
            if kind != b"IHDR" || len != 13 {
                return None;
            }
            size = Some((be32(b, at + 8)?, be32(b, at + 12)?));
        }
        if kind == b"IEND" {
            return (end == b.len()).then_some(size).flatten();
        }
        at = end;
    }
}

/// Walk the marker segments to the first scan, taking the frame's size from
/// its SOF segment; the file must end with EOI. What lies between the first
/// scan and EOI is entropy-coded data that only a decoder can check, so a
/// JPEG can carry other bytes there; the response headers make that harmless
/// (08 §6.2).
fn jpeg(b: &[u8]) -> Option<(u32, u32)> {
    if !b.ends_with(&[0xFF, 0xD9]) {
        return None;
    }
    let mut at = 2;
    let mut size = None;
    loop {
        if *b.get(at)? != 0xFF {
            return None;
        }
        let marker = *b.get(at + 1)?;
        match marker {
            // Fill bytes.
            0xFF => at += 1,
            // Markers without a length.
            0x01 | 0xD0..=0xD7 => at += 2,
            // End of image before any scan.
            0xD8 | 0xD9 => return None,
            _ => {
                let len = usize::try_from(be16(b, at + 2)?).ok()?;
                if len < 2 || at + 2 + len > b.len() {
                    return None;
                }
                // SOF0 to SOF15, but DHT, JPG and DAC.
                if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
                    // Precision, then height and width, inside the segment.
                    if len < 8 {
                        return None;
                    }
                    size = Some((be16(b, at + 7)?, be16(b, at + 5)?));
                }
                if marker == 0xDA {
                    return size;
                }
                at += 2 + len;
            }
        }
    }
}

/// The RIFF container must hold exactly the file; its chunks must fill it.
/// The size is the first chunk's: `VP8 ` (lossy), `VP8L` (lossless) or
/// `VP8X` (extended, the canvas).
fn webp(b: &[u8]) -> Option<(u32, u32)> {
    let riff = usize::try_from(le32(b, 4)?).ok()?;
    if riff.checked_add(8)? != b.len() {
        return None;
    }
    let mut at = 12;
    while at < b.len() {
        let len = usize::try_from(le32(b, at + 4)?).ok()?;
        at = at.checked_add(8 + len + (len & 1))?;
    }
    if at != b.len() {
        return None;
    }
    let data = 20;
    let first = le32(b, 16)?;
    if first < 10 {
        return None;
    }
    match b.get(12..16)? {
        b"VP8 " => {
            if b.get(data + 3..data + 6)? != [0x9D, 0x01, 0x2A] {
                return None;
            }
            Some((le16(b, data + 6)? & 0x3FFF, le16(b, data + 8)? & 0x3FFF))
        }
        b"VP8L" => {
            if *b.get(data)? != 0x2F {
                return None;
            }
            let bits = le32(b, data + 1)?;
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        b"VP8X" => Some((le24(b, data + 4)? + 1, le24(b, data + 7)? + 1)),
        _ => None,
    }
}

/// The content hash of an asset: the first 16 bytes of BLAKE3, as for chart
/// artefacts (03 §4.1).
#[must_use]
pub fn hash(bytes: &[u8]) -> [u8; 16] {
    let mut out = [0; 16];
    out.copy_from_slice(&blake3::hash(bytes).as_bytes()[..16]);
    out
}

const INSERT: &str = "INSERT INTO assets (hash, workspace_id, content_type, size, bytes)
VALUES ($1, $2::uuid, $3, $4, $5)
ON CONFLICT (workspace_id, hash) DO NOTHING
RETURNING true";

/// Store an asset for a workspace. `true` if it is new to the workspace;
/// `false` if the workspace already had these bytes.
pub async fn put(
    db: &PgPool,
    workspace: &str,
    hash: &[u8; 16],
    image: Image,
    bytes: &[u8],
) -> Result<bool, sqlx::Error> {
    let size = i32::try_from(bytes.len()).unwrap_or(i32::MAX);
    let created: Option<bool> = sqlx::query_scalar(INSERT)
        .bind(hash.as_slice())
        .bind(workspace)
        .bind(image.kind.content_type())
        .bind(size)
        .bind(bytes)
        .fetch_optional(db)
        .instrument(cavea_telemetry::db_span("insert_asset"))
        .await?;
    Ok(created.is_some())
}

const SIZE: &str =
    "SELECT content_type, size FROM assets WHERE hash = $1 AND bytes IS NOT NULL LIMIT 1";
const GET: &str = "SELECT bytes FROM assets WHERE hash = $1 AND bytes IS NOT NULL LIMIT 1";
const HELD: &str = "SELECT encode(hash, 'hex') FROM assets
 WHERE workspace_id = $1::uuid AND hash = ANY($2::bytea[])";
const EXISTS: &str = "SELECT EXISTS (SELECT FROM assets WHERE hash = $1)";

/// The content type and size of the asset with this hash, of any workspace:
/// every row with the hash holds the same bytes.
pub async fn size(db: &PgPool, hash: &[u8; 16]) -> Result<Option<(String, i32)>, sqlx::Error> {
    sqlx::query_as(SIZE)
        .bind(hash.as_slice())
        .fetch_optional(db)
        .instrument(cavea_telemetry::db_span("read_asset"))
        .await
}

/// The bytes of the asset with this hash, of any workspace.
pub async fn get(db: &PgPool, hash: &[u8; 16]) -> Result<Option<Vec<u8>>, sqlx::Error> {
    sqlx::query_scalar(GET)
        .bind(hash.as_slice())
        .fetch_optional(db)
        .instrument(cavea_telemetry::db_span("read_asset"))
        .await
}

/// Which of `hashes` (32 lowercase hex digits each) the workspace holds, as
/// they were given.
pub async fn held(
    db: &PgPool,
    workspace: &str,
    hashes: &[String],
) -> Result<Vec<String>, sqlx::Error> {
    let bytes: Vec<Vec<u8>> = hashes
        .iter()
        .filter_map(|h| crate::http::parse_hash(h).map(|b| b.to_vec()))
        .collect();
    sqlx::query_scalar(HELD)
        .bind(workspace)
        .bind(bytes)
        .fetch_all(db)
        .instrument(cavea_telemetry::db_span("check_assets"))
        .await
}

/// The image bytes a node holds in memory at once, per direction: uploads
/// being read and stored, and assets being served. Each is
/// [`MEMORY_FACTOR`] times `CAVEA_ASSET_MAX_BYTES`, so that so many uploads,
/// or responses, of the largest image proceed at once and more cannot grow
/// the process: an upload over the budget is `503 overloaded`; a response
/// waits for room up to [`SERVE_WAIT`], then is `503`. Serving holds its share
/// until the body has been written, so slow readers count.
#[derive(Debug, Clone)]
pub struct Memory {
    pub uploads: std::sync::Arc<tokio::sync::Semaphore>,
    pub serves: std::sync::Arc<tokio::sync::Semaphore>,
    /// Each semaphore's permits, in bytes.
    pub budget: u32,
}

/// See [`Memory`].
pub const MEMORY_FACTOR: u32 = 4;

/// How long a response waits for room in [`Memory::serves`].
pub const SERVE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

impl Memory {
    #[must_use]
    pub fn new(max_bytes: u32) -> Self {
        let budget = max_bytes.saturating_mul(MEMORY_FACTOR);
        let semaphore = || std::sync::Arc::new(tokio::sync::Semaphore::new(budget as usize));
        Self {
            uploads: semaphore(),
            serves: semaphore(),
            budget,
        }
    }
}

/// Whether an asset with this hash exists, without reading it.
pub async fn exists(db: &PgPool, hash: &[u8; 16]) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(EXISTS)
        .bind(hash.as_slice())
        .fetch_one(db)
        .instrument(cavea_telemetry::db_span("read_asset"))
        .await
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = u32::try_from(data.len()).unwrap().to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&[0; 4]);
        out
    }

    /// A structurally whole PNG of `w` × `h` (no pixels: nothing decodes it).
    pub(crate) fn png(w: u32, h: u32) -> Vec<u8> {
        let mut ihdr = w.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        out.extend(chunk(b"IHDR", &ihdr));
        out.extend(chunk(b"IDAT", &[0; 64]));
        out.extend(chunk(b"IEND", &[]));
        out
    }

    #[test]
    fn a_frame_header_too_short_for_its_size_is_refused() {
        let mut short = jpeg(10, 10);
        // SOF2's length 11 → 6: the size would be read past the segment.
        short[23] = 6;
        short.drain(28..33);
        assert_eq!(sniff(&short), Err(Refused::Malformed));
    }

    pub(crate) fn jpeg(w: u16, h: u16) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8];
        out.extend([0xFF, 0xE0, 0, 16]);
        out.extend(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        out.extend([0xFF, 0xC2, 0, 11, 8]);
        out.extend(h.to_be_bytes());
        out.extend(w.to_be_bytes());
        out.extend([1, 1, 0x11, 0]);
        out.extend([0xFF, 0xDA, 0, 8, 1, 1, 0, 0, 63, 0]);
        out.extend([0x12, 0x34, 0xFF, 0x00, 0x56]);
        out.extend([0xFF, 0xD9]);
        out
    }

    fn riff(chunk: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let len = u32::try_from(data.len()).unwrap();
        let mut body = b"WEBP".to_vec();
        body.extend_from_slice(chunk);
        body.extend(len.to_le_bytes());
        body.extend_from_slice(data);
        if data.len() % 2 == 1 {
            body.push(0);
        }
        let mut out = b"RIFF".to_vec();
        out.extend(u32::try_from(body.len()).unwrap().to_le_bytes());
        out.extend(body);
        out
    }

    pub(crate) fn webp_lossy(w: u16, h: u16) -> Vec<u8> {
        let mut data = vec![0x10, 0x02, 0x00, 0x9D, 0x01, 0x2A];
        data.extend(w.to_le_bytes());
        data.extend(h.to_le_bytes());
        data.extend([0; 9]);
        riff(b"VP8 ", &data)
    }

    fn webp_lossless(w: u32, h: u32) -> Vec<u8> {
        let bits = (w - 1) | (h - 1) << 14;
        let mut data = vec![0x2F];
        data.extend(bits.to_le_bytes());
        data.extend([0; 6]);
        riff(b"VP8L", &data)
    }

    fn webp_extended(w: u32, h: u32) -> Vec<u8> {
        let mut data = vec![0x10, 0, 0, 0];
        data.extend(&(w - 1).to_le_bytes()[..3]);
        data.extend(&(h - 1).to_le_bytes()[..3]);
        riff(b"VP8X", &data)
    }

    #[test]
    fn allowed_images_are_recognised_by_their_bytes() {
        let image = |kind, width, height| {
            Ok(Image {
                kind,
                width,
                height,
            })
        };
        assert_eq!(sniff(&png(640, 480)), image(Kind::Png, 640, 480));
        assert_eq!(sniff(&jpeg(4000, 3000)), image(Kind::Jpeg, 4000, 3000));
        assert_eq!(sniff(&webp_lossy(300, 200)), image(Kind::Webp, 300, 200));
        assert_eq!(sniff(&webp_lossless(8192, 1)), image(Kind::Webp, 8192, 1));
        assert_eq!(sniff(&webp_extended(7, 9)), image(Kind::Webp, 7, 9));
    }

    #[test]
    fn everything_else_is_refused() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#;
        let mut polyglot = png(10, 10);
        polyglot.extend_from_slice(b"<html><script>alert(1)</script>");
        let mut jpeg_tail = jpeg(10, 10);
        jpeg_tail.extend_from_slice(b"<script>");
        let mut webp_tail = webp_lossy(10, 10);
        webp_tail.extend_from_slice(b"<script>");
        let full = png(10, 10);
        let cases: [(&[u8], Refused); 14] = [
            (svg, Refused::Type),
            (b"GIF89a\x01\x00\x01\x00", Refused::Type),
            (b"<!doctype html><script>alert(1)</script>", Refused::Type),
            (b"", Refused::Type),
            (b"\x89PNG\r\n\x1a\n", Refused::Malformed),
            (&full[..full.len() - 1], Refused::Malformed),
            (&full[..40], Refused::Malformed),
            (&polyglot, Refused::Malformed),
            (&jpeg_tail, Refused::Malformed),
            (&[0xFF, 0xD8, 0xFF, 0xD9], Refused::Malformed),
            (&webp_tail, Refused::Malformed),
            (&png(0, 10), Refused::Dimensions),
            (&jpeg(8193, 10), Refused::Dimensions),
            (&webp_extended(10, 1 << 14), Refused::Dimensions),
        ];
        for (i, (bytes, refused)) in cases.into_iter().enumerate() {
            assert_eq!(sniff(bytes), Err(refused), "case {i}");
        }
        // A JPEG whose size is in no SOF segment before its scan.
        let mut no_frame = jpeg(10, 10);
        no_frame.drain(20..33);
        assert_eq!(sniff(&no_frame), Err(Refused::Malformed));
    }

    /// Hostile input (12 §3 T7): mutations of valid files (bytes changed,
    /// inserted, removed, the file cut, a length overwritten) never panic or
    /// hang the walkers, and anything still accepted has its sides in range.
    /// A fixed seed, so a failure reproduces.
    #[test]
    fn mutated_files_never_panic() {
        let valid = [
            png(64, 48),
            jpeg(640, 480),
            webp_lossy(30, 20),
            webp_lossless(9, 9),
            webp_extended(7, 9),
        ];
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let mut accepted = 0;
        for i in 0..20_000 {
            let mut b = valid[i % valid.len()].clone();
            for _ in 0..=next() % 3 {
                let at = usize::try_from(next()).unwrap() % (b.len() + 1);
                let byte = next().to_le_bytes()[0];
                match next() % 5 {
                    0 if at < b.len() => b[at] = byte,
                    1 => b.truncate(at),
                    2 => b.insert(at, byte),
                    3 if at < b.len() => drop(b.remove(at)),
                    _ => {
                        let word = next().to_le_bytes();
                        for (slot, w) in b.iter_mut().skip(at).zip(&word[..4]) {
                            *slot = *w;
                        }
                    }
                }
            }
            if let Ok(image) = sniff(&b) {
                accepted += 1;
                assert!((1..=MAX_SIDE).contains(&image.width), "{i}");
                assert!((1..=MAX_SIDE).contains(&image.height), "{i}");
            }
        }
        // Some mutations leave a valid file (a changed pixel byte).
        assert!(accepted > 0);
    }
}
