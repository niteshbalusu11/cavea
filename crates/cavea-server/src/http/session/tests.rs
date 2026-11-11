#![allow(clippy::unwrap_used, reason = "test code; a panic is a test failure")]

use serde_json::{Value, json};
use sha2::Digest as _;

use super::*;

/// Obviously fake: `sk_` and 32 times `ab`.
const SECRET: &str = "sk_abababababababababababababababababababababababababababababababab";
const KID: &str = "key_0123456789abcdef0123456789abcdef";
const EVENT: &str = "hamlet-2027-03-14";
/// The server clock, in Unix milliseconds.
const NOW: u64 = 1_800_000_000_000;

/// What `api_keys.hash` holds for [`SECRET`].
fn stored() -> [u8; 32] {
    Sha256::digest(SECRET).into()
}

/// HMAC-SHA-256 written out from RFC 2104, independently of the `hmac` crate.
fn rfc2104(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let pad = |byte: u8| block.iter().map(|b| b ^ byte).collect::<Vec<_>>();
    let inner = Sha256::new()
        .chain_update(pad(0x36))
        .chain_update(data)
        .finalize();
    Sha256::new()
        .chain_update(pad(0x5c))
        .chain_update(inner)
        .finalize()
        .into()
}

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn sign_raw(key: &[u8], header: &[u8], claims: &[u8]) -> String {
    let signed = format!("{}.{}", b64(header), b64(claims));
    format!("{signed}.{}", b64(&rfc2104(key, signed.as_bytes())))
}

fn sign(key: &[u8], header: &Value, claims: &Value) -> String {
    sign_raw(
        key,
        header.to_string().as_bytes(),
        claims.to_string().as_bytes(),
    )
}

fn header() -> Value {
    json!({ "alg": "HS256", "typ": "JWT", "kid": KID })
}

fn claims() -> Value {
    json!({ "sub": "buyer-42", "event": EVENT, "exp": NOW / 1000 + 600, "max_objects": 4 })
}

/// A token signed with the stored hash, after `change` edits its parts.
fn token(change: impl FnOnce(&mut Value, &mut Value)) -> String {
    let (mut h, mut c) = (header(), claims());
    change(&mut h, &mut c);
    sign(&stored(), &h, &c)
}

/// `max_objects` of the session, or the refusal.
type Outcome = Result<Option<u32>, Refusal>;

fn outcome(token: &str, key: Option<[u8; 32]>) -> Outcome {
    let token = parse(token)?;
    check(&token, key.as_ref(), Timestamp(NOW), EVENT).map(|s| s.max_objects)
}

/// Remove a field from a header or claims object.
fn without(v: &mut Value, field: &str) {
    v.as_object_mut().unwrap().remove(field);
}

#[test]
fn tokens_and_their_outcomes() {
    use Refusal::{Event, Expired, Invalid};
    let (ok, k, secs) = (Ok(Some(4)), Some(stored()), NOW / 1000);
    let valid = token(|_, _| {});
    let mut bad_signature = valid.clone();
    let last = bad_signature.pop().unwrap();
    bad_signature.push(if last == 'A' { 'B' } else { 'A' });
    let unsigned = format!(
        "{}.{}.",
        b64(br#"{"alg":"none"}"#),
        b64(claims().to_string().as_bytes())
    );
    let mut hex = String::new();
    push_hex(&mut hex, &stored());
    let p = valid.split('.').collect::<Vec<_>>();
    let other_alphabet = if valid.contains(['_', '-']) {
        Err(Invalid)
    } else {
        ok
    };
    #[rustfmt::skip]
    let cases: Vec<(&str, String, Option<[u8; 32]>, Outcome)> = vec![
        ("valid", valid.clone(), k, ok),
        // Keys longer than SHA-256's 64-byte block are hashed first (RFC 2104
        // §2): signing with the 67-byte key text is signing with its hash.
        ("signed with the key text", sign(SECRET.as_bytes(), &header(), &claims()), k, ok),
        ("signed with the hash as hex text", sign(hex.as_bytes(), &header(), &claims()), k, Err(Invalid)),
        ("no max_objects", token(|_, c| without(c, "max_objects")), k, Ok(None)),
        ("unknown claims and header fields", token(|h, c| { h["cty"] = json!("x"); c["iat"] = json!(1); c["nbf"] = json!(secs + 999); }), k, ok),
        ("channels", token(|_, c| c["channels"] = json!(["vip", "press"])), k, ok),
        ("expired", token(|_, c| c["exp"] = json!(secs - 30)), k, Err(Expired)),
        ("expired within the leeway", token(|_, c| c["exp"] = json!(secs - 29)), k, ok),
        ("another event", token(|_, c| c["event"] = json!("macbeth")), k, Err(Event)),
        ("unknown kid", valid.clone(), None, Err(Invalid)),
        ("bad signature", bad_signature, k, Err(Invalid)),
        ("alg none, unsigned", unsigned, k, Err(Invalid)),
        ("alg none, signed", token(|h, _| h["alg"] = json!("none")), k, Err(Invalid)),
        ("alg HS384", token(|h, _| h["alg"] = json!("HS384")), k, Err(Invalid)),
        ("alg RS256", token(|h, _| h["alg"] = json!("RS256")), k, Err(Invalid)),
        ("alg hs256", token(|h, _| h["alg"] = json!("hs256")), k, Err(Invalid)),
        ("alg missing", token(|h, _| without(h, "alg")), k, Err(Invalid)),
        ("crit", token(|h, _| h["crit"] = json!(["exp"])), k, Err(Invalid)),
        ("kid missing", token(|h, _| without(h, "kid")), k, Err(Invalid)),
        ("kid not a key id", token(|h, _| h["kid"] = json!("pk_0123")), k, Err(Invalid)),
        ("sub missing", token(|_, c| without(c, "sub")), k, Err(Invalid)),
        ("sub empty", token(|_, c| c["sub"] = json!("")), k, Err(Invalid)),
        ("sub of 128 characters", token(|_, c| c["sub"] = json!("é".repeat(128))), k, ok),
        ("sub of 129 characters", token(|_, c| c["sub"] = json!("é".repeat(129))), k, Err(Invalid)),
        ("sub not a string", token(|_, c| c["sub"] = json!(42)), k, Err(Invalid)),
        ("exp missing", token(|_, c| without(c, "exp")), k, Err(Invalid)),
        ("exp fractional", token(|_, c| c["exp"] = json!(secs as f64 + 0.5)), k, Err(Invalid)),
        ("event missing", token(|_, c| without(c, "event")), k, Err(Invalid)),
        ("max_objects 0", token(|_, c| c["max_objects"] = json!(0)), k, Err(Invalid)),
        ("max_objects negative", token(|_, c| c["max_objects"] = json!(-1)), k, Err(Invalid)),
        ("channels not a list", token(|_, c| c["channels"] = json!("vip")), k, Err(Invalid)),
        ("channel empty", token(|_, c| c["channels"] = json!([""])), k, Err(Invalid)),
        ("payload not JSON", sign_raw(&stored(), header().to_string().as_bytes(), b"{"), k, Err(Invalid)),
        ("payload not an object", sign(&stored(), &header(), &json!("x")), k, Err(Invalid)),
        ("payload changed after signing", format!("{}.{}.{}", p[0], b64(b"{}"), p[2]), k, Err(Invalid)),
        ("header not JSON", format!("{}.{}.{}", b64(b"{"), p[1], p[2]), k, Err(Invalid)),
        ("padded base64", format!("{}=.{}.{}", p[0], p[1], p[2]), k, Err(Invalid)),
        ("standard base64 alphabet", valid.replace('_', "/").replace('-', "+"), k, other_alphabet),
        ("two parts", format!("{}.{}", p[0], p[1]), k, Err(Invalid)),
        ("four parts", format!("{valid}.{}", p[2]), k, Err(Invalid)),
        ("empty", String::new(), k, Err(Invalid)),
    ];
    for (name, token, key, expected) in cases {
        assert_eq!(outcome(&token, key), expected, "{name}");
    }
}

/// The documented recipe, run by an independent implementation: the Python
/// script in the pull request (standard library only), with these arguments:
/// `SECRET KID hamlet-2027-03-14 buyer-42 600 4 4102444800`.
#[test]
fn a_token_minted_by_the_documented_recipe_verifies() {
    // The script's output: its two JSON texts, encoded, and its signature.
    let header = r#"{"alg":"HS256","typ":"JWT","kid":"key_0123456789abcdef0123456789abcdef"}"#;
    let claims =
        r#"{"sub":"buyer-42","event":"hamlet-2027-03-14","exp":4102444800,"max_objects":4}"#;
    let signature = "S1xq35OM5lNCnx_kOyT1bTEFtIPFnwM1i0V0Eg1Bxks";
    let minted = format!(
        "{}.{}.{signature}",
        b64(header.as_bytes()),
        b64(claims.as_bytes())
    );
    let session = check(
        &parse(&minted).unwrap(),
        Some(&stored()),
        Timestamp(NOW),
        EVENT,
    )
    .unwrap();
    assert_eq!(session.max_objects, Some(4));
    // Only the SHA-256 of `sub` is kept.
    let mut sub = String::new();
    push_hex(&mut sub, &Sha256::digest("buyer-42"));
    assert_eq!(session.sub, sub);
}

#[test]
fn a_token_is_at_most_1024_bytes() {
    let padded = |n: usize| token(|_, c| c["pad"] = json!("x".repeat(n)));
    let fits = (0..1024)
        .rev()
        .map(padded)
        .find(|t| t.len() <= MAX_TOKEN_BYTES)
        .unwrap();
    assert_eq!(outcome(&fits, Some(stored())), Ok(Some(4)));
    let over = (0..1024)
        .map(padded)
        .find(|t| t.len() > MAX_TOKEN_BYTES)
        .unwrap();
    assert_eq!(outcome(&over, Some(stored())), Err(Refusal::Invalid));
}
