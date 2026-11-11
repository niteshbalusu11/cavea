use std::collections::BTreeSet;

use cavea_format::{Artefact, decode};

use super::*;
use crate::finding::{Code, Detail};
use crate::{CompileError, compile};

/// Every stored sample document, by schema (03 §2.8, 13 §4). A new schema
/// keeps these and adds its own.
const SAMPLES: [(u32, &str, &str); 2] = [
    (
        1,
        "small",
        include_str!("../../samples/schema-1/small.json"),
    ),
    (1, "full", include_str!("../../samples/schema-1/full.json")),
];

/// Every `id`, `rid` and `oid` a document's JSON names: `id`, `rid`, `oid`
/// and `tableOid` fields, each `oid` of `oids` runs, and `seatOverrides`
/// keys. These key names hold in every schema so far.
fn ids(v: &Value, out: &mut Vec<u64>) {
    match v {
        Value::Array(items) => items.iter().for_each(|x| ids(x, out)),
        Value::Object(map) => {
            for (key, x) in map {
                match (key.as_str(), x) {
                    ("id" | "rid" | "oid" | "tableOid", Value::Number(n)) => out.extend(n.as_u64()),
                    ("oids", Value::Array(runs)) => {
                        for run in runs {
                            let [first, count] = [0, 1].map(|i| run[i].as_u64().unwrap());
                            out.extend(first..first + count);
                        }
                    }
                    ("seatOverrides", Value::Object(o)) => {
                        out.extend(o.keys().map(|k| k.parse::<u64>().unwrap()));
                    }
                    _ => ids(x, out),
                }
            }
        }
        _ => {}
    }
}

/// Every stored sample migrates to the current schema, parses and compiles
/// without errors, and keeps every id, `rid` and `oid`; the full one holds
/// every node type the compiler compiles.
#[test]
fn stored_samples_migrate_parse_and_compile_keeping_their_ids() {
    for (schema, name, json) in SAMPLES {
        let raw: Value = serde_json::from_str(json).unwrap();
        assert_eq!(raw["schema"], schema, "{name}");
        let doc =
            ChartDocument::from_json(json.as_bytes()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(doc.schema, SCHEMA);
        let saved: Value = serde_json::from_slice(&doc.to_json().unwrap()).unwrap();
        let (mut before, mut after) = (Vec::new(), Vec::new());
        ids(&raw, &mut before);
        ids(&saved, &mut after);
        before.sort_unstable();
        after.sort_unstable();
        assert!(!before.is_empty());
        assert_eq!(before, after, "{name}: a migration changed an id");

        let compiled = compile(&doc).unwrap_or_else(|e| panic!("{name}: {e}"));
        let chart = decode(&compiled.full, Artefact::Full).unwrap();
        let known: BTreeSet<u64> = before.into_iter().collect();
        let oids = &chart.full.unwrap().oids;
        assert!(!oids.is_empty() && oids.iter().all(|&o| known.contains(&u64::from(o))));
    }
    for schema in 1..=SCHEMA {
        assert!(
            SAMPLES.iter().any(|s| s.0 == schema),
            "no sample of schema {schema}"
        );
    }
    let full = SAMPLES.iter().find(|s| s.1 == "full").unwrap().2;
    for kind in [
        "section", "rowBlock", "row", "shape", "text", "icon", "image",
    ] {
        assert!(full.contains(&format!(r#""type": "{kind}""#)), "{kind}");
    }
}

/// A schema this compiler does not read is E010, before the typed parse, so
/// a newer document's new fields do not hide it. Something that is not a
/// document at all is not E010.
#[test]
fn unknown_schemas_are_e010() {
    for schema in [0, SCHEMA + 1, u32::MAX] {
        let json = format!(r#"{{"schema":{schema},"name":"x","nextOid":1,"future":true}}"#);
        let err = ChartDocument::from_json(json.as_bytes()).unwrap_err();
        let f = err.finding().unwrap();
        assert_eq!(
            (f.code, f.node, f.detail),
            (Code::E010, None, Detail::Schema { schema })
        );
    }
    for json in [&b"{\"name\":\"x\"}"[..], b"[1]", b"{\"schema\":-1}"] {
        assert!(
            ChartDocument::from_json(json)
                .unwrap_err()
                .finding()
                .is_none()
        );
    }
    // A document built in code with another schema is refused by the compiler.
    let mut doc = ChartDocument::from_json(SAMPLES[0].2.as_bytes()).unwrap();
    doc.schema = SCHEMA + 1;
    let Err(CompileError::Invalid(f)) = compile(&doc) else {
        panic!()
    };
    assert_eq!(f.iter().map(|f| f.code).collect::<Vec<_>>(), [Code::E010]);
}
