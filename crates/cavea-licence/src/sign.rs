//! Signing (feature `sign`): key generation, the signing key's file, and
//! issuing a licence file. Used by `cargo xtask licence` and tests only.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair as _};

use crate::{Invalid, Licence, PublicKey, VERSION, read_payload, signed_message};

/// The first line of a signing key's file.
pub const KEY_HEADER: &str = "cavea-licence-signing-key-v1";

/// A signing key could not be made or read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyError(&'static str);

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for KeyError {}

/// An Ed25519 signing key. Its `Debug` prints the public half only.
pub struct SigningKey(Ed25519KeyPair);

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SigningKey({})", self.public().id())
    }
}

impl SigningKey {
    /// A new key from the operating system's secure random source, and its
    /// file's text ([`Self::from_file`] reads it back).
    pub fn generate() -> Result<(Self, String), KeyError> {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .map_err(|_| KeyError("the system's random source failed"))?;
        let key = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
            .map_err(|_| KeyError("the generated key was rejected"))?;
        let text = format!("{KEY_HEADER}\n{}\n", STANDARD.encode(pkcs8.as_ref()));
        Ok((Self(key), text))
    }

    /// The key in a file [`Self::generate`] wrote.
    pub fn from_file(text: &str) -> Result<Self, KeyError> {
        let mut lines = text.lines();
        if lines.next() != Some(KEY_HEADER) {
            return Err(KeyError("not a Cavea licence signing key file"));
        }
        let pkcs8 = lines
            .next()
            .and_then(|line| STANDARD.decode(line.trim()).ok())
            .ok_or(KeyError("the key file's key does not decode"))?;
        if lines.any(|l| !l.trim().is_empty()) {
            return Err(KeyError("the key file has more than a key"));
        }
        Ed25519KeyPair::from_pkcs8(&pkcs8)
            .map(Self)
            .map_err(|_| KeyError("the key file's key is not an Ed25519 key"))
    }

    #[must_use]
    pub fn public(&self) -> PublicKey {
        let mut bytes = [0; 32];
        bytes.copy_from_slice(self.0.public_key().as_ref());
        PublicKey(bytes)
    }

    /// The licence file for `payload`, the licence document's bytes as they
    /// are to be signed, if they read as a licence (the checks
    /// [`crate::Verifier::verify`] makes after the signature).
    pub fn issue(&self, payload: &[u8]) -> Result<(Licence, Vec<u8>), Invalid> {
        let key_id = self.public().id();
        let licence = read_payload(payload, key_id.clone())?;
        let signature = self.0.sign(&signed_message(payload));
        let envelope = serde_json::json!({
            "cavea_licence": VERSION,
            "key_id": key_id,
            "payload": STANDARD.encode(payload),
            "signature": STANDARD.encode(signature.as_ref()),
        });
        let mut file = envelope.to_string().into_bytes();
        file.push(b'\n');
        if file.len() > crate::MAX_BYTES {
            return Err(Invalid::TooLarge);
        }
        Ok((licence, file))
    }
}
