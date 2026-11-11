//! Arbitrary bytes into the realtime frame decoders: `decode_server` for both
//! views and `decode_client`. An accepted frame yields only what its bounds
//! allow (08 §8.6): codes within the view's range, every `idx` strictly
//! increasing and below the receiver's object count (or refused), a snapshot
//! state covering exactly its objects. Re-encoding what was decoded and
//! decoding again gives the same values.

#![no_main]

use cavea_core::limits::MAX_OBJECTS;
use cavea_format::wire::{
    self, Area, Change, ClientFrame, MAX_KEY_BYTES, MAX_TOKEN_BYTES, ServerFrame, View,
};
use libfuzzer_sys::fuzz_target;

fn ascending(idx: impl Iterator<Item = u32>, objects: u32) -> Vec<u32> {
    let all: Vec<u32> = idx.collect();
    assert!(all.iter().all(|&i| i < objects), "idx at or past {objects}");
    assert!(all.windows(2).all(|w| w[0] < w[1]), "idx not increasing");
    all
}

/// The values of an accepted DELTA or PRIVATE: its sequence numbers or expiry,
/// its `idx` (with codes) and its areas.
type Lists = (u64, u64, Vec<(u32, u8)>, Vec<(u32, u32)>);

/// Checks an accepted DELTA or PRIVATE against a receiver with all
/// `MAX_OBJECTS` objects and returns its values.
fn lists(frame: ServerFrame<'_>, max: u8) -> Option<Lists> {
    let (a, b, items, areas): (u64, u64, Vec<(u32, u8)>, Vec<Area>) = match frame {
        ServerFrame::Delta(d) => {
            assert!(d.from_seq < d.to_seq);
            let changes = d.changes(MAX_OBJECTS).expect("within MAX_OBJECTS");
            let changes: Vec<(u32, u8)> = changes.map(|c| (c.idx, c.code)).collect();
            let areas = d.areas(MAX_OBJECTS).expect("within MAX_OBJECTS").collect();
            (d.from_seq, d.to_seq, changes, areas)
        }
        ServerFrame::Private(p) => {
            let idx = p.objects(MAX_OBJECTS).expect("within MAX_OBJECTS");
            let areas = p.areas(MAX_OBJECTS).expect("within MAX_OBJECTS").collect();
            (p.expires_at_ms, 0, idx.map(|i| (i, 0)).collect(), areas)
        }
        _ => return None,
    };
    assert!(items.iter().all(|&(_, code)| code <= max));
    ascending(items.iter().map(|x| x.0), MAX_OBJECTS);
    ascending(areas.iter().map(|x| x.idx), MAX_OBJECTS);
    // A receiver whose objects end before the highest `idx` refuses the frame.
    let top = items
        .iter()
        .map(|x| x.0)
        .chain(areas.iter().map(|x| x.idx))
        .max();
    if let Some(top) = top {
        let refused = match frame {
            ServerFrame::Delta(d) => d.changes(top).is_err() && d.areas(top).is_err(),
            ServerFrame::Private(p) => p.objects(top).is_err() && p.areas(top).is_err(),
            _ => false,
        };
        assert!(refused, "idx {top} accepted by a receiver of {top} objects");
    }
    Some((
        a,
        b,
        items,
        areas.iter().map(|x| (x.idx, x.value)).collect(),
    ))
}

fn server(data: &[u8], view: View) {
    let Ok(frame) = wire::decode_server(data, view) else {
        return;
    };
    let max = if view == View::Public {
        1
    } else {
        wire::MAX_STATUS
    };
    match frame {
        ServerFrame::Snapshot(s) => {
            assert!(s.objects <= MAX_OBJECTS);
            let mut next = 0;
            for (range, code) in s.runs(s.objects).expect("its own count") {
                assert!(range.start == next && range.end > range.start && code <= max);
                next = range.end;
            }
            assert_eq!(next, s.objects, "runs do not cover the objects");
            assert!(s.runs(s.objects.wrapping_add(1)).is_err());
            let mut state = vec![0; s.objects as usize];
            s.write_into(&mut state).expect("its own count");
            let areas: Vec<Area> = s.areas(s.objects).expect("its own count").collect();
            ascending(areas.iter().map(|a| a.idx), s.objects);
            let bytes = wire::encode_snapshot(view, s.seq, &state, &areas).expect("re-encodes");
            let Ok(ServerFrame::Snapshot(t)) = wire::decode_server(&bytes, view) else {
                panic!("re-encoded snapshot refused");
            };
            let mut again = vec![0; s.objects as usize];
            t.write_into(&mut again).expect("same count");
            let areas_again: Vec<Area> = t.areas(t.objects).expect("same count").collect();
            assert_eq!((t.seq, again, areas_again), (s.seq, state, areas));
        }
        ServerFrame::Delta(_) => {
            let values = lists(frame, max).expect("a delta");
            let (from, to, items, areas) = values.clone();
            let changes: Vec<Change> = items
                .iter()
                .map(|&(idx, code)| Change { idx, code })
                .collect();
            let areas: Vec<Area> = areas
                .iter()
                .map(|&(idx, value)| Area { idx, value })
                .collect();
            let bytes = wire::encode_delta(view, from, to, &changes, MAX_OBJECTS, &areas);
            let bytes = bytes.expect("re-encodes");
            let again = wire::decode_server(&bytes, view).expect("decodes");
            assert_eq!(lists(again, max), Some(values));
        }
        ServerFrame::Private(_) => {
            let values = lists(frame, max).expect("a private frame");
            let (expiry, _, items, areas) = values.clone();
            let idx: Vec<u32> = items.iter().map(|x| x.0).collect();
            let areas: Vec<Area> = areas
                .iter()
                .map(|&(idx, value)| Area { idx, value })
                .collect();
            let bytes = wire::encode_private(expiry, &idx, MAX_OBJECTS, &areas);
            let bytes = bytes.expect("re-encodes");
            let again = wire::decode_server(&bytes, view).expect("decodes");
            assert_eq!(lists(again, max), Some(values));
        }
        ServerFrame::Hello(h) => {
            assert!(h.objects <= MAX_OBJECTS);
            let bytes = wire::encode_hello(&h).expect("re-encodes");
            assert_eq!(wire::decode_server(&bytes, view), Ok(frame));
        }
        ServerFrame::Resync | ServerFrame::Ping(_) | ServerFrame::Close(_) => {
            let bytes = match frame {
                ServerFrame::Ping(n) => wire::encode_ping(n),
                ServerFrame::Close(r) => wire::encode_close(r),
                _ => wire::encode_resync(),
            };
            assert_eq!(wire::decode_server(&bytes, view), Ok(frame));
        }
    }
}

fuzz_target!(|data: &[u8]| {
    server(data, View::Public);
    server(data, View::Manager);
    let Ok(frame) = wire::decode_client(data) else {
        return;
    };
    let bytes = match frame {
        ClientFrame::Subscribe(s) => {
            assert!(s.version > 0 && s.event_key.len() <= MAX_KEY_BYTES);
            assert!(s.event_key.chars().count() <= wire::MAX_KEY_CHARS);
            for token in [s.hold_token, s.session_token].into_iter().flatten() {
                assert!(token.len() <= MAX_TOKEN_BYTES);
            }
            wire::encode_subscribe(&s).expect("re-encodes")
        }
        ClientFrame::Pong(nonce) => wire::encode_pong(nonce),
    };
    assert_eq!(wire::decode_client(&bytes), Ok(frame));
});
