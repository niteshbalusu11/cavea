use cavea_core::label::{AlphaOverflow, Scheme};

use crate::ccf::{container, write};
use crate::codec::put_varint;
use crate::tags::*;
use crate::*;

/// xorshift64*: deterministic, no dependency.
pub(crate) struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `0..n` (0 when `n` is 0).
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            0
        } else {
            (self.next() % u64::from(n)) as u32
        }
    }
    fn i32(&mut self) -> i32 {
        self.next() as i32
    }
    pub fn chance(&mut self, percent: u32) -> bool {
        self.below(100) < percent
    }
}

/// A random chart that passes [`Chart::check`], with about `rows` rows of up to
/// `max_row` seats. Zero rows gives empty seat arrays and tables.
fn gen_chart(seed: u64, rows: u32, max_row: u16) -> Chart {
    let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut strings = Strings::default();
    let words = [
        "",
        "A",
        "Orchestra",
        "Ünïcødé",
        "東京",
        "Box 1",
        "{section}-{row}-{seat}",
    ];
    for i in 0..8 + r.below(20) {
        strings.push(
            words
                .get((i as usize) % words.len())
                .copied()
                .unwrap_or("x"),
        );
    }
    let ns = strings.len() as u32;
    let s = |r: &mut Rng| r.below(ns);
    let opt_s = |r: &mut Rng| if r.chance(30) { NONE } else { r.below(ns) };
    let pt = |r: &mut Rng| [r.i32(), r.i32()];
    let bx = |r: &mut Rng| [r.i32(), r.i32(), r.i32(), r.i32()];
    let ncat = 1 + r.below(6);
    let categories = (0..ncat)
        .map(|_| Category {
            key: s(&mut r),
            label: s(&mut r),
            color: [1, 2, 3, 255],
            flags: r.below(2) as u8,
        })
        .collect();

    let nrules = r.below(4);
    let schemes = [
        Scheme::Numeric,
        Scheme::Odd,
        Scheme::Alpha,
        Scheme::Roman,
        Scheme::Custom,
    ];
    let label_rules: Vec<SeatLabelRule> = (0..nrules)
        .map(|_| SeatLabelRule {
            scheme: schemes[r.below(5) as usize],
            overflow: if r.chance(50) {
                AlphaOverflow::Double
            } else {
                AlphaOverflow::Spreadsheet
            },
            start: r.below(10),
            step: 1 + r.below(3),
            prefix: opt_s(&mut r),
            suffix: opt_s(&mut r),
            custom_first: 1,
            custom_count: r.below(ns - 1),
        })
        .collect();

    // Rows, with segments split at random removed positions.
    let directions = [SeatOrder::Ltr, SeatOrder::Rtl, SeatOrder::CenterOut];
    let (mut row_list, mut segments, mut seat_pos) = (Vec::new(), Vec::new(), Vec::new());
    for index in 0..rows {
        let count = r.below(u32::from(max_row) + 1) as u16;
        let first = seat_pos.len() as u32;
        let mut position = 0u16;
        let mut left = count;
        while left > 0 {
            position += r.below(3) as u16; // a gap
            let n = if r.chance(70) {
                left
            } else {
                1 + r.below(u32::from(left)) as u16
            };
            segments.push(Segment {
                row: index,
                seat_first: first + u32::from(count - left),
                seat_count: n,
                position,
            });
            position += n;
            left -= n;
        }
        // A straight row with a constant step, or random jitter.
        let (mut p, step) = (
            pt(&mut r).map(|v| v / 4),
            [r.below(300) as i32, r.below(9) as i32 - 4],
        );
        for _ in 0..count {
            seat_pos.push(p);
            p = if r.chance(90) {
                [p[0] + step[0], p[1] + step[1]]
            } else {
                pt(&mut r)
            };
        }
        let positions = position + r.below(3) as u16;
        let ordinal_base = if r.chance(50) { 0 } else { r.below(1000) };
        // A custom list must cover every position.
        let pick = r.below(nrules + 1);
        let label_rule = match label_rules.get(pick as usize) {
            Some(l)
                if l.scheme != Scheme::Custom
                    || ordinal_base + u32::from(positions) <= l.custom_count =>
            {
                pick
            }
            _ => NONE,
        };
        row_list.push(Row {
            section: NONE,
            label: s(&mut r),
            seat_first: first,
            seat_count: count,
            positions,
            label_rule,
            ordinal_base,
            direction: directions[r.below(3) as usize],
            shape: SeatShape::Rounded,
            seat_size: 176,
            bounds: bx(&mut r),
            label_start: pt(&mut r),
            label_end: pt(&mut r),
        });
    }
    let seats = seat_pos.len() as u32;

    // Floors, sections and chunks each cut the seats at random row boundaries.
    let cuts = |r: &mut Rng, max: u32, parts: u32| -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        let mut first = 0;
        for i in 0..parts {
            let count = if i + 1 == parts {
                seats - first
            } else {
                r.below((seats - first).min(max) + 1)
            };
            out.push((first, count));
            first += count;
        }
        out
    };
    let nfloors = if seats == 0 {
        r.below(2)
    } else {
        1 + r.below(3)
    };
    let floors: Vec<Floor> = cuts(&mut r, seats, nfloors)
        .into_iter()
        .map(|(seat_first, seat_count)| Floor {
            name: s(&mut r),
            level: r.below(5) as i32 - 2,
            seat_first,
            seat_count,
            bounds: bx(&mut r),
            focal: r.chance(50).then(|| pt(&mut r)),
        })
        .collect();
    let mut chunks = Vec::new();
    let mut first = 0;
    while first < seats {
        let count = 1 + r.below((seats - first).min(CHUNK_MAX_SEATS));
        chunks.push(RenderChunk {
            section: NONE,
            seat_first: first,
            seat_count: count,
            bounds: bx(&mut r),
        });
        first += count;
    }

    let nverts = r.below(30);
    let indices: Vec<u32> = (0..if nverts == 0 { 0 } else { 3 * r.below(20) })
        .map(|_| r.below(nverts))
        .collect();
    let ni = indices.len() as u32;
    let range = |r: &mut Rng| {
        let first = r.below(ni + 1);
        (first, r.below(ni - first + 1))
    };
    let nsect = if floors.is_empty() { 0 } else { r.below(5) };
    let sections: Vec<Section> = (0..nsect)
        .map(|_| {
            let (mesh_first, mesh_count) = range(&mut r);
            let seat_first = r.below(seats + 1);
            let row_first = r.below(rows + 1);
            Section {
                label: opt_s(&mut r),
                floor: r.below(floors.len() as u32),
                seat_first,
                seat_count: r.below(seats - seat_first + 1),
                row_first,
                row_count: r.below(rows - row_first + 1),
                category: if r.chance(50) { NONE } else { r.below(ncat) },
                entrance: opt_s(&mut r),
                mesh_first,
                mesh_count,
                bounds: bx(&mut r),
                label_anchor: pt(&mut r),
                label_size: r.below(4000) as u16,
            }
        })
        .collect();
    for row in &mut row_list {
        row.section = if nsect == 0 || r.chance(20) {
            NONE
        } else {
            r.below(nsect)
        };
    }
    for c in &mut chunks {
        c.section = if nsect == 0 { NONE } else { r.below(nsect) };
    }

    // Categories and flags in long runs.
    let (mut seat_category, mut seat_flags) = (Vec::new(), Vec::new());
    let (mut cat, mut flag) = (0u16, 0u8);
    for _ in 0..seats {
        if r.chance(3) {
            cat = r.below(ncat) as u16;
        }
        if r.chance(2) {
            flag = r.below(32) as u8;
        }
        seat_category.push(cat);
        seat_flags.push(flag);
    }
    let mut label_exceptions = Vec::new();
    let mut idx = r.below(40);
    while idx < seats {
        label_exceptions.push(LabelException {
            idx,
            label: s(&mut r),
        });
        idx += 1 + r.below(40);
    }

    let nobj = if floors.is_empty() { 0 } else { r.below(6) };
    let kinds = [ObjectKind::Table, ObjectKind::Booth, ObjectKind::Area];
    let objects: Vec<Object> = (0..nobj)
        .map(|_| {
            let (mesh_first, mesh_count) = range(&mut r);
            Object {
                kind: kinds[r.below(3) as usize],
                label: s(&mut r),
                category: r.below(ncat),
                capacity: r.below(5000),
                floor: r.below(floors.len() as u32),
                section: if nsect == 0 { NONE } else { r.below(nsect) },
                mesh_first,
                mesh_count,
                bounds: bx(&mut r),
                label_anchor: pt(&mut r),
            }
        })
        .collect();
    let tables = (0..r.below(4))
        .map(|_| {
            let whole = nobj > 0 && r.chance(50);
            let chairs = r.below(seats.min(12) + 1) as u16;
            Table {
                shape: if r.chance(50) {
                    TableShape::Round
                } else {
                    TableShape::Rect
                },
                rotation: r.below(65536) as u16,
                chairs,
                seat_first: if whole {
                    NONE
                } else {
                    r.below(seats - u32::from(chairs) + 1)
                },
                object: if whole { r.below(nobj) } else { NONE },
                centre: pt(&mut r),
                size: pt(&mut r),
            }
        })
        .collect();
    // Sorted by (floor, layer), end to end.
    let mut draws = Vec::new();
    let mut next = r.below(ni + 1);
    for floor in 0..floors.len() as u32 {
        for layer in [Layer::Below, Layer::Above] {
            if r.chance(40) {
                let index_count = r.below(ni - next + 1);
                draws.push(Draw {
                    floor,
                    layer,
                    index_first: next,
                    index_count,
                });
                next += index_count;
            }
        }
    }
    let vertices = (0..nverts)
        .map(|_| Vertex {
            pos: [r.i32() as f32 / 7.0, -0.0],
            color: [r.below(256) as u8, 0, 0, 255],
        })
        .collect();
    let nfl = floors.len() as u32;
    let texts = (0..if nfl == 0 { 0 } else { r.below(4) })
        .map(|_| Text {
            text: s(&mut r),
            floor: r.below(nfl),
            pos: pt(&mut r),
            size: 480,
            angle: r.below(65536) as u16,
            color: [9, 9, 9, 255],
            align: TextAlign::Center,
            weight: 600,
            min_zoom: r.below(100) as f32 / 8.0,
        })
        .collect();
    let icons = (0..if nfl == 0 { 0 } else { r.below(4) })
        .map(|_| Icon {
            key: s(&mut r),
            floor: r.below(nfl),
            pos: pt(&mut r),
            size: 320,
            angle: 0,
        })
        .collect();
    let images = (0..if nfl == 0 { 0 } else { r.below(3) })
        .map(|_| Image {
            asset: s(&mut r),
            floor: r.below(nfl),
            bounds: bx(&mut r),
            angle: 0,
            opacity: 128,
        })
        .collect();

    let objects_total = seats + nobj;
    let full = Full {
        oids: (0..objects_total)
            .map(|_| {
                if r.chance(80) {
                    100 + r.below(1_000_000)
                } else {
                    r.next() as u32
                }
            })
            .collect(),
        ranks: (0..objects_total).map(|_| r.below(65536) as u16).collect(),
        priorities: (0..nsect).map(|_| r.below(40) as f32 / 8.0).collect(),
    };
    Chart {
        meta: Meta {
            name: s(&mut r),
            label_template: s(&mut r),
            table_label_template: s(&mut r),
            compiler: s(&mut r),
            seat_count: seats,
            bounds: bx(&mut r),
            seat_size: 176,
            seat_shape: SeatShape::Circle,
            row_label_display: RowLabelDisplay::Both,
            rank_algo: 1,
        },
        strings,
        categories,
        floors,
        sections,
        chunks,
        rows: row_list,
        segments,
        label_rules,
        label_exceptions,
        seat_pos,
        seat_category,
        seat_flags,
        objects,
        tables,
        mesh: Mesh {
            vertices,
            indices,
            draws,
        },
        texts,
        icons,
        images,
        full: Some(full),
    }
}

fn public(chart: &Chart) -> Chart {
    Chart {
        full: None,
        ..chart.clone()
    }
}

fn charts() -> impl Iterator<Item = Chart> {
    let sizes = [(0, 0), (1, 1), (3, 5), (10, 40), (40, 300), (3, 4095)];
    (0..60u64).map(move |seed| {
        let (rows, max_row) = sizes[seed as usize % sizes.len()];
        gen_chart(seed, rows, max_row)
    })
}

#[test]
fn round_trip_both_artefacts() {
    for chart in charts() {
        chart.check().unwrap();
        let full = encode(&chart, Artefact::Full).unwrap();
        let public_bytes = encode(&chart, Artefact::Public).unwrap();
        assert_eq!(
            full,
            encode(&chart.clone(), Artefact::Full).unwrap(),
            "deterministic"
        );
        assert_eq!(decode(&full, Artefact::Full).unwrap(), chart);
        // A public consumer reads either artefact and gets the same chart.
        assert_eq!(decode(&full, Artefact::Public).unwrap(), public(&chart));
        assert_eq!(
            decode(&public_bytes, Artefact::Public).unwrap(),
            public(&chart)
        );
        assert_eq!(
            encode(&public(&chart), Artefact::Public).unwrap(),
            public_bytes
        );
        let err = decode(&public_bytes, Artefact::Full).unwrap_err();
        assert_eq!(
            err,
            Error {
                chunk: None,
                kind: ErrorKind::NotFull
            }
        );
    }
}

#[test]
fn content_hash_is_patched_not_decoded() {
    let chart = gen_chart(7, 5, 20);
    let mut file = encode(&chart, Artefact::Full).unwrap();
    assert_eq!(content_hash(&file), Some([0; 16]));
    set_content_hash(&mut file, [0xAB; 16]).unwrap();
    assert_eq!(content_hash(&file), Some([0xAB; 16]));
    assert_eq!(decode(&file, Artefact::Full).unwrap(), chart);
}

/// The chunks of a well-formed file, in directory order.
fn chunks_of(file: &[u8]) -> Vec<(Tag, u8, Vec<u8>)> {
    let n = u32::from_le_bytes(file[8..12].try_into().unwrap()) as usize;
    (0..n)
        .map(|i| {
            let e = &file[32 + 16 * i..48 + 16 * i];
            let off = u32::from_le_bytes(e[8..12].try_into().unwrap()) as usize;
            let len = u32::from_le_bytes(e[12..16].try_into().unwrap()) as usize;
            (
                e[..4].try_into().unwrap(),
                e[4],
                file[off..off + len].to_vec(),
            )
        })
        .collect()
}

fn assemble(flags: u16, chunks: &[(Tag, u8, Vec<u8>)]) -> Vec<u8> {
    let tags: Vec<(Tag, u8)> = chunks.iter().map(|c| (c.0, c.1)).collect();
    let mut i = 0;
    container(flags, &tags, |_, out| {
        out.extend_from_slice(&chunks[i].2);
        i += 1;
        Ok(())
    })
    .unwrap()
}

/// `file` with chunk `tag`'s payload replaced.
fn with_payload(file: &[u8], tag: Tag, payload: Vec<u8>) -> Vec<u8> {
    let flags = u16::from_le_bytes([file[6], file[7]]);
    let mut chunks = chunks_of(file);
    chunks.iter_mut().find(|c| c.0 == tag).unwrap().2 = payload;
    assemble(flags, &chunks)
}

fn varints(values: &[u32]) -> Vec<u8> {
    let mut out = Vec::new();
    for &v in values {
        put_varint(&mut out, v);
    }
    out
}

fn put32(file: &mut [u8], at: usize, v: u32) {
    file[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

#[test]
fn unknown_chunks_are_skipped() {
    let chart = gen_chart(3, 10, 40);
    let file = encode(&chart, Artefact::Full).unwrap();
    let mut chunks = chunks_of(&file);
    chunks.insert(0, (*b"ZZZZ", 4, vec![0xFF; 7]));
    chunks.insert(5, (*b"zzzz", 0, Vec::new()));
    chunks.push((*b"XTRA", 2, vec![1, 2, 3]));
    let file = assemble(FLAG_FULL, &chunks);
    assert_eq!(decode(&file, Artefact::Full).unwrap(), chart);
}

#[test]
fn hostile_inputs_give_typed_errors() {
    use ErrorKind::*;
    let chart = gen_chart(11, 6, 30);
    let good = encode(&chart, Artefact::Full).unwrap();
    let dir = |i: usize| 32 + 16 * i; // directory entry i
    let n = chunks_of(&good).len();
    let edit = |f: &dyn Fn(&mut Vec<u8>)| {
        let mut file = good.clone();
        f(&mut file);
        file
    };
    let bad_chart = |f: &dyn Fn(&mut Chart)| {
        let mut c = chart.clone();
        f(&mut c);
        write(&c, Artefact::Full).unwrap()
    };
    // A floor index one past the last floor, and records to place there.
    let nf = chart.floors.len() as u32;
    let text = Text {
        text: 0,
        floor: 0,
        pos: [0, 0],
        size: 4,
        angle: 0,
        color: [0; 4],
        align: TextAlign::Left,
        weight: 400,
        min_zoom: 0.0,
    };
    let icon = Icon {
        key: 0,
        floor: 0,
        pos: [0, 0],
        size: 4,
        angle: 0,
    };
    let image = Image {
        asset: 0,
        floor: 0,
        bounds: [0; 4],
        angle: 0,
        opacity: 255,
    };
    let draw = |floor, layer, index_first| Draw {
        floor,
        layer,
        index_first,
        index_count: 0,
    };
    let mut rows = chunks_of(&good);
    rows.retain(|c| c.0 != SCAT);
    let mut dup = chunks_of(&good);
    dup.push(dup[2].clone());

    let cases: Vec<(&str, Vec<u8>, Option<Tag>, ErrorKind)> = vec![
        ("truncated header", good[..20].to_vec(), None, Truncated),
        ("bad magic", edit(&|f| f[0] = b'X'), None, BadMagic),
        (
            "newer version",
            edit(&|f| f[4] = 2),
            None,
            UnsupportedVersion,
        ),
        ("version 0", edit(&|f| f[4] = 0), None, UnsupportedVersion),
        (
            "truncated file",
            good[..good.len() - 1].to_vec(),
            None,
            LengthMismatch,
        ),
        (
            "directory beyond file",
            edit(&|f| put32(f, 8, 1_000_000)),
            None,
            OutOfBounds,
        ),
        (
            "offset beyond file",
            edit(&|f| put32(f, dir(3) + 8, 0x7FFF_FFF0)),
            Some(FLRS),
            OutOfBounds,
        ),
        (
            "length beyond file",
            edit(&|f| put32(f, dir(3) + 12, 0xFFFF_FFFF)),
            Some(FLRS),
            OutOfBounds,
        ),
        (
            "offset in directory",
            edit(&|f| put32(f, dir(0) + 8, 32)),
            Some(META),
            OutOfBounds,
        ),
        (
            "overlap",
            edit(&|f| {
                let prev = u32::from_le_bytes(f[dir(2) + 8..dir(2) + 12].try_into().unwrap());
                put32(f, dir(3) + 8, prev);
            }),
            Some(FLRS),
            Overlap,
        ),
        (
            "unaligned",
            edit(&|f| {
                let o = u32::from_le_bytes(f[dir(n - 1) + 8..dir(n - 1) + 12].try_into().unwrap());
                put32(f, dir(n - 1) + 8, o + 1);
                put32(f, dir(n - 1) + 12, 0);
            }),
            Some(OPRI),
            Unaligned,
        ),
        (
            "unknown encoding",
            edit(&|f| f[dir(1) + 4] = 9),
            Some(STRS),
            UnknownEncoding,
        ),
        (
            "wrong encoding",
            edit(&|f| f[dir(1) + 4] = 0),
            Some(STRS),
            WrongEncoding,
        ),
        (
            "duplicate chunk",
            assemble(FLAG_FULL, &dup),
            Some(CATS),
            Duplicate,
        ),
        (
            "missing chunk",
            assemble(FLAG_FULL, &rows),
            Some(SCAT),
            Missing,
        ),
        (
            "missing full chunk",
            assemble(
                FLAG_FULL,
                &chunks_of(&encode(&chart, Artefact::Public).unwrap()),
            ),
            Some(SOID),
            Missing,
        ),
        (
            "categories over limit",
            with_payload(&good, CATS, varints(&[4096])),
            Some(CATS),
            OverLimit,
        ),
        (
            "seats over limit",
            bad_chart(&|c| c.meta.seat_count = 1_000_001),
            Some(META),
            OverLimit,
        ),
        (
            "count beyond payload",
            with_payload(&good, STRS, varints(&[1_000_000, 0])),
            Some(STRS),
            BadCount,
        ),
        (
            "trailing bytes",
            with_payload(&good, ICON, varints(&[0, 0])),
            Some(ICON),
            BadCount,
        ),
        (
            "varint never ends",
            with_payload(&good, LBLX, vec![0x80; 3]),
            Some(LBLX),
            Truncated,
        ),
        (
            "varint overflows",
            with_payload(&good, LBLX, vec![0xFF, 0xFF, 0xFF, 0xFF, 0x1F]),
            Some(LBLX),
            BadVarint,
        ),
        (
            "string offsets out of order",
            with_payload(
                &good,
                STRS,
                [
                    varints(&[2]),
                    3u32.to_le_bytes().to_vec(),
                    1u32.to_le_bytes().to_vec(),
                    b"abc".to_vec(),
                ]
                .concat(),
            ),
            Some(STRS),
            BadString,
        ),
        (
            "string not UTF-8",
            with_payload(
                &good,
                STRS,
                [varints(&[1]), 2u32.to_le_bytes().to_vec(), vec![0xC3, 0x28]].concat(),
            ),
            Some(STRS),
            BadString,
        ),
        (
            "string splits a character",
            with_payload(
                &good,
                STRS,
                [
                    varints(&[2]),
                    1u32.to_le_bytes().to_vec(),
                    2u32.to_le_bytes().to_vec(),
                    "é".as_bytes().to_vec(),
                ]
                .concat(),
            ),
            Some(STRS),
            BadString,
        ),
        (
            "zero run",
            with_payload(&good, SFLG, varints(&[chart.seat_pos.len() as u32, 0, 0])),
            Some(SFLG),
            BadCount,
        ),
        (
            "bad enum",
            with_payload(&good, META, {
                let mut meta = chunks_of(&good)[0].2.clone();
                meta[1 + 38] = 9; // seat_shape, after the count and 38 bytes of fields
                meta
            }),
            Some(META),
            BadValue,
        ),
        (
            "row beyond seats",
            bad_chart(&|c| c.rows[0].seat_first = 900),
            Some(ROWS),
            BadRange,
        ),
        (
            "string index beyond table",
            bad_chart(&|c| c.categories[0].label = 9_999),
            Some(CATS),
            BadReference,
        ),
        (
            "category beyond table",
            bad_chart(&|c| c.seat_category[0] = 4_000),
            Some(SCAT),
            BadReference,
        ),
        (
            "mesh index beyond vertices",
            bad_chart(&|c| c.mesh.indices.push(u32::MAX - 1)),
            Some(MESH),
            BadReference,
        ),
        (
            "segment off its row",
            bad_chart(&|c| c.segments[0].row = 5),
            Some(RSEG),
            BadRange,
        ),
        (
            "table with no booking target",
            bad_chart(&|c| {
                c.tables.push(Table {
                    seat_first: NONE,
                    object: NONE,
                    ..chart.tables[0]
                })
            }),
            Some(TBLS),
            BadValue,
        ),
        (
            "row label over 64 bytes",
            bad_chart(&|c| c.rows[0].label = c.strings.push(&"x".repeat(65)).unwrap()),
            Some(ROWS),
            OverLimit,
        ),
        (
            "ordinals overflow",
            bad_chart(&|c| c.rows[0].ordinal_base = u32::MAX - 1),
            Some(ROWS),
            BadRange,
        ),
        (
            "custom labels shorter than the row",
            bad_chart(&|c| {
                // Covers the row's positions but not with a base of 1.
                let rule = SeatLabelRule {
                    scheme: Scheme::Custom,
                    custom_count: u32::from(c.rows[0].positions),
                    ..c.label_rules[0]
                };
                c.label_rules.push(rule);
                c.rows[0].label_rule = c.label_rules.len() as u32 - 1;
                c.rows[0].ordinal_base = 1;
            }),
            Some(ROWS),
            BadRange,
        ),
        (
            "non-finite float",
            bad_chart(&|c| {
                c.texts.push(Text {
                    min_zoom: f32::INFINITY,
                    ..text
                })
            }),
            Some(TEXT),
            BadValue,
        ),
        (
            "text on an unknown floor",
            bad_chart(&|c| c.texts.push(Text { floor: nf, ..text })),
            Some(TEXT),
            BadReference,
        ),
        (
            "icon on an unknown floor",
            bad_chart(&|c| c.icons.push(Icon { floor: nf, ..icon })),
            Some(ICON),
            BadReference,
        ),
        (
            "image on an unknown floor",
            bad_chart(&|c| c.images.push(Image { floor: nf, ..image })),
            Some(IMGS),
            BadReference,
        ),
        (
            "draw range on an unknown floor",
            bad_chart(&|c| c.mesh.draws = vec![draw(nf, Layer::Below, 0)]),
            Some(MESH),
            BadReference,
        ),
        (
            "draw ranges out of order",
            bad_chart(&|c| c.mesh.draws = vec![draw(0, Layer::Above, 0), draw(0, Layer::Below, 0)]),
            Some(MESH),
            BadRange,
        ),
        (
            "draw ranges not end to end",
            bad_chart(&|c| {
                let n = c.mesh.indices.len() as u32;
                c.mesh.vertices.push(Vertex {
                    pos: [0.0; 2],
                    color: [0; 4],
                });
                c.mesh.indices.extend([0; 6]);
                c.mesh.draws = vec![
                    Draw {
                        index_count: 3,
                        ..draw(0, Layer::Below, n)
                    },
                    Draw {
                        index_count: 2,
                        ..draw(0, Layer::Above, n + 4)
                    },
                ];
            }),
            Some(MESH),
            BadRange,
        ),
    ];
    for (name, file, chunk, kind) in cases {
        let err = decode(&file, Artefact::Full).unwrap_err();
        assert_eq!(err, Error { chunk, kind }, "{name}");
    }
}

/// Seeded byte flips, truncations and splices: decode returns `Ok` or `Err` and
/// never panics, and whatever it accepts re-encodes to an equal chart.
#[test]
fn mutated_files_never_panic() {
    let mut r = Rng(0xC0FFEE);
    let mut accepted = 0;
    for (i, chart) in charts().take(12).enumerate() {
        let artefact = if i % 2 == 0 {
            Artefact::Full
        } else {
            Artefact::Public
        };
        let good = encode(&chart, artefact).unwrap();
        // Payload spans of the chunks that carry floors and draw ranges.
        let spans: Vec<(usize, usize)> = (0..good.len().min(32 + 16 * 32))
            .step_by(16)
            .skip(2)
            .filter_map(|at| {
                let entry = good.get(at..at + 16)?;
                let tag: Tag = entry[..4].try_into().ok()?;
                let word = |i: usize| u32::from_le_bytes(entry[i..i + 4].try_into().unwrap());
                [MESH, TEXT, ICON, IMGS]
                    .contains(&tag)
                    .then(|| (word(8) as usize, word(12) as usize))
            })
            .filter(|&(_, len)| len > 1)
            .collect();
        for _ in 0..400 {
            let mut f = good.clone();
            match r.below(5) {
                4 if !spans.is_empty() => {
                    // Rewrite a byte inside a decoration chunk.
                    let (at, len) = spans[r.below(spans.len() as u32) as usize];
                    f[at + r.below(len as u32) as usize] = r.next() as u8;
                }
                0 => {
                    for _ in 0..1 + r.below(4) {
                        let at = r.below(f.len() as u32) as usize;
                        f[at] ^= 1 << r.below(8);
                    }
                }
                1 => f.truncate(r.below(f.len() as u32) as usize),
                2 => {
                    let (a, b) = (
                        r.below(f.len() as u32) as usize,
                        r.below(f.len() as u32) as usize,
                    );
                    let len = r.below(64) as usize;
                    let piece: Vec<u8> = f[a..(a + len).min(f.len())].to_vec();
                    f.splice(b..b, piece);
                }
                _ => {
                    // Corrupt a payload byte only, past the header checks.
                    let at = 32 + r.below(f.len() as u32 - 32) as usize;
                    f[at] = r.next() as u8;
                }
            }
            if f.len() >= 16 && r.chance(70) {
                let len = f.len() as u32;
                put32(&mut f, 12, len); // keep total_len plausible to reach deeper checks
            }
            for want in [Artefact::Public, Artefact::Full] {
                if let Ok(decoded) = decode(&f, want) {
                    accepted += 1;
                    let again = encode(&decoded, want).unwrap();
                    assert_eq!(decode(&again, want).unwrap(), decoded);
                }
            }
        }
    }
    assert!(accepted > 0, "some mutations should survive");
}

/// Release-mode decode timing for a chart of about 100,000 seats:
/// `cargo test -p cavea-format --release -- --ignored --nocapture decode_speed`.
#[test]
#[ignore = "timing, run by hand in release mode"]
fn decode_speed() {
    let chart = gen_chart(42, 2_000, 100);
    let file = encode(&chart, Artefact::Public).unwrap();
    let runs = 50;
    let start = std::time::Instant::now();
    for _ in 0..runs {
        std::hint::black_box(decode(std::hint::black_box(&file), Artefact::Public).unwrap());
    }
    let per = start.elapsed() / runs;
    let seats = chart.seat_pos.len();
    println!(
        "decode: {seats} seats, {} bytes, {per:?} per decode",
        file.len()
    );
}
