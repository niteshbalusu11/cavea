//! Tests and the HTTP fuzz target only (feature `fuzzing`): hold requests and
//! responses to the document. The router tests' `send` calls [`exchange`] for
//! every request it makes, so every response a test receives, and every JSON
//! body the server accepted, is validated against the document's schema for
//! its route and status; the fuzz target does the same for each request it
//! generates.
//!
//! The JSON Schema check covers the keywords the document uses and refuses any
//! other, so the document cannot use one this check would ignore.

#![allow(clippy::unwrap_used, reason = "tests and the fuzz target only")]

use std::sync::OnceLock;

use serde_json::Value;

use super::document;

pub(crate) fn doc() -> &'static Value {
    static DOC: OnceLock<Value> = OnceLock::new();
    DOC.get_or_init(document)
}

/// The operation of `method` on a concrete `path`: literal segments before
/// parameters, as the router matches.
pub fn operation(method: &str, path: &str) -> Option<(&'static str, &'static Value)> {
    let segments: Vec<&str> = path.split('/').collect();
    let mut best: Option<(usize, &str, &Value)> = None;
    for (template, item) in doc()["paths"].as_object()? {
        let parts: Vec<&str> = template.split('/').collect();
        if parts.len() != segments.len() {
            continue;
        }
        let mut literals = 0;
        let matched = parts.iter().zip(&segments).all(|(p, s)| {
            if p.starts_with('{') {
                !s.is_empty()
            } else {
                literals += 1;
                p == s
            }
        });
        let op = item.get(method.to_ascii_lowercase());
        if let (true, Some(op)) = (matched, op)
            && best.is_none_or(|(l, ..)| literals > l)
        {
            best = Some((literals, template, op));
        }
    }
    best.map(|(_, template, op)| (template, op))
}

fn resolve(value: &Value) -> &Value {
    match value.get("$ref").and_then(Value::as_str) {
        Some(reference) => {
            let pointer = reference.strip_prefix('#').unwrap();
            resolve(
                doc()
                    .pointer(pointer)
                    .unwrap_or_else(|| panic!("{reference} unresolved")),
            )
        }
        None => value,
    }
}

fn essence(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn is_json(media: &str) -> bool {
    media == "application/json" || media.ends_with("+json")
}

/// Check one exchange. `request` is the JSON body sent, if any; it is checked
/// only when the server accepted it (2xx). Requests to paths the document
/// does not have (test-only routes) are not checked.
pub fn exchange(
    method: &str,
    path: &str,
    request: Option<&[u8]>,
    status: u16,
    content_type: Option<&str>,
    body: &[u8],
) -> Result<(), String> {
    let Some((template, op)) = operation(method, path) else {
        return Ok(());
    };
    let at = format!("{method} {template} → {status}");
    let response = op["responses"]
        .get(status.to_string())
        .map(resolve)
        .ok_or_else(|| format!("{at}: status not documented"))?;
    if let (Some(sent), true) = (request, (200..300).contains(&status)) {
        let schema = &resolve(&op["requestBody"])["content"]["application/json"]["schema"];
        if schema.is_null() {
            return Err(format!(
                "{at}: a JSON body was accepted but none is documented"
            ));
        }
        let sent: Value = serde_json::from_slice(sent).map_err(|e| format!("{at}: {e}"))?;
        validate(schema, &sent, "request").map_err(|e| format!("{at}: request body: {e}"))?;
    }
    let Some(content) = response.get("content").and_then(Value::as_object) else {
        return if body.is_empty() {
            Ok(())
        } else {
            Err(format!("{at}: a body where none is documented"))
        };
    };
    let media = essence(content_type.ok_or_else(|| format!("{at}: no Content-Type"))?);
    let entry = content
        .get(&media)
        .ok_or_else(|| format!("{at}: {media} not documented"))?;
    if is_json(&media) {
        let value: Value = serde_json::from_slice(body).map_err(|e| format!("{at}: {e}"))?;
        validate(&entry["schema"], &value, "response").map_err(|e| format!("{at}: {e}"))?;
        if media == "application/problem+json" {
            declared_reasons(&value).map_err(|e| format!("{at}: {e}"))?;
        }
    }
    Ok(())
}

/// A problem's `details.reason`, and each `details.objects[].reason`, must
/// be one the document lists for its `code` (`Problem`'s `x-cavea-reasons`
/// and `x-cavea-object-reasons`).
fn declared_reasons(problem: &Value) -> Result<(), String> {
    let schema = &doc()["components"]["schemas"]["Problem"];
    let code = problem["code"].as_str().unwrap_or_default();
    let listed = |map: &str, reason: &Value| {
        schema[map][code]
            .as_array()
            .is_some_and(|reasons| reasons.contains(reason))
    };
    let details = &problem["details"];
    if let Some(reason) = details.get("reason")
        && !listed("x-cavea-reasons", reason)
    {
        return Err(format!("reason {reason} is not declared for `{code}`"));
    }
    for object in details["objects"].as_array().into_iter().flatten() {
        if let Some(reason) = object.get("reason")
            && !listed("x-cavea-object-reasons", reason)
        {
            return Err(format!(
                "object reason {reason} is not declared for `{code}`"
            ));
        }
    }
    Ok(())
}

/// Annotation keywords: described, never checked.
const ANNOTATIONS: [&str; 3] = ["description", "format", "title"];

/// JSON Schema 2020-12, the subset the document uses.
pub(crate) fn validate(schema: &Value, value: &Value, at: &str) -> Result<(), String> {
    let schema = resolve(schema);
    let Some(schema) = schema.as_object() else {
        return Err(format!("{at}: schema is not an object"));
    };
    for (keyword, rule) in schema {
        let ok = match keyword.as_str() {
            k if ANNOTATIONS.contains(&k) || k.starts_with("x-") => true,
            "type" => match rule {
                Value::String(t) => is_type(t, value),
                Value::Array(ts) => ts.iter().any(|t| is_type(t.as_str().unwrap(), value)),
                _ => false,
            },
            "enum" => rule.as_array().unwrap().contains(value),
            "properties" => {
                if let Some(object) = value.as_object() {
                    for (name, property) in rule.as_object().unwrap() {
                        if let Some(v) = object.get(name) {
                            validate(property, v, &format!("{at}.{name}"))?;
                        }
                    }
                }
                true
            }
            "required" => value.as_object().is_none_or(|o| {
                rule.as_array()
                    .unwrap()
                    .iter()
                    .all(|n| o.contains_key(n.as_str().unwrap()))
            }),
            "additionalProperties" => match value.as_object() {
                None => true,
                Some(object) => {
                    let known = schema.get("properties").and_then(Value::as_object);
                    let extra = object
                        .iter()
                        .filter(|(k, _)| known.is_none_or(|p| !p.contains_key(*k)));
                    for (name, v) in extra {
                        match rule {
                            Value::Bool(false) => {
                                return Err(format!("{at}: undocumented field `{name}`"));
                            }
                            Value::Bool(true) => {}
                            _ => validate(rule, v, &format!("{at}.{name}"))?,
                        }
                    }
                    true
                }
            },
            "items" => {
                if let Some(items) = value.as_array() {
                    for (i, item) in items.iter().enumerate() {
                        validate(rule, item, &format!("{at}[{i}]"))?;
                    }
                }
                true
            }
            "oneOf" | "anyOf" => {
                let matched = rule
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|s| validate(s, value, at).is_ok())
                    .count();
                if keyword == "oneOf" {
                    matched == 1
                } else {
                    matched >= 1
                }
            }
            "minimum" => value.as_f64().is_none_or(|v| v >= rule.as_f64().unwrap()),
            "maximum" => value.as_f64().is_none_or(|v| v <= rule.as_f64().unwrap()),
            "minLength" => value
                .as_str()
                .is_none_or(|s| s.chars().count() as u64 >= rule.as_u64().unwrap()),
            "maxLength" => value
                .as_str()
                .is_none_or(|s| s.chars().count() as u64 <= rule.as_u64().unwrap()),
            "minItems" => value
                .as_array()
                .is_none_or(|a| a.len() as u64 >= rule.as_u64().unwrap()),
            "maxItems" => value
                .as_array()
                .is_none_or(|a| a.len() as u64 <= rule.as_u64().unwrap()),
            other => return Err(format!("{at}: keyword `{other}` is not checked")),
        };
        if !ok {
            return Err(format!("{at}: {value} fails `{keyword}`: {rule}"));
        }
    }
    Ok(())
}

fn is_type(t: &str, value: &Value) -> bool {
    match t {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        _ => false,
    }
}
