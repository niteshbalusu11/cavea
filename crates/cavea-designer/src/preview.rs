//! The preview (07 §11): the open chart compiled as the buyer's renderer
//! loads it, and sample seat states for it as realtime SNAPSHOT frames
//! (08 §8.1), so the shell hands both to `createSeatMap` with a
//! `customState` adapter and nothing of its own decodes either.

use cavea_edit::Session;
use cavea_format::{Artefact, wire};

/// The open chart's public artefact (03 §4), or `None` when it does not
/// compile (its errors are the validation panel's).
pub fn chart(session: &Session) -> Option<Vec<u8>> {
    cavea_compiler::compile(&session.document().snapshot())
        .ok()
        .map(|c| c.public)
}

/// A SNAPSHOT frame for `chart` (a public artefact) with about `sold` of
/// its objects unavailable (0 to 1), in runs of one to eight as sales
/// leave them, at `seq`. The same arguments give the same frame. `None`
/// for bytes that are not a compiled chart.
pub fn state(chart: &[u8], sold: f64, seq: u32) -> Option<Vec<u8>> {
    let chart = cavea_format::decode(chart, Artefact::Public).ok()?;
    let objects = chart.seat_pos.len() + chart.objects.len();
    let sold = if sold.is_finite() {
        sold.clamp(0.0, 1.0)
    } else {
        0.0
    };
    // A run of length n is sold with probability `sold`: the expected share
    // of objects sold is `sold` whatever the run lengths.
    let threshold = (sold * f64::from(u32::MAX)) as u32;
    let mut rng: u32 = 0x9E37_79B9;
    let mut statuses = Vec::with_capacity(objects);
    while statuses.len() < objects {
        let run = (next(&mut rng) % 8 + 1) as usize;
        let code = u8::from(sold > 0.0 && next(&mut rng) <= threshold);
        let n = run.min(objects - statuses.len());
        statuses.extend(std::iter::repeat_n(code, n));
    }
    wire::encode_snapshot(wire::View::Public, u64::from(seq), &statuses, &[]).ok()
}

/// xorshift32: deterministic, and enough for a picture of a half-sold house.
fn next(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

#[cfg(test)]
mod tests {
    use cavea_format::wire::{ServerFrame, View, decode_server};

    use super::*;

    /// The frames the preview's three sample states use carry one code
    /// per object of the chart, about the share asked for unavailable.
    #[test]
    fn sample_states_cover_every_object_at_about_the_share_asked() {
        let doc = cavea_testkit::presets::find("theatre")
            .expect("preset")
            .generate(cavea_testkit::presets::DEFAULT_SEED);
        let session = Session::import(&serde_json::to_string(&doc).expect("json")).expect("import");
        let ccf = chart(&session).expect("compiles");
        let decoded = cavea_format::decode(&ccf, Artefact::Public).expect("decodes");
        let objects = decoded.seat_pos.len() + decoded.objects.len();
        for (sold, seq) in [(0.0, 1), (0.5, 2), (0.95, 3), (1.0, 4)] {
            let frame = state(&ccf, sold, seq).expect("frame");
            assert_eq!(state(&ccf, sold, seq), Some(frame.clone()), "deterministic");
            let Ok(ServerFrame::Snapshot(s)) = decode_server(&frame, View::Public) else {
                panic!("not a snapshot");
            };
            assert_eq!(s.seq, u64::from(seq));
            let mut codes = vec![0; objects];
            s.write_into(&mut codes).expect("one code per object");
            let share = codes.iter().filter(|&&c| c != 0).count() as f64 / objects as f64;
            assert!((share - sold).abs() < 0.06, "{sold}: {share}");
        }
        assert_eq!(state(b"not a chart", 0.5, 1), None);
    }
}
