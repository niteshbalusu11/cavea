use cavea_core::limits::MAX_OBJECTS;

use crate::tests::Rng;
use crate::wire::*;

fn v(x: u64) -> Vec<u8> {
    let mut out = Vec::new();
    let mut x = x;
    while x >= 0x80 {
        out.push(x as u8 | 0x80);
        x >>= 7;
    }
    out.push(x as u8);
    out
}

fn reduce(view: View, statuses: &[u8]) -> Vec<u8> {
    statuses
        .iter()
        .map(|&s| match view {
            View::Public => u8::from(s != 0),
            View::Manager => s,
        })
        .collect()
}

/// A state payload in the given encoding, written from the spec (08 §8.1)
/// independently of the encoder, so that every encoding is decoded whatever
/// the encoder would choose.
fn payload(encoding: u8, codes: &[u8]) -> Vec<u8> {
    match encoding {
        0 => codes.to_vec(),
        1 => {
            let mut out = Vec::new();
            let mut i = 0;
            while i < codes.len() {
                let run = codes[i..].iter().take_while(|&&c| c == codes[i]).count();
                out.push(codes[i]);
                out.extend(v(run as u64));
                i += run;
            }
            out
        }
        _ => {
            let mut out = vec![0u8; codes.len().div_ceil(8)];
            for (i, &c) in codes.iter().enumerate() {
                out[i / 8] |= c << (i % 8);
            }
            out
        }
    }
}

fn snapshot_frame(seq: u64, encoding: u8, codes: &[u8], areas: &[(u32, u32)]) -> Vec<u8> {
    let p = payload(encoding, codes);
    let mut f = [
        vec![0x02],
        v(seq),
        v(codes.len() as u64),
        vec![encoding],
        v(p.len() as u64),
        p,
        v(areas.len() as u64),
    ]
    .concat();
    for &(idx, value) in areas {
        f.extend(v(idx.into()));
        f.extend(v(value.into()));
    }
    f
}

/// Random domain statuses: uniform, mostly free, or in runs.
fn statuses(r: &mut Rng, n: usize, max: u8) -> Vec<u8> {
    let mode = r.below(3);
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let s = r.below(u32::from(max) + 1) as u8;
        let run = match mode {
            0 => 1,
            1 => 1 + r.below(200) as usize,
            _ => usize::from(r.chance(5)) * s as usize + 1,
        };
        let s = if mode == 2 && !r.chance(10) { 0 } else { s };
        out.extend(std::iter::repeat_n(s, run.min(n - out.len())));
    }
    out
}

fn sizes() -> impl Iterator<Item = usize> {
    [0, 1, 7, 8, 9, 15, 16, 17, 255, 1_000, 4_097].into_iter()
}

fn seqs() -> [u64; 5] {
    [0, 1, 127, u64::MAX - 1, u64::MAX]
}

fn server(frame: &[u8], view: View) -> ServerFrame<'_> {
    decode_server(frame, view).unwrap_or_else(|e| panic!("{e:?}: {frame:?}"))
}

fn state_of(frame: &[u8], view: View) -> Vec<u8> {
    let ServerFrame::Snapshot(s) = server(frame, view) else {
        panic!("not a snapshot");
    };
    let mut out = vec![0xEE; s.objects as usize];
    s.write_into(&mut out).unwrap();
    // The runs say the same, cover every object once, and alternate in code
    // when the encoder chose them.
    let mut from_runs = Vec::new();
    for (range, code) in s.runs(s.objects).unwrap() {
        assert_eq!(range.start as usize, from_runs.len());
        assert!(!range.is_empty());
        from_runs.extend(std::iter::repeat_n(code, range.len()));
    }
    assert_eq!(from_runs, out);
    out
}

#[test]
fn snapshots_round_trip_in_the_smallest_encoding() {
    let mut r = Rng(7);
    let random: Vec<usize> = (0..40).map(|_| r.below(3_000) as usize).collect();
    for n in sizes().chain(random) {
        for view in [View::Public, View::Manager] {
            let max = if r.chance(50) { 1 } else { 15 };
            let domain = statuses(&mut r, n, max);
            let codes = reduce(view, &domain);
            let seq = seqs()[r.below(5) as usize];
            let frame = encode_snapshot(view, seq, &domain, &[]).unwrap();
            let ServerFrame::Snapshot(s) = server(&frame, view) else {
                panic!("not a snapshot");
            };
            assert_eq!((s.seq, s.objects as usize), (seq, n));
            assert_eq!(state_of(&frame, view), codes);

            // Every encoding the view allows decodes to the same state; the
            // encoder's choice is no larger than any of them, and ties go to
            // the lowest number.
            let allowed: &[u8] = match (view, codes.iter().all(|&c| c <= 1)) {
                (View::Public, true) => &[0, 1, 2],
                _ => &[0, 1],
            };
            let mut best = (usize::MAX, 0);
            for &e in allowed {
                let alt = snapshot_frame(seq, e, &codes, &[]);
                assert_eq!(state_of(&alt, view), codes, "encoding {e}");
                best = best.min((alt.len(), e));
            }
            assert_eq!((frame.len(), s.encoding as u8), best, "n {n} {view:?}");
        }
    }
}

#[test]
fn public_frames_carry_only_public_codes() {
    let all: Vec<u8> = (0..=MAX_STATUS).collect();
    let public: Vec<u8> = all.iter().map(|&s| u8::from(s != 0)).collect();
    let frame = encode_snapshot(View::Public, 1, &all, &[]).unwrap();
    assert_eq!(state_of(&frame, View::Public), public);
    let changes: Vec<Change> = all
        .iter()
        .map(|&s| Change {
            idx: u32::from(s),
            code: s,
        })
        .collect();
    let frame = encode_delta(View::Public, 1, 2, &changes, 16, &[]).unwrap();
    let ServerFrame::Delta(d) = server(&frame, View::Public) else {
        panic!("not a delta");
    };
    let codes: Vec<u8> = d.changes(16).unwrap().map(|c| c.code).collect();
    assert_eq!(codes, public);
    // A status beyond the domain range is refused, not reduced.
    let err = encode_snapshot(View::Public, 1, &[0, 16], &[]).unwrap_err();
    assert_eq!(err, WireError::BadValue);
    let bad = [Change { idx: 0, code: 16 }];
    assert_eq!(
        encode_delta(View::Public, 1, 2, &bad, 1, &[]),
        Err(WireError::BadValue)
    );
}

fn random_changes(r: &mut Rng, objects: u32, n: usize) -> Vec<Change> {
    (0..n)
        .map(|_| Change {
            idx: r.below(objects),
            code: r.below(16) as u8,
        })
        .collect()
}

#[test]
fn coalescing_keeps_the_last_code_in_ascending_idx() {
    let mut r = Rng(11);
    for _ in 0..200 {
        let objects = 1 + r.below(300);
        let commits = {
            let k = r.below(600) as usize;
            random_changes(&mut r, objects, k)
        };
        let mut last = vec![None; objects as usize];
        for c in &commits {
            last[c.idx as usize] = Some(c.code);
        }
        let mut coalesced = commits.clone();
        coalesce(&mut coalesced);
        let expected: Vec<Change> = last
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                c.map(|code| Change {
                    idx: i as u32,
                    code,
                })
            })
            .collect();
        assert_eq!(coalesced, expected);
    }
}

/// A server applies commands to its status array and publishes a snapshot and
/// then one coalesced delta per interval; a client that applies the snapshot
/// and each delta whose `from_seq` is its `seq` ends with the same codes.
#[test]
fn snapshot_then_deltas_match_direct_application() {
    let mut r = Rng(13);
    for round in 0..60 {
        let view = if round % 2 == 0 {
            View::Public
        } else {
            View::Manager
        };
        let objects = 1 + r.below(2_000);
        let mut truth = statuses(&mut r, objects as usize, 15);
        let mut seq = u64::from(r.below(1_000));
        if round % 7 == 0 {
            seq = u64::MAX - 200;
        }
        let snap = encode_snapshot(view, seq, &truth, &[]).unwrap();
        let ServerFrame::Snapshot(s) = server(&snap, view) else {
            panic!("not a snapshot");
        };
        let (mut have, mut have_seq) = (vec![0u8; objects as usize], s.seq);
        s.write_into(&mut have).unwrap();
        for _ in 0..10 {
            let commands = r.below(15) as u64;
            if commands == 0 {
                continue;
            }
            // Each command commits one `seq` and changes a few objects.
            let mut interval = Vec::new();
            for _ in 0..commands {
                let changes = {
                    let k = r.below(8) as usize;
                    random_changes(&mut r, objects, k)
                };
                for c in &changes {
                    truth[c.idx as usize] = c.code;
                }
                interval.extend(changes);
            }
            coalesce(&mut interval);
            let to = seq + commands;
            let frame = encode_delta(view, seq, to, &interval, objects, &[]).unwrap();
            seq = to;
            let ServerFrame::Delta(d) = server(&frame, view) else {
                panic!("not a delta");
            };
            assert_eq!(d.from_seq, have_seq, "no gap in this run");
            let mut prev = None;
            for c in d.changes(objects).unwrap() {
                assert!(prev < Some(c.idx));
                prev = Some(c.idx);
                have[c.idx as usize] = c.code;
            }
            have_seq = d.to_seq;
        }
        assert_eq!(have, reduce(view, &truth));
        assert_eq!(have_seq, seq);
    }
}

/// One valid frame of every type, with random contents and boundary values.
fn frames(r: &mut Rng) -> Vec<(Vec<u8>, View)> {
    let mut out = Vec::new();
    for _ in 0..20 {
        let view = if r.chance(50) {
            View::Public
        } else {
            View::Manager
        };
        let objects = [0, 1, 300, MAX_OBJECTS][r.below(4) as usize];
        let seq = seqs()[r.below(5) as usize];
        let hello = Hello {
            server_time_ms: seqs()[r.below(5) as usize],
            chart_hash: [r.next() as u8; 16],
            objects,
        };
        out.push((encode_hello(&hello).unwrap(), view));
        let n = r.below(objects.min(2_000) + 1);
        let domain = statuses(r, n as usize, 15);
        let areas: Vec<Area> = if n > 1 && r.chance(30) {
            vec![
                Area { idx: 0, value: 0 },
                Area {
                    idx: n - 1,
                    value: u32::MAX,
                },
            ]
        } else {
            Vec::new()
        };
        let areas = &areas[..];
        out.push((encode_snapshot(view, seq, &domain, areas).unwrap(), view));
        for e in [0, 1, 2] {
            let codes = reduce(View::Public, &domain);
            out.push((snapshot_frame(seq, e, &codes, &[]), View::Public));
        }
        if objects > 0 {
            let mut changes = {
                let k = r.below(200) as usize;
                random_changes(r, objects, k)
            };
            if r.chance(30) {
                changes.push(Change {
                    idx: objects - 1,
                    code: 15,
                });
            }
            coalesce(&mut changes);
            let from = seq.min(u64::MAX - 1);
            let to = from + 1 + u64::from(r.below(3)).min(u64::MAX - from - 1);
            let d = encode_delta(view, from, to, &changes, objects, &[]).unwrap();
            out.push((d, view));
            let mut idx: Vec<u32> = changes.iter().map(|c| c.idx).collect();
            idx.truncate(r.below(20) as usize);
            let p = encode_private(seq, &idx, objects, areas).unwrap();
            out.push((p, view));
        }
        out.push((encode_resync(), view));
        out.push((encode_ping(seq), view));
        out.push((encode_close(CloseReason(r.next() as u8)), view));
    }
    out
}

fn client_frames(r: &mut Rng) -> Vec<Vec<u8>> {
    let key = "Ü".repeat(MAX_KEY_BYTES / 2);
    let long = vec![0xAB; MAX_TOKEN_BYTES];
    let tokens: [Option<&[u8]>; 4] = [None, Some(&[]), Some(b"ht_abc"), Some(&long)];
    let mut out = Vec::new();
    for i in 0..40 {
        let s = Subscribe {
            version: [1, 2, 255][i % 3],
            event_key: [&key[..], "", "concert-2026"][i % 3],
            last_seq: seqs()[r.below(5) as usize],
            hold_token: tokens[r.below(4) as usize],
            session_token: tokens[r.below(4) as usize],
        };
        let f = encode_subscribe(&s).unwrap();
        assert_eq!(decode_client(&f), Ok(ClientFrame::Subscribe(s)));
        out.push(f);
        let nonce = seqs()[r.below(5) as usize];
        let f = encode_pong(nonce);
        assert_eq!(decode_client(&f), Ok(ClientFrame::Pong(nonce)));
        out.push(f);
    }
    out
}

#[test]
fn every_frame_round_trips() {
    let mut r = Rng(17);
    for _ in 0..20 {
        let objects = [0, 1, 300, MAX_OBJECTS][r.below(4) as usize];
        let hello = Hello {
            server_time_ms: seqs()[r.below(5) as usize],
            chart_hash: core::array::from_fn(|_| r.next() as u8),
            objects,
        };
        let f = encode_hello(&hello).unwrap();
        assert_eq!(server(&f, View::Public), ServerFrame::Hello(hello));

        let seq = seqs()[r.below(5) as usize];
        let n = r.below(objects.min(3_000) + 1);
        let mut changes = {
            let k = r.below(500) as usize;
            random_changes(&mut r, n.max(1), k)
        };
        coalesce(&mut changes);
        if n == 0 {
            changes.clear();
        }
        let areas: Vec<Area> = changes
            .iter()
            .step_by(7)
            .map(|c| Area {
                idx: c.idx,
                value: r.next() as u32,
            })
            .collect();
        let from = seq.min(u64::MAX - 1);
        let f = encode_delta(View::Manager, from, u64::MAX, &changes, n, &areas).unwrap();
        let ServerFrame::Delta(d) = server(&f, View::Manager) else {
            panic!("not a delta");
        };
        assert_eq!((d.from_seq, d.to_seq), (from, u64::MAX));
        assert_eq!(d.changes(n).unwrap().collect::<Vec<_>>(), changes);
        assert_eq!(d.areas(n).unwrap().collect::<Vec<_>>(), areas);

        let idx: Vec<u32> = changes.iter().map(|c| c.idx).collect();
        let f = encode_private(seq, &idx, n, &areas).unwrap();
        let ServerFrame::Private(p) = server(&f, View::Public) else {
            panic!("not a private frame");
        };
        assert_eq!(p.expires_at_ms, seq);
        assert_eq!(p.objects(n).unwrap().collect::<Vec<_>>(), idx);
        assert_eq!(p.areas(n).unwrap().collect::<Vec<_>>(), areas);

        let domain = statuses(&mut r, n as usize, 15);
        let f = encode_snapshot(View::Manager, seq, &domain, &areas).unwrap();
        let ServerFrame::Snapshot(s) = server(&f, View::Manager) else {
            panic!("not a snapshot");
        };
        assert_eq!(s.areas(n).unwrap().collect::<Vec<_>>(), areas);

        let nonce = r.next();
        assert_eq!(
            server(&encode_ping(nonce), View::Public),
            ServerFrame::Ping(nonce)
        );
        let reason = CloseReason(r.next() as u8);
        let f = encode_close(reason);
        assert_eq!(server(&f, View::Public), ServerFrame::Close(reason));
    }
    assert_eq!(server(&encode_resync(), View::Public), ServerFrame::Resync);
    // The largest change count a frame may carry.
    let all: Vec<Change> = (0..MAX_OBJECTS)
        .map(|idx| Change { idx, code: 1 })
        .collect();
    let f = encode_delta(View::Public, 0, 1, &all, MAX_OBJECTS, &[]).unwrap();
    let ServerFrame::Delta(d) = server(&f, View::Public) else {
        panic!("not a delta");
    };
    assert!(d.changes(MAX_OBJECTS).unwrap().eq(all.iter().copied()));
    client_frames(&mut r);
}

#[test]
fn versions() {
    assert_eq!(negotiate(0), None);
    assert_eq!(negotiate(1), Some(1));
    assert_eq!(negotiate(200), Some(PROTOCOL_VERSION));
    let s = Subscribe {
        version: 1,
        event_key: "e",
        last_seq: 5,
        hold_token: None,
        session_token: None,
    };
    let mut f = encode_subscribe(&s).unwrap();
    // A newer client may append fields; the same bytes from a version-1
    // client are trailing garbage.
    f.extend([9, 9, 9]);
    assert_eq!(decode_client(&f), Err(WireError::TrailingBytes));
    f[1] = 2;
    let Ok(ClientFrame::Subscribe(got)) = decode_client(&f) else {
        panic!("newer client refused");
    };
    assert_eq!((got.version, got.last_seq), (2, 5));
    f[1] = 0;
    assert_eq!(decode_client(&f), Err(WireError::UnsupportedVersion));
    // HELLO in a version this side does not speak is refused before its
    // fields are read; the version byte is always at offset 1.
    for version in [0, 2, 255] {
        assert_eq!(
            decode_server(&[0x01, version], View::Public),
            Err(WireError::UnsupportedVersion)
        );
    }
}

#[test]
fn hostile_frames_give_typed_errors() {
    use View::{Manager, Public};
    use WireError::*;
    let cat = |parts: &[&[u8]]| parts.concat();
    let hello = |version: u8, objects: u64| cat(&[&[0x01, version], &v(5), &[0; 16], &v(objects)]);
    // SNAPSHOT of `n` objects: encoding byte, then a payload of `len` bytes.
    let snap = |n: u64, enc: u8, payload: &[u8], tail: &[u8]| {
        cat(&[
            &[0x02],
            &v(1),
            &v(n),
            &[enc],
            &v(payload.len() as u64),
            payload,
            tail,
        ])
    };
    let delta = |from: u64, to: u64, body: &[u8]| cat(&[&[0x03], &v(from), &v(to), body]);
    let sub = |rest: &[u8]| cat(&[&[0x10, 1], rest]);
    let key = |k: &[u8]| cat(&[&v(k.len() as u64), k]);
    let over_u64 = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x02];
    let endless = [0x80; 11];
    let two_pow_32 = v(1 << 32);
    let a: &[u8] = &[0];

    let server_cases: Vec<(&str, Vec<u8>, View, WireError)> = vec![
        ("empty", vec![], Public, Truncated),
        ("type 0", vec![0x00], Public, UnknownType),
        ("type 0x08", vec![0x08, 1, 2], Public, UnknownType),
        ("client type", vec![0x10], Public, UnknownType),
        ("hello v0", hello(0, 1), Public, UnsupportedVersion),
        ("hello v2", hello(2, 1), Public, UnsupportedVersion),
        ("hello no version", vec![0x01], Public, Truncated),
        ("hello no time", vec![0x01, 1], Public, Truncated),
        (
            "hello short hash",
            cat(&[&[0x01, 1], &v(5), &[0; 15]]),
            Public,
            Truncated,
        ),
        (
            "hello no objects",
            cat(&[&[0x01, 1], &v(5), &[0; 16]]),
            Public,
            Truncated,
        ),
        (
            "hello too many objects",
            hello(1, u64::from(MAX_OBJECTS) + 1),
            Public,
            OverLimit,
        ),
        (
            "hello objects past u32",
            cat(&[&[0x01, 1], &v(5), &[0; 16], &two_pow_32]),
            Public,
            BadVarint,
        ),
        (
            "hello trailing",
            cat(&[&hello(1, 3), &[0]]),
            Public,
            TrailingBytes,
        ),
        (
            "varint 11 bytes",
            cat(&[&[0x06], &endless]),
            Public,
            BadVarint,
        ),
        (
            "varint past u64",
            cat(&[&[0x06], &over_u64]),
            Public,
            BadVarint,
        ),
        ("varint truncated", vec![0x06, 0x80], Public, Truncated),
        ("ping trailing", vec![0x06, 1, 0], Public, TrailingBytes),
        ("resync trailing", vec![0x05, 0], Public, TrailingBytes),
        ("close no reason", vec![0x07], Public, Truncated),
        ("close trailing", vec![0x07, 1, 1], Public, TrailingBytes),
        ("snapshot no seq", vec![0x02], Public, Truncated),
        (
            "snapshot no count",
            cat(&[&[0x02], &v(1)]),
            Public,
            Truncated,
        ),
        (
            "snapshot no encoding",
            cat(&[&[0x02], &v(1), &v(3)]),
            Public,
            Truncated,
        ),
        (
            "snapshot no length",
            cat(&[&[0x02], &v(1), &v(3), &[0]]),
            Public,
            Truncated,
        ),
        (
            "snapshot short payload",
            cat(&[&[0x02], &v(1), &v(3), &[0], &v(3), &[0, 0]]),
            Public,
            Truncated,
        ),
        (
            "snapshot no area count",
            snap(3, 0, &[0, 0, 0], &[]),
            Public,
            Truncated,
        ),
        (
            "snapshot too many objects",
            snap(u64::from(MAX_OBJECTS) + 1, 1, &[], a),
            Public,
            OverLimit,
        ),
        (
            "encoding 3",
            snap(3, 3, &[0, 0, 0], a),
            Public,
            UnknownEncoding,
        ),
        (
            "encoding 255",
            snap(3, 255, &[0, 0, 0], a),
            Manager,
            UnknownEncoding,
        ),
        (
            "bits on a manager stream",
            snap(3, 2, &[0], a),
            Manager,
            UnknownEncoding,
        ),
        ("raw too short", snap(3, 0, &[0, 0], a), Public, BadCount),
        (
            "raw too long",
            snap(3, 0, &[0, 0, 0, 0], a),
            Public,
            BadCount,
        ),
        (
            "raw public code 2",
            snap(3, 0, &[0, 2, 0], a),
            Public,
            BadValue,
        ),
        (
            "raw manager code 16",
            snap(3, 0, &[0, 16, 0], a),
            Manager,
            BadValue,
        ),
        ("bits one byte short", snap(9, 2, &[0], a), Public, BadCount),
        (
            "bits one byte long",
            snap(8, 2, &[0, 0], a),
            Public,
            BadCount,
        ),
        (
            "bits padding set",
            snap(9, 2, &[0, 0b10], a),
            Public,
            BadValue,
        ),
        (
            "runs past the count",
            snap(3, 1, &[0, 2, 1, 2], a),
            Public,
            BadCount,
        ),
        (
            "runs short of the count",
            snap(3, 1, &[0, 2], a),
            Public,
            BadCount,
        ),
        ("zero run", snap(3, 1, &[0, 0, 1, 3], a), Public, BadCount),
        ("run without length", snap(3, 1, &[0], a), Public, Truncated),
        (
            "run public code 2",
            snap(3, 1, &[2, 3], a),
            Public,
            BadValue,
        ),
        (
            "run past u32",
            snap(3, 1, &cat(&[&[0], &two_pow_32]), a),
            Public,
            BadVarint,
        ),
        (
            "snapshot area past objects",
            snap(3, 0, &[0, 0, 0], &[1, 3, 0]),
            Public,
            OutOfRange,
        ),
        (
            "snapshot areas repeat",
            snap(3, 0, &[0, 0, 0], &[2, 1, 0, 0, 0]),
            Public,
            NotIncreasing,
        ),
        (
            "snapshot area count too big",
            snap(3, 0, &[0, 0, 0], &[3, 0, 0]),
            Public,
            BadCount,
        ),
        (
            "snapshot trailing",
            snap(3, 0, &[0, 0, 0], &[0, 0]),
            Public,
            TrailingBytes,
        ),
        ("delta no to_seq", cat(&[&[0x03], &v(1)]), Public, Truncated),
        ("delta same seq", delta(4, 4, &[0, 0]), Public, BadSeq),
        ("delta backwards", delta(4, 3, &[0, 0]), Public, BadSeq),
        ("delta no count", delta(1, 2, &[]), Public, Truncated),
        (
            "delta count too big",
            delta(1, 2, &[3, 0, 1, 1, 1]),
            Public,
            BadCount,
        ),
        (
            "delta count over limit",
            delta(1, 2, &v(u64::from(MAX_OBJECTS) + 1)),
            Public,
            OverLimit,
        ),
        (
            "delta change without code",
            delta(1, 2, &[2, 0x80, 0x01, 1, 5]),
            Public,
            Truncated,
        ),
        (
            "delta repeated idx",
            delta(1, 2, &[2, 5, 1, 0, 1, 0]),
            Public,
            NotIncreasing,
        ),
        (
            "delta idx at the limit",
            delta(1, 2, &cat(&[&[1], &v(MAX_OBJECTS.into()), &[1], a])),
            Public,
            OutOfRange,
        ),
        (
            "delta idx past u32",
            delta(1, 2, &cat(&[&[1], &two_pow_32, &[1], a])),
            Public,
            BadVarint,
        ),
        (
            "delta public code 2",
            delta(1, 2, &[1, 0, 2, 0]),
            Public,
            BadValue,
        ),
        (
            "delta manager code 16",
            delta(1, 2, &[1, 0, 16, 0]),
            Manager,
            BadValue,
        ),
        (
            "delta no area count",
            delta(1, 2, &[1, 0, 1]),
            Public,
            Truncated,
        ),
        (
            "delta trailing",
            delta(1, 2, &[1, 0, 1, 0, 0]),
            Public,
            TrailingBytes,
        ),
        ("private no expiry", vec![0x04], Public, Truncated),
        (
            "private count too big",
            cat(&[&[0x04], &v(9), &[4, 1, 1]]),
            Public,
            BadCount,
        ),
        (
            "private repeated idx",
            cat(&[&[0x04], &v(9), &[2, 1, 0, 0]]),
            Public,
            NotIncreasing,
        ),
        (
            "private area count too big",
            cat(&[&[0x04], &v(9), &[0, 1, 0]]),
            Public,
            BadCount,
        ),
        (
            "private trailing",
            cat(&[&[0x04], &v(9), &[0, 0, 0]]),
            Public,
            TrailingBytes,
        ),
    ];
    for (name, frame, view, err) in server_cases {
        assert_eq!(decode_server(&frame, view), Err(err), "{name}");
    }

    let client_cases: Vec<(&str, Vec<u8>, WireError)> = vec![
        ("empty", vec![], Truncated),
        ("server type", encode_ping(1), UnknownType),
        ("type 0x12", vec![0x12], UnknownType),
        ("subscribe no version", vec![0x10], Truncated),
        (
            "subscribe v0",
            cat(&[&[0x10, 0], &key(b"e"), &[0, 0, 0]]),
            UnsupportedVersion,
        ),
        ("no key", sub(&[]), Truncated),
        ("key cut short", sub(&[3, b'a']), Truncated),
        (
            "key over the limit",
            sub(&key(&[b'a'; MAX_KEY_BYTES + 1])),
            OverLimit,
        ),
        (
            "key not UTF-8",
            sub(&cat(&[&key(&[0xFF]), &[0, 0, 0]])),
            BadValue,
        ),
        ("no last seq", sub(&key(b"e")), Truncated),
        (
            "no hold presence",
            sub(&cat(&[&key(b"e"), &[0]])),
            Truncated,
        ),
        ("presence 2", sub(&cat(&[&key(b"e"), &[0, 2]])), BadValue),
        (
            "hold token cut",
            sub(&cat(&[&key(b"e"), &[0, 1, 4, 1]])),
            Truncated,
        ),
        (
            "token over the limit",
            sub(&cat(&[
                &key(b"e"),
                &[0, 1],
                &key(&[1; MAX_TOKEN_BYTES + 1]),
            ])),
            OverLimit,
        ),
        (
            "no session presence",
            sub(&cat(&[&key(b"e"), &[0, 0]])),
            Truncated,
        ),
        (
            "session presence 2",
            sub(&cat(&[&key(b"e"), &[0, 0, 2]])),
            BadValue,
        ),
        (
            "session token over the limit",
            sub(&cat(&[
                &key(b"e"),
                &[0, 0, 1],
                &key(&[1; MAX_TOKEN_BYTES + 1]),
            ])),
            OverLimit,
        ),
        (
            "subscribe trailing",
            sub(&cat(&[&key(b"e"), &[0, 0, 0, 0]])),
            TrailingBytes,
        ),
        ("pong no nonce", vec![0x11], Truncated),
        ("pong trailing", vec![0x11, 1, 1], TrailingBytes),
    ];
    for (name, frame, err) in client_cases {
        assert_eq!(decode_client(&frame), Err(err), "{name}");
    }

    // Checks against the receiver's object count.
    let s = snapshot_frame(1, 1, &[0, 0, 1], &[]);
    let ServerFrame::Snapshot(s) = server(&s, Public) else {
        panic!("not a snapshot");
    };
    assert_eq!(s.runs(4).err(), Some(CountMismatch));
    assert_eq!(s.write_into(&mut [0; 2]), Err(CountMismatch));
    let d = delta(1, 2, &[2, 3, 1, 1, 0, 1, 7, 1]);
    let ServerFrame::Delta(d) = server(&d, Public) else {
        panic!("not a delta");
    };
    assert_eq!(d.changes(4).err(), Some(OutOfRange));
    assert!(d.changes(5).is_ok());
    assert_eq!(d.areas(7).err(), Some(OutOfRange));
    let p = cat(&[&[0x04], &v(9), &[1, 9, 1, 2, 1]]);
    let ServerFrame::Private(p) = server(&p, Public) else {
        panic!("not a private frame");
    };
    assert_eq!(p.objects(9).err(), Some(OutOfRange));
    assert_eq!(p.areas(2).err(), Some(OutOfRange));
    assert!(p.objects(10).is_ok() && p.areas(3).is_ok());

    // Encoders refuse what decoders would refuse.
    let c = |idx| Change { idx, code: 1 };
    assert_eq!(encode_delta(Public, 2, 2, &[], 1, &[]), Err(BadSeq));
    assert_eq!(
        encode_delta(Public, 1, 2, &[c(1), c(1)], 3, &[]),
        Err(NotIncreasing)
    );
    assert_eq!(encode_delta(Public, 1, 2, &[c(3)], 3, &[]), Err(OutOfRange));
    assert_eq!(encode_private(0, &[2, 1], 3, &[]), Err(NotIncreasing));
    let long = vec![0; MAX_TOKEN_BYTES + 1];
    let too_long = Subscribe {
        version: 1,
        event_key: "e",
        last_seq: 0,
        hold_token: Some(&long),
        session_token: None,
    };
    assert_eq!(encode_subscribe(&too_long), Err(OverLimit));
}

#[test]
fn every_strict_prefix_and_trailing_byte_is_refused() {
    let mut r = Rng(19);
    for (frame, view) in frames(&mut r) {
        for len in 0..frame.len() {
            let err = decode_server(&frame[..len], view).unwrap_err();
            assert!(
                matches!(err, WireError::Truncated | WireError::BadCount),
                "{err:?}"
            );
        }
        let longer = [&frame[..], &[0]].concat();
        assert_eq!(decode_server(&longer, view), Err(WireError::TrailingBytes));
    }
    for frame in client_frames(&mut r) {
        for len in 0..frame.len() {
            assert_eq!(decode_client(&frame[..len]), Err(WireError::Truncated));
        }
        // SUBSCRIBE from a newer client may carry appended fields.
        if frame[0] == 0x11 || frame[1] == 1 {
            let longer = [&frame[..], &[0]].concat();
            assert_eq!(decode_client(&longer), Err(WireError::TrailingBytes));
        }
    }
}

/// Everything an accepted server frame yields is in bounds for a receiver
/// with `objects` objects, and its codes are within the view's range.
fn check_accepted(frame: ServerFrame<'_>, view: View, objects: u32) {
    let max = match view {
        View::Public => 1,
        View::Manager => MAX_STATUS,
    };
    let ascending = |idx: &mut dyn Iterator<Item = u32>| {
        let mut prev = None;
        for i in idx {
            assert!(i < objects && prev < Some(i));
            prev = Some(i);
        }
    };
    match frame {
        ServerFrame::Snapshot(s) => {
            let Ok(runs) = s.runs(objects) else {
                assert_ne!(s.objects, objects);
                return;
            };
            let mut next = 0;
            for (range, code) in runs {
                assert!(range.start == next && range.end > next && code <= max);
                next = range.end;
            }
            assert_eq!(next, objects);
            let mut state = vec![0; objects as usize];
            s.write_into(&mut state).unwrap();
            // Accepted state re-encodes to the same state.
            let again = encode_snapshot(view, s.seq, &state, &[]).unwrap();
            assert_eq!(state_of(&again, view), state);
            if let Ok(a) = s.areas(objects) {
                ascending(&mut a.map(|a| a.idx));
            }
        }
        ServerFrame::Delta(d) => {
            assert!(d.from_seq < d.to_seq);
            if let Ok(changes) = d.changes(objects) {
                let changes: Vec<Change> = changes.collect();
                assert!(changes.iter().all(|c| c.code <= max));
                ascending(&mut changes.iter().map(|c| c.idx));
            }
            if let Ok(a) = d.areas(objects) {
                ascending(&mut a.map(|a| a.idx));
            }
        }
        ServerFrame::Private(p) => {
            if let Ok(mut idx) = p.objects(objects) {
                ascending(&mut idx);
            }
            if let Ok(a) = p.areas(objects) {
                ascending(&mut a.map(|a| a.idx));
            }
        }
        _ => {}
    }
}

/// Seeded byte flips, truncations and splices of valid frames: decoding
/// returns `Ok` or `Err` and never panics, and what it accepts is in bounds.
#[test]
fn mutated_frames_never_panic() {
    let mut r = Rng(0xF00D);
    let server_frames = frames(&mut r);
    let client = client_frames(&mut r);
    let mut accepted = 0;
    for round in 0..20_000 {
        let pick = r.below(server_frames.len() as u32) as usize;
        let mut f = if round % 4 == 3 {
            client[r.below(client.len() as u32) as usize].clone()
        } else {
            server_frames[pick].0.clone()
        };
        if f.is_empty() {
            continue;
        }
        match r.below(4) {
            0 => {
                for _ in 0..1 + r.below(3) {
                    let at = r.below(f.len() as u32) as usize;
                    f[at] ^= 1 << r.below(8);
                }
            }
            1 => f.truncate(r.below(f.len() as u32) as usize),
            2 => {
                let a = r.below(f.len() as u32) as usize;
                let b = r.below(f.len() as u32) as usize;
                let piece = f[a..(a + r.below(16) as usize).min(f.len())].to_vec();
                f.splice(b..b, piece);
            }
            _ => {
                let at = r.below(f.len() as u32) as usize;
                f[at] = r.next() as u8;
            }
        }
        for view in [View::Public, View::Manager] {
            if let Ok(frame) = decode_server(&f, view) {
                accepted += 1;
                for objects in [0, 1, 300, 2_000, MAX_OBJECTS] {
                    check_accepted(frame, view, objects);
                }
                if let ServerFrame::Snapshot(s) = frame {
                    check_accepted(frame, view, s.objects);
                }
            }
        }
        let _ = decode_client(&f);
    }
    assert!(
        accepted > 1_000,
        "some mutations should survive: {accepted}"
    );
}

/// Snapshot sizes and codec timings for 100,000 objects:
/// `cargo test -p cavea-format --release -- --ignored --nocapture wire_speed`.
#[test]
#[ignore = "timing, run by hand in release mode"]
fn wire_speed() {
    use std::hint::black_box;
    use std::time::Instant;
    let n = 100_000usize;
    let mut r = Rng(23);
    let free = vec![0u8; n];
    // 30% sold in blocks of 50 to 500 seats.
    let mut blocks = vec![0u8; n];
    while blocks.iter().filter(|&&s| s != 0).count() < n * 3 / 10 {
        let at = r.below(n as u32) as usize;
        let len = 50 + r.below(450) as usize;
        let status = if r.chance(80) { 2 } else { 1 };
        blocks[at..(at + len).min(n)].fill(status);
    }
    let scattered: Vec<u8> = (0..n).map(|_| u8::from(r.chance(50)) * 2).collect();
    for (name, state) in [
        ("all free", &free),
        ("30% sold in blocks", &blocks),
        ("50% sold at random", &scattered),
    ] {
        let codes = reduce(View::Public, state);
        let lens: Vec<usize> = (0..3).map(|e| payload(e, &codes).len()).collect();
        let chosen = encode_snapshot(View::Public, 1, state, &[]).unwrap();
        let ServerFrame::Snapshot(s) = server(&chosen, View::Public) else {
            panic!("not a snapshot");
        };
        let manager = encode_snapshot(View::Manager, 1, state, &[]).unwrap();
        println!(
            "{name}: raw {} B, runs {} B, bits {} B; public frame {} B (encoding {}), manager frame {} B",
            lens[0],
            lens[1],
            lens[2],
            chosen.len(),
            s.encoding as u8,
            manager.len()
        );
    }

    let runs = 200;
    let mut out = vec![0u8; n];
    for (name, state) in [("blocks", &blocks), ("scattered", &scattered)] {
        let start = Instant::now();
        for _ in 0..runs {
            black_box(encode_snapshot(View::Public, 1, black_box(state), &[]).unwrap());
        }
        let encode = start.elapsed() / runs;
        let frame = encode_snapshot(View::Public, 1, state, &[]).unwrap();
        let start = Instant::now();
        for _ in 0..runs {
            let ServerFrame::Snapshot(s) = decode_server(black_box(&frame), View::Public).unwrap()
            else {
                panic!("not a snapshot");
            };
            s.write_into(&mut out).unwrap();
            black_box(&out);
        }
        let decode = start.elapsed() / runs;
        println!("public snapshot, {name}: encode {encode:?}, decode and write {decode:?}");
    }

    let mut commits = random_changes(&mut r, n as u32, 10_530);
    let start = Instant::now();
    let mut delta = Vec::new();
    for _ in 0..runs {
        let mut c = commits.clone();
        coalesce(&mut c);
        delta = encode_delta(View::Public, 1, 2, &c, n as u32, &[]).unwrap();
    }
    let encode = start.elapsed() / runs;
    coalesce(&mut commits);
    let start = Instant::now();
    for _ in 0..runs {
        let ServerFrame::Delta(d) = decode_server(black_box(&delta), View::Public).unwrap() else {
            panic!("not a delta");
        };
        for c in d.changes(n as u32).unwrap() {
            out[c.idx as usize] = c.code;
        }
        black_box(&out);
    }
    let apply = start.elapsed() / runs;
    println!(
        "delta of {} changes ({} B): coalesce and encode {encode:?}, decode and apply {apply:?}",
        commits.len(),
        delta.len()
    );
}
