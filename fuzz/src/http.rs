//! The HTTP target's view of the OpenAPI document (`http_api`, 13 §5): each
//! operation's method, path template, parameters, body schema and the
//! credentials it accepts, in the document's order; and the input format,
//! shared with the seeds example.
//!
//! **Input format.** A sequence of records, each one request: a length byte
//! `n`, then `n` bytes. The record's first byte picks the operation
//! (`index % OPERATIONS`); the rest are the request's choices, read in order,
//! where a missing byte reads as 0 and 0 is always the most ordinary choice
//! (the right credential, the world's own event, a well-formed body). So a
//! seed is a list of operations, `[2, op, 0]` each, and libFuzzer's
//! mutations vary one request without shifting the others.

use serde_json::Value;

/// One operation of the document.
#[derive(Debug, Clone)]
pub struct Operation {
    pub id: String,
    /// Upper case.
    pub method: String,
    pub template: String,
    pub params: Vec<Param>,
    /// The JSON schema of a JSON body, resolved one level.
    pub json_body: Option<Value>,
    /// A body of bytes (an image upload).
    pub bytes_body: bool,
    /// Each alternative's schemes: one alternative must be met in full.
    pub security: Vec<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub place: Place,
    pub required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Path,
    Query,
    Header,
}

/// Left out: each hashes a password on a blocking thread (argon2, tens of
/// milliseconds), which would slow every input for routes the console's own
/// tests cover (13 §10).
pub const EXCLUDED: &[&str] = &[
    "consoleSignIn",
    "consoleSetup",
    "consolePassword",
    "acceptInvitation",
];

fn resolve<'a>(doc: &'a Value, value: &'a Value) -> &'a Value {
    match value.get("$ref").and_then(Value::as_str) {
        Some(r) => r
            .strip_prefix('#')
            .and_then(|p| doc.pointer(p))
            .unwrap_or(value),
        None => value,
    }
}

/// The operations the target generates, in the document's order.
pub fn operations(doc: &Value) -> Vec<Operation> {
    let mut out = Vec::new();
    let Some(paths) = doc.get("paths").and_then(Value::as_object) else {
        return out;
    };
    for (template, item) in paths {
        let Some(item) = item.as_object() else {
            continue;
        };
        for (method, op) in item {
            let Some(id) = op.get("operationId").and_then(Value::as_str) else {
                continue;
            };
            if EXCLUDED.contains(&id) {
                continue;
            }
            let params = op
                .get("parameters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    let p = resolve(doc, p);
                    let place = match p.get("in")?.as_str()? {
                        "path" => Place::Path,
                        "query" => Place::Query,
                        "header" => Place::Header,
                        _ => return None,
                    };
                    Some(Param {
                        name: p.get("name")?.as_str()?.to_owned(),
                        place,
                        required: p.get("required").and_then(Value::as_bool) == Some(true),
                    })
                })
                .collect();
            let content = op.pointer("/requestBody/content");
            let json_body = content
                .and_then(|c| c.pointer("/application~1json/schema"))
                .cloned();
            let bytes_body = content.is_some() && json_body.is_none();
            let security = op
                .get("security")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
                .map(|alt| alt.keys().cloned().collect())
                .collect();
            out.push(Operation {
                id: id.to_owned(),
                method: method.to_uppercase(),
                template: template.clone(),
                params,
                json_body,
                bytes_body,
                security,
            });
        }
    }
    out
}

/// A record for each operation id in `ids`, with no further choices: a seed.
pub fn seed(ops: &[Operation], ids: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for id in ids {
        let index = ops
            .iter()
            .position(|op| op.id == *id)
            .unwrap_or_else(|| panic!("no operation {id}"));
        out.extend([2, u8::try_from(index).expect("under 256 operations"), 0]);
    }
    out
}
