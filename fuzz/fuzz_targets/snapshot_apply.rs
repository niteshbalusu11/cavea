//! A valid chart plus arbitrary snapshot, delta and private frames, mixed with
//! selection, hold results and frames, into the renderer engine (13 §5). The
//! engine must not panic, and every buffer write and draw it makes must stay
//! inside the resources it created (`cavea_fuzz::Checked`).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    cavea_fuzz::run(&cavea_fuzz::ops(data));
});
