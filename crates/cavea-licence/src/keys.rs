//! The public keys every Cavea build trusts to sign licences (12 §7.2, D-80).
//!
//! **Empty until the owner adds the first one.** With no key here every
//! licence is `invalid` (`unknown_key`): the server runs in the "missing or
//! invalid" state, which never stops a sale (D-20). The signing keys are
//! generated and kept offline (`cargo xtask licence keygen`); only the public
//! half ever comes here, as the base64 line `keygen` prints, in a one-line pull
//! request. Its key id (the first 8 bytes of the key's SHA-256, in hex) is
//! computed, never written.
//!
//! Rotation: add the new key beside the old one and ship a release; issue new
//! licences with the new key; once every licence signed by the old key has
//! expired, or its holder has been sent a replacement, remove the old key in
//! a later release. A server keeps accepting every licence signed by a key in
//! its own build, so retiring a key only affects releases made after it.
//!
//! The list is compiled in, never read from configuration: whoever runs the
//! server controls its environment and files, and a key they could add there
//! would let them sign their own licences. Tests trust their own key through
//! `Verifier::new`, which only code can call.

/// Base64 Ed25519 public keys, 32 bytes each, oldest first; each with a
/// comment naming its key id and the date it was added.
pub const TRUSTED: &[&str] = &[];
