#![allow(clippy::unwrap_used, reason = "tests")]

use super::sign::SigningKey;
use super::*;

/// A document as the tool writes it, with `extra` fields spliced in.
fn document(expires: &str, extra: &str) -> Vec<u8> {
    format!(
        r#"{{"licence_id":"lic_test_1","customer":"Test Customer","plan":"growth","features":["embed"],"domains":["tickets.example.com","*.Example.org"],"max_workspaces":null,"issued_at":"2027-01-10","expires_at":"{expires}","grace_days":30{extra}}}"#
    )
    .into_bytes()
}

fn key() -> SigningKey {
    SigningKey::generate().unwrap().0
}

fn issued(key: &SigningKey) -> Vec<u8> {
    key.issue(&document("2028-01-10", "")).unwrap().1
}

#[test]
fn an_issued_licence_verifies_with_its_key_only() {
    let key = key();
    let file = issued(&key);
    let licence = Verifier::new([key.public()]).verify(&file).unwrap();
    assert_eq!(licence.id, "lic_test_1");
    assert_eq!(licence.plan, "growth");
    assert_eq!(licence.domains, ["tickets.example.com", "*.example.org"]);
    assert_eq!(licence.max_workspaces, None);
    assert_eq!(licence.issued_at.to_string(), "2027-01-10");
    assert_eq!(licence.key_id, key.public().id());
    // Another key, or none: unknown.
    let other = self::key();
    assert_eq!(
        Verifier::new([other.public()]).verify(&file),
        Err(Invalid::UnknownKey)
    );
    assert_eq!(Verifier::new([]).verify(&file), Err(Invalid::UnknownKey));
    // Several keys (a rotation): the one named is used.
    let both = Verifier::new([other.public(), key.public()]);
    assert!(both.verify(&file).is_ok());
    assert!(both.verify(&issued(&other)).is_ok());
}

/// The production list: well formed, and today empty (D-80): no build
/// trusts a key until the owner adds one.
#[test]
fn trusted_keys_are_well_formed() {
    for key in TRUSTED {
        assert!(PublicKey::from_base64(key).is_some(), "{key}");
    }
    assert_eq!(Verifier::production().key_ids().len(), TRUSTED.len());
    let file = issued(&key());
    assert_eq!(
        Verifier::production().verify(&file),
        Err(Invalid::UnknownKey)
    );
}

/// Any change to a byte of an issued file, anywhere before its final
/// newline, makes it invalid; the bytes as issued verify. Each position is
/// tried with several replacements: a flipped low bit, case, high bit, and
/// each JSON structural character.
#[test]
fn any_mutation_of_a_signed_file_is_invalid() {
    let key = key();
    let verifier = Verifier::new([key.public()]);
    let file = issued(&key);
    assert!(verifier.verify(&file).is_ok());
    let body = file.len() - 1;
    assert_eq!(file[body], b'\n');
    let mut tried = 0;
    for at in 0..body {
        let original = file[at];
        let mut replacements = vec![original ^ 1, original ^ 0x20, original ^ 0x80];
        replacements.extend(b"\"\\{}[],: 0Aa=+/");
        for replacement in replacements {
            if replacement == original {
                continue;
            }
            let mut mutated = file.clone();
            mutated[at] = replacement;
            assert!(
                verifier.verify(&mutated).is_err(),
                "byte {at} {original:#x} -> {replacement:#x}"
            );
            tried += 1;
        }
    }
    assert!(tried > body * 10);
    // A signed payload changed by one bit and signed by nobody.
    let opened = open(&file).unwrap();
    for at in 0..opened.payload.len() {
        let mut payload = opened.payload.clone();
        payload[at] ^= 1;
        let forged = serde_json::json!({
            "cavea_licence": 1, "key_id": opened.key_id,
            "payload": STANDARD.encode(&payload),
            "signature": STANDARD.encode(&opened.signature),
        });
        assert_eq!(
            verifier.verify(forged.to_string().as_bytes()),
            Err(Invalid::Signature)
        );
    }
}

#[test]
fn hostile_files_are_invalid_with_a_reason() {
    let key = key();
    let verifier = Verifier::new([key.public()]);
    let file = issued(&key);
    let text = String::from_utf8(file.clone()).unwrap();
    let envelope: serde_json::Value = serde_json::from_str(&text).unwrap();
    let with = |field: &str, value: serde_json::Value| {
        let mut e = envelope.clone();
        e[field] = value;
        e.to_string().into_bytes()
    };
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    let huge = format!(
        r#"{{"pad":"{}",{}"#,
        "x".repeat(MAX_BYTES),
        text.trim_start_matches('{')
    );
    let duplicate = text.replacen(
        r#""cavea_licence":1,"#,
        r#""cavea_licence":1,"cavea_licence":1,"#,
        1,
    );
    assert_ne!(duplicate, text);
    let nested_unknown = text.replacen(
        '{',
        &format!(r#"{{"x":{}0{},"#, "[".repeat(17), "]".repeat(17)),
        1,
    );
    let cases: Vec<(Vec<u8>, Invalid)> = vec![
        (Vec::new(), Invalid::Malformed),
        (b"null".to_vec(), Invalid::Malformed),
        (b"[]".to_vec(), Invalid::Malformed),
        (vec![0xff, 0xfe, b'{'], Invalid::NotUtf8),
        (file[..file.len() / 2].to_vec(), Invalid::Malformed),
        (deep.into_bytes(), Invalid::TooLarge),
        (huge.into_bytes(), Invalid::TooLarge),
        (duplicate.into_bytes(), Invalid::Malformed),
        (nested_unknown.into_bytes(), Invalid::Malformed),
        (with("cavea_licence", 2.into()), Invalid::Version),
        (with("cavea_licence", "1".into()), Invalid::Malformed),
        (
            with("key_id", "0000000000000000".into()),
            Invalid::UnknownKey,
        ),
        (with("payload", "not base64!".into()), Invalid::Malformed),
        (with("signature", "AAAA".into()), Invalid::Signature),
        (
            with("signature", serde_json::Value::Null),
            Invalid::Malformed,
        ),
    ];
    for (bytes, reason) in cases {
        assert_eq!(
            verifier.verify(&bytes),
            Err(reason),
            "{}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(80)])
        );
    }
    // Deep nesting within the size cap.
    let deep = format!("{}{}", "[".repeat(8_000), "]".repeat(8_000));
    assert_eq!(verifier.verify(deep.as_bytes()), Err(Invalid::Malformed));
}

/// A key signing what is not a licence document: the tool refuses to, and a
/// file made anyway is invalid after its signature checks.
#[test]
fn signed_nonsense_is_invalid() {
    let key = key();
    for (payload, reason) in [
        (b"not json".to_vec(), Invalid::Payload),
        (b"{}".to_vec(), Invalid::Payload),
        (document("2028-02-30", ""), Invalid::Dates),
        (document("2026-01-10", ""), Invalid::Dates),
        (
            document("2028-01-10", r#","plan":"twice""#),
            Invalid::Payload,
        ),
        (
            format!(r#"{{"x":{}0{}}}"#, "[".repeat(20), "]".repeat(20)).into_bytes(),
            Invalid::Payload,
        ),
    ] {
        assert_eq!(key.issue(&payload).err(), Some(reason));
    }
}

/// A licence from a later version: unknown fields in the envelope and the
/// payload, an unknown plan and feature, are accepted and kept as text.
#[test]
fn newer_fields_are_ignored() {
    let key = key();
    let payload = document(
        "2028-01-10",
        r#","seats_hint":{"a":[1,2]},"support":"gold""#,
    );
    let payload = String::from_utf8(payload)
        .unwrap()
        .replace("growth", "enterprise")
        .replace("embed", "teleport");
    let (_, file) = key.issue(payload.as_bytes()).unwrap();
    let mut envelope: serde_json::Value = serde_json::from_slice(&file).unwrap();
    envelope["comment"] = "from a later tool".into();
    let licence = Verifier::new([key.public()])
        .verify(envelope.to_string().as_bytes())
        .unwrap();
    assert_eq!(licence.plan, "enterprise");
    assert_eq!(licence.features, ["teleport"]);
}

/// 12 §7.3's states against the clock, at each boundary to the millisecond.
#[test]
fn standing_follows_the_clock() {
    let key = key();
    let licence = Verifier::new([key.public()]).verify(&issued(&key)).unwrap();
    let day = |s: &str| Date::parse(s).unwrap().start_ms();
    assert_eq!(licence.expires_ms(), day("2028-01-11"));
    assert_eq!(licence.grace_ends_ms(), day("2028-02-10"));
    assert_eq!(licence.last_grace_day().to_string(), "2028-02-09");
    for (at, standing, early) in [
        (0, Standing::Valid, true),
        (day("2027-01-10") - 1, Standing::Valid, true),
        (day("2027-01-10"), Standing::Valid, false),
        (day("2028-01-11") - 1, Standing::Valid, false),
        (day("2028-01-11"), Standing::Grace, false),
        (day("2028-02-10") - 1, Standing::Grace, false),
        (day("2028-02-10"), Standing::Expired, false),
        (u64::MAX, Standing::Expired, false),
    ] {
        assert_eq!(licence.standing(at), standing, "{at}");
        assert_eq!(licence.before_issue(at), early, "{at}");
    }
    // No grace: expired the day after.
    let none = Licence {
        grace_days: 0,
        ..licence
    };
    assert_eq!(none.standing(day("2028-01-11")), Standing::Expired);
    assert_eq!(none.last_grace_day().to_string(), "2028-01-10");
}

#[test]
fn key_files_round_trip_and_refuse_anything_else() {
    let (key, text) = SigningKey::generate().unwrap();
    let read = SigningKey::from_file(&text).unwrap();
    assert_eq!(read.public(), key.public());
    assert_eq!(key.public().id().len(), 16);
    assert_eq!(
        PublicKey::from_base64(&key.public().to_base64()),
        Some(key.public())
    );
    // Debug never prints the private half.
    assert_eq!(
        format!("{key:?}"),
        format!("SigningKey({})", key.public().id())
    );
    for bad in [
        String::new(),
        text.replace(sign::KEY_HEADER, "something else"),
        format!("{text}more\n"),
        format!("{}\nAAAA\n", sign::KEY_HEADER),
    ] {
        assert!(SigningKey::from_file(&bad).is_err());
    }
}

#[test]
fn domain_patterns() {
    for ok in [
        "tickets.example.com",
        "*.example.com",
        "localhost",
        "a-b.c1.example",
    ] {
        assert!(valid_domain(ok), "{ok}");
    }
    for bad in [
        "",
        "*.",
        "*",
        "Tickets.example.com",
        "a.*.example.com",
        "**.example.com",
        "-a.example.com",
        "a..example.com",
        "https://example.com",
        "example.com:443",
        "example.com/",
        "exämple.com",
    ] {
        assert!(!valid_domain(bad), "{bad}");
    }
}
