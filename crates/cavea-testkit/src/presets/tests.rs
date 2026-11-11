use super::*;

/// Generates `name` with the default seed twice, checks the two documents are
/// identical, survive JSON, and compile with exactly the preset's seats and
/// no findings, and returns the public artefact's hash in hex.
fn compiled_hash(name: &str) -> String {
    let preset = find(name).unwrap();
    let doc = preset.generate(DEFAULT_SEED);
    let json = doc.to_json().unwrap();
    assert_eq!(json, preset.generate(DEFAULT_SEED).to_json().unwrap());
    assert_eq!(ChartDocument::from_json(&json).unwrap(), doc);
    let compiled = cavea_compiler::compile(&doc).unwrap();
    assert_eq!(compiled.stats.seats, preset.seats);
    assert_eq!(compiled.findings, []);
    compiled
        .public_hash
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// Pinned: a change to a preset invalidates every benchmark comparison made
// with it, so it must be deliberate. Update the hash in the same change.
macro_rules! pinned {
    ($($test:ident: $name:literal => $hash:literal,)*) => {$(
        #[test]
        fn $test() {
            assert_eq!(compiled_hash($name), $hash);
        }
    )*};
}

pinned! {
    theatre: "theatre" => "4531290d7e3daed5bc092cc23d7a5ee5",
    arena: "arena" => "a1db3ca66c5bbd090fa1f91b9d3a9756",
    stadium_60k: "stadium-60k" => "29aa54adf1fb7f036f41ef5839fab80c",
    stadium_100k: "stadium-100k" => "53652cd26d909a7ae10b9a7b3953165f",
    stress_250k: "stress-250k" => "dd03f652a8c5b3a68f625e2b7fc0322c",
    stress_500k: "stress-500k" => "94cd6f49746d6f08744c65415aa9b529",
}

/// The seed changes the chart but never makes it warn: every category stays
/// in use and the geometry does not depend on it.
#[test]
fn other_seeds_compile_cleanly() {
    for name in ["theatre", "arena"] {
        let preset = find(name).unwrap();
        let default = preset.generate(DEFAULT_SEED);
        let mut differs = false;
        for seed in [0, 2, 3, 4, 5, 6, 7, u64::MAX] {
            let doc = preset.generate(seed);
            differs |= doc.nodes != default.nodes;
            let compiled = cavea_compiler::compile(&doc).unwrap();
            assert_eq!(compiled.stats.seats, preset.seats, "{name} seed {seed}");
            assert_eq!(compiled.findings, [], "{name} seed {seed}");
        }
        assert!(differs, "{name}: the seed changes nothing");
    }
}
