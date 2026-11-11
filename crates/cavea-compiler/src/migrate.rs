//! Schema migrations (`docs/03-chart-format.md` §2.8). Parsing reads the
//! `schema` field first: the current schema parses directly into
//! [`ChartDocument`], an older one is migrated step by step as JSON and then
//! parsed, and any other is refused (E010).

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::document::{ChartDocument, DocumentError};

/// One step: rewrites a document of schema `n` into schema `n + 1` in place.
/// It never changes an `id`, `rid` or `oid`; the stored samples test that.
/// The `schema` field is set by the caller after the step.
type Step = fn(&mut Map<String, Value>) -> Result<(), &'static str>;

/// `STEPS[i]` migrates schema `i + 1` to `i + 2`. Schema 1 is the first, so
/// there is none yet; 03 §2.8 says how to add one.
const STEPS: &[Step] = &[];

/// The document schema this compiler reads and writes.
pub const SCHEMA: u32 = 1 + STEPS.len() as u32;

/// The `schema` field alone; every other field is skipped unread.
#[derive(Deserialize)]
struct Peek {
    schema: u32,
}

pub(crate) fn parse(bytes: &[u8]) -> Result<ChartDocument, DocumentError> {
    let schema = match serde_json::from_slice::<Peek>(bytes) {
        Ok(peek) => peek.schema,
        // Not JSON, or no integer `schema`: the typed parse says where.
        Err(_) => return serde_json::from_slice(bytes).map_err(DocumentError::Json),
    };
    if schema == SCHEMA {
        return serde_json::from_slice(bytes).map_err(DocumentError::Json);
    }
    if schema == 0 || schema > SCHEMA {
        return Err(DocumentError::Schema { schema });
    }
    let mut value: Value = serde_json::from_slice(bytes).map_err(DocumentError::Json)?;
    let Value::Object(map) = &mut value else {
        return Err(DocumentError::Schema { schema });
    };
    let first = schema as usize - 1; // schema ≥ 1
    for (from, step) in (schema..).zip(STEPS.get(first..).unwrap_or_default()) {
        step(map).map_err(|problem| DocumentError::Migration { from, problem })?;
        map.insert("schema".into(), Value::from(from + 1));
    }
    serde_json::from_value(value).map_err(DocumentError::Json)
}

#[cfg(test)]
mod tests;
