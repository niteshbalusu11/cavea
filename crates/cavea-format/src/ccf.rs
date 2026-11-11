//! The CCF container: header, chunk directory and chunk payloads
//! (`docs/03-chart-format.md` §4.1, §4.4).

use cavea_core::limits::{MAX_CATEGORIES, MAX_FLOORS, MAX_OBJECTS, MAX_SECTIONS};

use crate::chart::{Chart, Full, LabelException, Mesh, Strings};
use crate::codec::{Cur, Field, put_varint, unzigzag, zigzag};
use crate::{Error, ErrorKind, Tag, tags::*};

pub const MAGIC: [u8; 4] = *b"CCF1";
pub const VERSION: u16 = 1;
/// Header flag bit 0: the full (server) artefact.
pub const FLAG_FULL: u16 = 1;
/// Header length. The content hash covers `file[HEADER_LEN..]`.
pub const HEADER_LEN: usize = 32;
const DIR_ENTRY: usize = 16;

const RAW: u8 = 0;
const VARINT: u8 = 1;
const DELTA: u8 = 2;
const RLE: u8 = 3;
const STRINGS: u8 = 4;

/// Every known chunk with its encoding, in the order the writer emits them.
/// The last [`FULL_ONLY`] belong to the full artefact only.
const CHUNKS: [(Tag, u8); 22] = [
    (META, RAW),
    (STRS, STRINGS),
    (CATS, RAW),
    (FLRS, RAW),
    (SECT, RAW),
    (CHNK, RAW),
    (ROWS, RAW),
    (RSEG, RAW),
    (LBLR, RAW),
    (LBLX, VARINT),
    (SPOS, DELTA),
    (SCAT, RLE),
    (SFLG, RLE),
    (OBJS, RAW),
    (TBLS, RAW),
    (MESH, RAW),
    (TEXT, RAW),
    (ICON, RAW),
    (IMGS, RAW),
    (SOID, DELTA),
    (SRNK, DELTA),
    (OPRI, RAW),
];
const FULL_ONLY: usize = 3;
const PUBLIC: usize = CHUNKS.len() - FULL_ONLY;

/// Which artefact to write, or which data to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Artefact {
    /// For renderers: no `oid`s, ranks or priorities.
    Public,
    /// For the server: flag bit 0 and the full-only chunks.
    Full,
}

/// The stored content hash. Not verified by [`decode`].
pub fn content_hash(file: &[u8]) -> Option<[u8; 16]> {
    file.get(16..32)?.try_into().ok()
}

/// Writes the content hash into a file produced by [`encode`]: the first 16
/// bytes of the BLAKE3 hash of `file[HEADER_LEN..]`, computed by the caller.
pub fn set_content_hash(file: &mut [u8], hash: [u8; 16]) -> Option<()> {
    file.get_mut(16..32)?.copy_from_slice(&hash);
    Some(())
}

/// Encodes `chart` as the given artefact, after [`Chart::check`].
/// Deterministic: the same chart value always gives the same bytes. The content hash field is left zero (see [`set_content_hash`]).
pub fn encode(chart: &Chart, artefact: Artefact) -> Result<Vec<u8>, Error> {
    chart.check()?;
    write(chart, artefact)
}

/// [`encode`] without the consistency check.
pub(crate) fn write(chart: &Chart, artefact: Artefact) -> Result<Vec<u8>, Error> {
    let (n, flags) = match artefact {
        Artefact::Public => (PUBLIC, 0),
        Artefact::Full => (CHUNKS.len(), FLAG_FULL),
    };
    let full = match (artefact, &chart.full) {
        (Artefact::Full, None) => return Err(Error::at(SOID, ErrorKind::Missing)),
        (_, f) => f.as_ref(),
    };
    container(flags, CHUNKS.get(..n).unwrap_or_default(), |tag, out| {
        write_chunk(chart, full, tag, out)
    })
}

/// Lays out header, directory and payloads, `fill` writing each payload.
pub(crate) fn container(
    flags: u16,
    chunks: &[(Tag, u8)],
    mut fill: impl FnMut(Tag, &mut Vec<u8>) -> Result<(), ErrorKind>,
) -> Result<Vec<u8>, Error> {
    let too_large = Error::at_file(ErrorKind::TooLarge);
    let base = HEADER_LEN + chunks.len() * DIR_ENTRY;
    let mut dir = Vec::with_capacity(chunks.len() * DIR_ENTRY);
    let mut body = Vec::new();
    for &(tag, encoding) in chunks {
        body.resize(body.len().next_multiple_of(4), 0);
        let start = body.len();
        fill(tag, &mut body).map_err(|kind| Error::at(tag, kind))?;
        let offset = u32::try_from(base + start).map_err(|_| too_large)?;
        let len = u32::try_from(body.len() - start).map_err(|_| too_large)?;
        dir.extend_from_slice(&tag);
        dir.extend_from_slice(&[encoding, 0, 0, 0]);
        dir.extend_from_slice(&offset.to_le_bytes());
        dir.extend_from_slice(&len.to_le_bytes());
    }
    let total = u32::try_from(base + body.len()).map_err(|_| too_large)?;
    let count = u32::try_from(chunks.len()).map_err(|_| too_large)?;
    let mut out = Vec::with_capacity(total as usize);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&total.to_le_bytes());
    out.extend_from_slice(&[0; 16]);
    out.extend_from_slice(&dir);
    out.extend_from_slice(&body);
    Ok(out)
}

fn put_count(out: &mut Vec<u8>, n: usize) -> Result<(), ErrorKind> {
    put_varint(out, u32::try_from(n).map_err(|_| ErrorKind::TooLarge)?);
    Ok(())
}

fn put_raw<T: Field>(out: &mut Vec<u8>, items: &[T]) -> Result<(), ErrorKind> {
    put_count(out, items.len())?;
    for x in items {
        x.write(out);
    }
    Ok(())
}

/// Zigzag LEB128 of differences modulo 2^32, the first from 0.
fn put_delta(
    out: &mut Vec<u8>,
    values: impl ExactSizeIterator<Item = u32>,
) -> Result<(), ErrorKind> {
    put_count(out, values.len())?;
    let mut prev = 0u32;
    for v in values {
        put_varint(out, zigzag(v.wrapping_sub(prev) as i32));
        prev = v;
    }
    Ok(())
}

fn put_rle<T: Copy + PartialEq + Into<u32>>(
    out: &mut Vec<u8>,
    values: &[T],
) -> Result<(), ErrorKind> {
    put_count(out, values.len())?;
    let mut rest = values;
    while let Some(&v) = rest.first() {
        let run = rest.iter().take_while(|&&x| x == v).count();
        put_varint(out, v.into());
        put_count(out, run)?;
        rest = rest.get(run..).unwrap_or_default();
    }
    Ok(())
}

fn write_chunk(
    chart: &Chart,
    full: Option<&Full>,
    tag: Tag,
    out: &mut Vec<u8>,
) -> Result<(), ErrorKind> {
    let full = || full.ok_or(ErrorKind::Missing);
    match tag {
        META => put_raw(out, core::slice::from_ref(&chart.meta)),
        STRS => {
            let s = &chart.strings;
            put_count(out, s.ends.len())?;
            put_raw_items(out, &s.ends);
            out.extend_from_slice(s.bytes.as_bytes());
            Ok(())
        }
        CATS => put_raw(out, &chart.categories),
        FLRS => put_raw(out, &chart.floors),
        SECT => put_raw(out, &chart.sections),
        CHNK => put_raw(out, &chart.chunks),
        ROWS => put_raw(out, &chart.rows),
        RSEG => put_raw(out, &chart.segments),
        LBLR => put_raw(out, &chart.label_rules),
        LBLX => {
            put_count(out, chart.label_exceptions.len())?;
            for x in &chart.label_exceptions {
                put_varint(out, x.idx);
                put_varint(out, x.label);
            }
            Ok(())
        }
        SPOS => {
            put_count(out, chart.seat_pos.len())?;
            let mut seats = chart.seat_pos.iter();
            for row in &chart.rows {
                // Each row starts from (0, 0): its first seat is absolute.
                let mut prev = [0i32; 2];
                for p in seats.by_ref().take(usize::from(row.seat_count)) {
                    put_varint(out, zigzag(p[0].wrapping_sub(prev[0])));
                    put_varint(out, zigzag(p[1].wrapping_sub(prev[1])));
                    prev = *p;
                }
            }
            Ok(())
        }
        SCAT => put_rle(out, &chart.seat_category),
        SFLG => put_rle(out, &chart.seat_flags),
        OBJS => put_raw(out, &chart.objects),
        TBLS => put_raw(out, &chart.tables),
        MESH => {
            let m = &chart.mesh;
            put_count(out, m.vertices.len())?;
            put_count(out, m.indices.len())?;
            put_count(out, m.draws.len())?;
            put_raw_items(out, &m.vertices);
            put_raw_items(out, &m.indices);
            put_raw_items(out, &m.draws);
            Ok(())
        }
        TEXT => put_raw(out, &chart.texts),
        ICON => put_raw(out, &chart.icons),
        IMGS => put_raw(out, &chart.images),
        SOID => put_delta(out, full()?.oids.iter().copied()),
        SRNK => put_delta(out, full()?.ranks.iter().map(|&r| u32::from(r))),
        OPRI => put_raw(out, &full()?.priorities),
        _ => Ok(()),
    }
}

fn put_raw_items<T: Field>(out: &mut Vec<u8>, items: &[T]) {
    for x in items {
        x.write(out);
    }
}

/// Decodes and validates a CCF file (`docs/03-chart-format.md` §4.4). With
/// [`Artefact::Public`] it reads either artefact and skips the full-only
/// chunks; with [`Artefact::Full`] it requires the full artefact. The content
/// hash is not verified.
pub fn decode(file: &[u8], want: Artefact) -> Result<Chart, Error> {
    let head = Error::at_file;
    let mut c = Cur(file);
    let h: [u8; HEADER_LEN] = c.array().map_err(head)?;
    let u16_at = |i: usize| {
        h.get(i..i + 2)
            .and_then(|b| b.try_into().ok())
            .map(u16::from_le_bytes)
    };
    let u32_at = |i: usize| {
        h.get(i..i + 4)
            .and_then(|b| b.try_into().ok())
            .map(u32::from_le_bytes)
    };
    if h.get(..4) != Some(&MAGIC[..]) {
        return Err(head(ErrorKind::BadMagic));
    }
    if u16_at(4) != Some(VERSION) {
        return Err(head(ErrorKind::UnsupportedVersion));
    }
    let flags = u16_at(6).unwrap_or_default();
    let count = u32_at(8).unwrap_or_default();
    if u32_at(12).map(u64::from) != Some(file.len() as u64) {
        return Err(head(ErrorKind::LengthMismatch));
    }
    let n = PUBLIC
        + match want {
            Artefact::Public => 0,
            Artefact::Full if flags & FLAG_FULL != 0 => FULL_ONLY,
            Artefact::Full => return Err(head(ErrorKind::NotFull)),
        };
    let dir_end = HEADER_LEN as u64 + u64::from(count) * DIR_ENTRY as u64;
    if dir_end > file.len() as u64 {
        return Err(head(ErrorKind::OutOfBounds));
    }

    // Payloads must follow the directory in directory order without overlap.
    let mut found: [Option<&[u8]>; CHUNKS.len()] = [None; CHUNKS.len()];
    let mut prev_end = dir_end;
    for _ in 0..count {
        let e: [u8; DIR_ENTRY] = c.array().map_err(head)?;
        let [
            t0,
            t1,
            t2,
            t3,
            encoding,
            _,
            _,
            _,
            o0,
            o1,
            o2,
            o3,
            l0,
            l1,
            l2,
            l3,
        ] = e;
        let tag = [t0, t1, t2, t3];
        let err = |kind| Error::at(tag, kind);
        let offset = u64::from(u32::from_le_bytes([o0, o1, o2, o3]));
        let end = offset + u64::from(u32::from_le_bytes([l0, l1, l2, l3]));
        if encoding > STRINGS {
            return Err(err(ErrorKind::UnknownEncoding));
        }
        if offset % 4 != 0 {
            return Err(err(ErrorKind::Unaligned));
        }
        if offset < dir_end || end > file.len() as u64 {
            return Err(err(ErrorKind::OutOfBounds));
        }
        if offset < prev_end {
            return Err(err(ErrorKind::Overlap));
        }
        prev_end = end;
        let Some(i) = CHUNKS.iter().take(n).position(|&(t, _)| t == tag) else {
            continue; // unknown, or full-only and not wanted
        };
        if CHUNKS.get(i).map(|&(_, enc)| enc) != Some(encoding) {
            return Err(err(ErrorKind::WrongEncoding));
        }
        let slot = found.get_mut(i).ok_or(err(ErrorKind::OutOfBounds))?;
        if slot.is_some() {
            return Err(err(ErrorKind::Duplicate));
        }
        *slot = file.get(offset as usize..end as usize);
    }

    let get = |tag: Tag| -> Result<Cur<'_>, Error> {
        let i = CHUNKS.iter().position(|&(t, _)| t == tag);
        i.and_then(|i| found.get(i).copied().flatten())
            .map(Cur)
            .ok_or(Error::at(tag, ErrorKind::Missing))
    };
    let at = |tag: Tag| move |kind| Error::at(tag, kind);

    let meta = raw(get(META)?, 1).map_err(at(META))?;
    let [meta]: [crate::Meta; 1] =
        <[_; 1]>::try_from(meta).map_err(|_| Error::at(META, ErrorKind::BadCount))?;
    let seats = meta.seat_count as usize;
    if seats > MAX_OBJECTS as usize {
        return Err(Error::at(META, ErrorKind::OverLimit));
    }
    let rows = raw(get(ROWS)?, usize::MAX).map_err(at(ROWS))?;
    let seat_pos = positions(get(SPOS)?, seats, &rows).map_err(at(SPOS))?;
    let full = match want {
        Artefact::Public => None,
        Artefact::Full => Some(Full {
            oids: deltas(get(SOID)?).map_err(at(SOID))?,
            ranks: deltas(get(SRNK)?).map_err(at(SRNK))?,
            priorities: raw(get(OPRI)?, MAX_SECTIONS as usize).map_err(at(OPRI))?,
        }),
    };
    let chart = Chart {
        meta,
        strings: strings(get(STRS)?).map_err(at(STRS))?,
        categories: raw(get(CATS)?, MAX_CATEGORIES as usize).map_err(at(CATS))?,
        floors: raw(get(FLRS)?, MAX_FLOORS as usize).map_err(at(FLRS))?,
        sections: raw(get(SECT)?, MAX_SECTIONS as usize).map_err(at(SECT))?,
        chunks: raw(get(CHNK)?, usize::MAX).map_err(at(CHNK))?,
        rows,
        segments: raw(get(RSEG)?, usize::MAX).map_err(at(RSEG))?,
        label_rules: raw(get(LBLR)?, usize::MAX).map_err(at(LBLR))?,
        label_exceptions: exceptions(get(LBLX)?).map_err(at(LBLX))?,
        seat_pos,
        seat_category: runs(get(SCAT)?, seats).map_err(at(SCAT))?,
        seat_flags: runs(get(SFLG)?, seats).map_err(at(SFLG))?,
        objects: raw(get(OBJS)?, MAX_OBJECTS as usize).map_err(at(OBJS))?,
        tables: raw(get(TBLS)?, usize::MAX).map_err(at(TBLS))?,
        mesh: mesh(get(MESH)?).map_err(at(MESH))?,
        texts: raw(get(TEXT)?, usize::MAX).map_err(at(TEXT))?,
        icons: raw(get(ICON)?, usize::MAX).map_err(at(ICON))?,
        images: raw(get(IMGS)?, usize::MAX).map_err(at(IMGS))?,
        full,
    };
    chart.check()?;
    Ok(chart)
}

fn items<T: Field>(c: &mut Cur<'_>, n: usize) -> Result<Vec<T>, ErrorKind> {
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(T::read(c)?);
    }
    Ok(v)
}

/// A `Raw` chunk: count, then exactly that many records.
fn raw<T: Field>(mut c: Cur<'_>, max: usize) -> Result<Vec<T>, ErrorKind> {
    let n = c.count(max, T::SIZE)?;
    let v = items(&mut c, n)?;
    c.end()?;
    Ok(v)
}

fn strings(mut c: Cur<'_>) -> Result<Strings, ErrorKind> {
    let n = c.count(usize::MAX, 4)?;
    let ends: Vec<u32> = items(&mut c, n)?;
    if ends.windows(2).any(|w| w.first() > w.get(1))
        || ends.last().map_or(0, |&e| e as usize) != c.len()
    {
        return Err(ErrorKind::BadString);
    }
    let text = core::str::from_utf8(c.0).map_err(|_| ErrorKind::BadString)?;
    if !ends.iter().all(|&e| text.is_char_boundary(e as usize)) {
        return Err(ErrorKind::BadString);
    }
    Ok(Strings {
        bytes: text.into(),
        ends,
    })
}

fn exceptions(mut c: Cur<'_>) -> Result<Vec<LabelException>, ErrorKind> {
    let n = c.count(MAX_OBJECTS as usize, 2)?;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(LabelException {
            idx: c.varint()?,
            label: c.varint()?,
        });
    }
    c.end()?;
    Ok(v)
}

/// `SPOS`: per row, the first seat's position from (0, 0), then each seat's
/// difference from the previous one, x then y, zigzag LEB128.
fn positions(
    mut c: Cur<'_>,
    seats: usize,
    rows: &[crate::Row],
) -> Result<Vec<[i32; 2]>, ErrorKind> {
    if c.count(MAX_OBJECTS as usize, 2)? != seats {
        return Err(ErrorKind::BadCount);
    }
    let mut v = Vec::with_capacity(seats);
    for row in rows {
        let mut p = [0i32; 2];
        for _ in 0..row.seat_count {
            // Positions are differences modulo 2^32, so wrapping is exact.
            p[0] = p[0].wrapping_add(unzigzag(c.varint()?));
            p[1] = p[1].wrapping_add(unzigzag(c.varint()?));
            v.push(p);
        }
        if v.len() > seats {
            return Err(ErrorKind::BadCount);
        }
    }
    if v.len() != seats {
        return Err(ErrorKind::BadCount);
    }
    c.end()?;
    Ok(v)
}

fn deltas<T: TryFrom<u32>>(mut c: Cur<'_>) -> Result<Vec<T>, ErrorKind> {
    let n = c.count(MAX_OBJECTS as usize, 1)?;
    let mut v = Vec::with_capacity(n);
    let mut prev = 0u32;
    for _ in 0..n {
        prev = prev.wrapping_add(unzigzag(c.varint()?) as u32);
        v.push(T::try_from(prev).map_err(|_| ErrorKind::BadValue)?);
    }
    c.end()?;
    Ok(v)
}

/// `Run-length`: the expanded count, then (value, run) pairs, runs ≥ 1.
fn runs<T: TryFrom<u32> + Copy>(mut c: Cur<'_>, seats: usize) -> Result<Vec<T>, ErrorKind> {
    if c.count(MAX_OBJECTS as usize, 0)? != seats {
        return Err(ErrorKind::BadCount);
    }
    let mut v = Vec::with_capacity(seats);
    while v.len() < seats {
        let value = T::try_from(c.varint()?).map_err(|_| ErrorKind::BadValue)?;
        let run = c.varint()? as usize;
        if run == 0 || run > seats - v.len() {
            return Err(ErrorKind::BadCount);
        }
        v.resize(v.len() + run, value);
    }
    c.end()?;
    Ok(v)
}

fn mesh(mut c: Cur<'_>) -> Result<Mesh, ErrorKind> {
    let nv = c.varint()? as usize;
    let ni = c.varint()? as usize;
    let nd = c.varint()? as usize;
    let size = nv
        .checked_mul(crate::Vertex::SIZE)
        .zip(ni.checked_mul(4))
        .zip(nd.checked_mul(crate::Draw::SIZE))
        .and_then(|((a, b), d)| a.checked_add(b)?.checked_add(d));
    if size != Some(c.len()) {
        return Err(ErrorKind::BadCount);
    }
    Ok(Mesh {
        vertices: items(&mut c, nv)?,
        indices: items(&mut c, ni)?,
        draws: items(&mut c, nd)?,
    })
}
