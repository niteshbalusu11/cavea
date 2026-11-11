//! Pins the realtime frames `@cavea/renderer` encodes and reads in TypeScript
//! (`packages/renderer/src/wire.ts`) to `cavea_format::wire`: the fixture the
//! TypeScript tests read is checked here against the Rust encoders.

use cavea_format::wire::{
    self, Change, CloseReason, Hello, Subscribe, View, encode_close, encode_delta, encode_hello,
    encode_ping, encode_pong, encode_snapshot, encode_subscribe,
};

const FIXTURE: &str = include_str!("../../../packages/renderer/src/wire.fixture.json");

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn the_typescript_fixture_matches_the_rust_encoders() -> Result<(), wire::WireError> {
    // Fake tokens: a hold token's shape (`ht_` and 64 hex digits) and a JWT's.
    let hold_token = format!("ht_{}", "ab".repeat(32));
    let nonce = (1 << 63) + 5;
    let frames: Vec<(&str, Vec<u8>)> = vec![
        (
            "subscribe",
            encode_subscribe(&Subscribe {
                version: 1,
                event_key: "hamlet-2027-03-14",
                last_seq: 48_211,
                hold_token: Some(hold_token.as_bytes()),
                session_token: Some(b"eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.c2ln"),
            })?,
        ),
        (
            "subscribeBare",
            encode_subscribe(&Subscribe {
                version: 1,
                event_key: "Bühne",
                last_seq: 0,
                hold_token: None,
                session_token: None,
            })?,
        ),
        (
            "hello",
            encode_hello(&Hello {
                server_time_ms: 1_793_000_000_123,
                chart_hash: *b"\x9f\x2c\x4e\x1a\x7b\x3d\x5f\x6e\x8a\x0c\x2b\x4d\x6f\x8e\x0a\x1c",
                objects: 2000,
            })?,
        ),
        (
            "snapshot",
            encode_snapshot(View::Public, 48_211, &[0, 1, 0], &[])?,
        ),
        (
            "delta",
            encode_delta(
                View::Public,
                48_211,
                48_212,
                &[Change { idx: 1, code: 0 }],
                3,
                &[],
            )?,
        ),
        ("ping", encode_ping(nonce)),
        ("pong", encode_pong(nonce)),
        ("close", encode_close(CloseReason::NOT_FOUND)),
    ];
    let body: Vec<String> = frames
        .iter()
        .map(|(name, bytes)| format!("  \"{name}\": \"{}\"", hex(bytes)))
        .collect();
    let expected = format!("{{\n{}\n}}\n", body.join(",\n"));
    assert_eq!(
        FIXTURE, expected,
        "packages/renderer/src/wire.fixture.json is stale; replace it with the right side"
    );
    Ok(())
}
