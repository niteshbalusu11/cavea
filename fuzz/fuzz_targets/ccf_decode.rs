//! Arbitrary bytes into the CCF reader, as either artefact. Whatever it
//! accepts passes `Chart::check`, re-encodes, and re-decodes to the same chart;
//! the re-encoding is canonical (encoding the re-decoded chart gives the same
//! bytes). A file accepted as the full artefact is also accepted as the public
//! one, giving the same chart without the full-only data.

#![no_main]

use cavea_format::{Artefact, Chart, decode, encode};
use libfuzzer_sys::fuzz_target;

fn accepted(data: &[u8], want: Artefact) -> Option<Chart> {
    let chart = decode(data, want).ok()?;
    chart.check().expect("a decoded chart passes its own check");
    assert_eq!(chart.full.is_some(), want == Artefact::Full);
    let bytes = encode(&chart, want).expect("a decoded chart re-encodes");
    let again = decode(&bytes, want).expect("a re-encoded chart decodes");
    assert_eq!(again, chart, "re-encoding changes the chart");
    assert_eq!(
        encode(&again, want).ok(),
        Some(bytes),
        "re-encoding is not canonical"
    );
    Some(chart)
}

fuzz_target!(|data: &[u8]| {
    let public = accepted(data, Artefact::Public);
    if let Some(full) = accepted(data, Artefact::Full) {
        let stripped = Chart { full: None, ..full };
        assert_eq!(
            public.as_ref(),
            Some(&stripped),
            "full accepted, public not the same"
        );
    }
});
