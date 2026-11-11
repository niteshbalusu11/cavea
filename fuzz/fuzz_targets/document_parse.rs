//! Arbitrary bytes as a chart document into the parser and, when it parses,
//! the compiler. A parsed document survives saving and loading again; a
//! refusal lists errors only; a compiled chart carries warnings only, and both
//! its artefacts decode to the same chart, under the content hashes stated.
//! Time and memory are bounded by libFuzzer's `-timeout` and `-rss_limit_mb`.

#![no_main]

use cavea_compiler::{ChartDocument, CompileError, Severity, compile};
use cavea_format::{Artefact, Chart, HEADER_LEN, content_hash, decode};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(doc) = ChartDocument::from_json(data) else {
        return;
    };
    let saved = doc.to_json().expect("a parsed document saves");
    let loaded = ChartDocument::from_json(&saved).expect("a saved document loads");
    assert_eq!(loaded, doc, "saving and loading changes the document");
    match compile(&doc) {
        Ok(c) => {
            assert!(c.findings.iter().all(|f| f.severity() == Severity::Warning));
            let public = decode(&c.public, Artefact::Public).expect("public artefact decodes");
            let full = decode(&c.full, Artefact::Full).expect("full artefact decodes");
            assert_eq!(public, Chart { full: None, ..full });
            for (bytes, hash) in [(&c.public, c.public_hash), (&c.full, c.full_hash)] {
                assert_eq!(content_hash(bytes), Some(hash));
                let digest = blake3::hash(&bytes[HEADER_LEN..]);
                assert_eq!(digest.as_bytes()[..16], hash);
            }
        }
        Err(CompileError::Invalid(findings)) => {
            assert!(!findings.is_empty());
            assert!(findings.iter().all(|f| f.severity() == Severity::Error));
        }
        Err(e @ CompileError::Encode(_)) => panic!("compiler bug: {e}"),
    }
});
