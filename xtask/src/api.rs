//! The console's API types, generated from the server's committed OpenAPI
//! document (09 §9) as part of `cargo xtask types`. The document is ours and
//! regular (D-46), so this reads only the JSON Schema it uses: a keyword it
//! does not know stops the generation with an error rather than writing a
//! type that is quietly wrong.
//!
//! It writes two TypeScript files from the one document:
//!
//! - the console's (`OUTPUT`): a type per component schema, and for each
//!   operation its method, path, permission and roles (`OPERATIONS`) and the
//!   types of its input and reply (`Operations`). The two headers the
//!   console's request layer adds itself, `Cavea-Workspace` and
//!   `Cavea-Csrf-Token`, are left out of the inputs;
//! - `@cavea/server-sdk`'s (`SDK_OUTPUT`, 05 §1): the same for every operation
//!   a secret key or the admin key may call, and only the schemas those
//!   reach; its table gives each operation's credential, whether it takes an
//!   `Idempotency-Key` (the SDK's core sets that header itself, so it is not
//!   an input), whether it is a page (`after`), and the body its successes
//!   answer (`json`, `text`, or `bytes`, typed `ArrayBuffer`) and whether
//!   one may be empty.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Map, Value};

/// The document, relative to the workspace root.
pub const DOCUMENT: &str = "crates/cavea-server/openapi.json";
/// The console's generated file, relative to the workspace root.
pub const OUTPUT: &str = "apps/console/src/api/generated.ts";
/// The server SDK's generated file, relative to the workspace root.
pub const SDK_OUTPUT: &str = "packages/server-sdk/src/generated.ts";

/// Which client a file is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Client {
    Console,
    Sdk,
}

impl Client {
    /// Headers the client's request layer sets itself on every request that
    /// needs them: not inputs.
    fn layer_headers(self) -> &'static [&'static str] {
        match self {
            Self::Console => &["Cavea-Workspace", "Cavea-Csrf-Token"],
            Self::Sdk => &["Cavea-Workspace", "Cavea-Csrf-Token", "Idempotency-Key"],
        }
    }
}

/// Keywords a schema may carry; the ones that only constrain values
/// (bounds, formats) change no type.
const KEYWORDS: &[&str] = &[
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "enum",
    "$ref",
    "anyOf",
    "oneOf",
    "description",
    "format",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
];

const METHODS: &[&str] = &["get", "put", "post", "delete", "patch", "head", "options"];

/// The console's TypeScript for the document at `root`.
pub fn generate(root: &Path) -> Result<String> {
    render(&read(root)?, Client::Console)
}

/// The server SDK's TypeScript for the document at `root`.
pub fn generate_sdk(root: &Path) -> Result<String> {
    render(&read(root)?, Client::Sdk)
}

fn read(root: &Path) -> Result<Value> {
    let text = fs::read_to_string(root.join(DOCUMENT))
        .with_context(|| format!("cannot read {DOCUMENT}"))?;
    serde_json::from_str(&text).with_context(|| DOCUMENT.to_owned())
}

/// The credential an SDK operation takes: the admin key where the document
/// allows it, else a secret key; `None` for an operation neither may call.
fn credential(op: &Value) -> Option<&'static str> {
    let schemes: Vec<&str> = op
        .get("security")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .flat_map(|s| s.keys().map(String::as_str))
        .collect();
    if schemes.contains(&"adminKey") {
        Some("admin")
    } else if schemes.contains(&"secretKey") {
        Some("secret")
    } else {
        None
    }
}

/// The component schemas `value` refers to, directly or through other
/// components, added to `seen`.
fn reach(doc: &Value, value: &Value, seen: &mut BTreeSet<String>) -> Result<()> {
    match value {
        Value::Object(map) => {
            if let Some(r) = map.get("$ref").and_then(Value::as_str) {
                if let Some(name) = r.strip_prefix("#/components/schemas/") {
                    if seen.insert(name.to_owned()) {
                        let schema = doc
                            .pointer(&format!("/components/schemas/{name}"))
                            .ok_or_else(|| anyhow!("cannot resolve {r}"))?;
                        reach(doc, schema, seen)?;
                    }
                } else {
                    reach(doc, resolve(doc, value)?, seen)?;
                }
            }
            for (key, member) in map {
                if key != "$ref" {
                    reach(doc, member, seen)?;
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                reach(doc, item, seen)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn render(doc: &Value, client: Client) -> Result<String> {
    let mut out = String::from(match client {
        Client::Console => {
            "// Generated by `cargo xtask types` from crates/cavea-server/openapi.json.\n\
             // Do not edit: change the server's route table, regenerate the document,\n\
             // then run `cargo xtask types` (09 §9).\n\n"
        }
        Client::Sdk => {
            "// Generated by `cargo xtask types` from crates/cavea-server/openapi.json:\n\
             // the operations a secret key or the admin key may call (05 §1). Do not\n\
             // edit: change the server's route table, regenerate the document, then\n\
             // run `cargo xtask types`.\n\n"
        }
    });
    let paths = object(doc.get("paths"), "paths")?;
    // The SDK's operations, and the schemas they reach.
    let mut reached = BTreeSet::new();
    if client == Client::Sdk {
        for item in paths.values() {
            for op in object(Some(item), "path item")?.values() {
                if credential(op).is_some() {
                    reach(doc, op, &mut reached)?;
                }
            }
        }
    }
    let schemas = object(doc.pointer("/components/schemas"), "components.schemas")?;
    for (name, schema) in schemas {
        if client == Client::Sdk && !reached.contains(name) {
            continue;
        }
        push_doc(&mut out, schema.get("description"), "");
        let ty = ts(schema, "").with_context(|| format!("schema {name}"))?;
        let plain_object = schema.get("type").and_then(Value::as_str) == Some("object")
            && schema.get("properties").is_some();
        if plain_object {
            let _ = writeln!(out, "export interface {name} {ty}\n");
        } else {
            let _ = writeln!(out, "export type {name} = {ty};\n");
        }
    }

    let mut table = String::new();
    let mut types = String::new();
    for (path, item) in paths {
        let item = object(Some(item), path)?;
        for (method, op) in item {
            if !METHODS.contains(&method.as_str()) {
                bail!("{path}: unexpected member `{method}`");
            }
            let id = op
                .get("operationId")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("{method} {path}: no operationId"))?;
            if client == Client::Sdk && credential(op).is_none() {
                continue;
            }
            let entry = Entry {
                path,
                method,
                id,
                op,
            };
            operation(doc, client, &entry, &mut table, &mut types)
                .with_context(|| format!("{method} {path}"))?;
        }
    }
    let table_doc = match client {
        Client::Console => {
            "/** Each operation's method and path, and who may call it: the permission\n \
             * a console session's workspace must grant, `null` when the route takes\n \
             * no staff credential. */\n"
        }
        Client::Sdk => {
            "/** Each operation's method and path; its credential (`admin` where the\n \
             * admin key may call it, else `secret`); whether it takes an\n \
             * `Idempotency-Key`, which the client sets; whether it is a page of a\n \
             * list (`after`); the body its successes answer (`json`, `text` or\n \
             * `bytes`) and whether one may be empty. */\n"
        }
    };
    let _ = write!(
        out,
        "/** Each operation's input (path and query parameters, headers other than\n \
         * the request layer's, body) and reply. */\n\
         export interface Operations {{\n{types}}}\n\n\
         export type OperationId = keyof Operations;\n\n\
         {table_doc}\
         export const OPERATIONS = {{\n{table}}} as const;\n"
    );
    Ok(out)
}

/// One operation of the document.
struct Entry<'a> {
    path: &'a str,
    method: &'a str,
    id: &'a str,
    op: &'a Value,
}

fn operation(
    doc: &Value,
    client: Client,
    entry: &Entry<'_>,
    table: &mut String,
    types: &mut String,
) -> Result<()> {
    let &Entry {
        path,
        method,
        id,
        op,
    } = entry;
    let permission = match op.get("x-cavea-permission") {
        None => "null".to_owned(),
        Some(p) => quote(
            p.as_str()
                .ok_or_else(|| anyhow!("permission is not text"))?,
        ),
    };
    let roles = op
        .get("x-cavea-roles")
        .and_then(Value::as_array)
        .map(|r| {
            r.iter()
                .map(|v| {
                    v.as_str()
                        .map(quote)
                        .ok_or_else(|| anyhow!("role is not text"))
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    let mut idempotency_key = false;
    let mut paginated = false;

    // Parameters by where they go.
    let mut groups: [(&str, Vec<String>, bool); 3] = [
        ("path", Vec::new(), false),
        ("query", Vec::new(), false),
        ("headers", Vec::new(), false),
    ];
    for param in op
        .get("parameters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let param = resolve(doc, param)?;
        let name = text(param, "name")?;
        let place = text(param, "in")?;
        idempotency_key |= place == "header" && name == "Idempotency-Key";
        paginated |= place == "query" && name == "after";
        if place == "header" && client.layer_headers().contains(&name) {
            continue;
        }
        let slot = match place {
            "path" => 0,
            "query" => 1,
            "header" => 2,
            "cookie" => continue,
            other => bail!("parameter {name} in `{other}`"),
        };
        let required = param.get("required").and_then(Value::as_bool) == Some(true);
        let schema = param
            .get("schema")
            .ok_or_else(|| anyhow!("parameter {name} has no schema"))?;
        let ty = ts(schema, "      ").with_context(|| format!("parameter {name}"))?;
        let group = &mut groups[slot];
        group.1.push(format!(
            "{}{}: {ty}",
            key(name),
            if required { "" } else { "?" }
        ));
        group.2 |= required;
    }
    let mut input = Vec::new();
    for (name, fields, required) in &groups {
        if fields.is_empty() {
            continue;
        }
        input.push(format!(
            "{name}{}: {{ {} }}",
            if *required { "" } else { "?" },
            fields.join("; ")
        ));
    }
    if let Some(body) = op.get("requestBody") {
        let required = body.get("required").and_then(Value::as_bool) == Some(true);
        let ty = content(body.get("content"), "Blob")?.unwrap_or_else(|| "never".to_owned());
        input.push(format!("body{}: {ty}", if required { "" } else { "?" }));
    }

    // The reply: every success status, a JSON body by its schema, another
    // body as text (for the SDK, text as text and anything else as bytes),
    // none as `undefined`.
    let mut replies: Vec<String> = Vec::new();
    // For the SDK: the body its successes answer (`json`, `text`, `bytes`),
    // and whether one may be empty; it refuses any other answer.
    let mut kind = "json";
    let mut empty = false;
    for (status, response) in object(op.get("responses"), "responses")? {
        if !(status.starts_with('1') || status.starts_with('2')) {
            continue;
        }
        let response = resolve(doc, response)?;
        let media: Vec<&str> = response
            .get("content")
            .and_then(Value::as_object)
            .map(|c| c.keys().map(String::as_str).collect())
            .unwrap_or_default();
        empty |= media.is_empty();
        let bytes = media
            .iter()
            .any(|m| *m != "application/json" && !m.starts_with("text/"));
        if bytes {
            kind = "bytes";
        } else if kind == "json" && media.iter().any(|m| m.starts_with("text/")) {
            kind = "text";
        }
        let other = if client == Client::Sdk && bytes {
            "ArrayBuffer"
        } else {
            "string"
        };
        let ty = content(response.get("content"), other)?.unwrap_or_else(|| "undefined".to_owned());
        if !replies.contains(&ty) {
            replies.push(ty);
        }
    }
    if replies.is_empty() {
        bail!("no success response");
    }
    match client {
        Client::Console => {
            let _ = writeln!(
                table,
                "  {id}: {{ method: {}, path: {}, permission: {permission}, roles: [{}] }},",
                quote(&method.to_uppercase()),
                quote(path),
                roles.join(", ")
            );
        }
        Client::Sdk => {
            let credential = credential(op).ok_or_else(|| anyhow!("no SDK credential"))?;
            let _ = writeln!(
                table,
                "  {id}: {{ method: {}, path: {}, credential: {}, idempotencyKey: {idempotency_key}, paginated: {paginated}, reply: {}, empty: {empty} }},",
                quote(&method.to_uppercase()),
                quote(path),
                quote(credential),
                quote(kind),
            );
        }
    }
    let input = if input.is_empty() {
        "Record<string, never>".to_owned()
    } else {
        format!("{{ {} }}", input.join("; "))
    };
    let _ = writeln!(
        types,
        "  {id}: {{\n    input: {input};\n    reply: {};\n  }};",
        replies.join(" | ")
    );
    Ok(())
}

/// The type of a request or response `content`: the JSON schema's, or
/// `other` for any other media type; `None` without content.
fn content(content: Option<&Value>, other: &str) -> Result<Option<String>> {
    let Some(content) = content else {
        return Ok(None);
    };
    let content = object(Some(content), "content")?;
    if let Some(json) = content.get("application/json") {
        let schema = json
            .get("schema")
            .ok_or_else(|| anyhow!("application/json without a schema"))?;
        return ts(schema, "    ").map(Some);
    }
    Ok(Some(other.to_owned()))
}

/// The TypeScript type of `schema`, its nested lines indented by `pad`.
fn ts(schema: &Value, pad: &str) -> Result<String> {
    let map = object(Some(schema), "schema")?;
    if let Some(unknown) = map.keys().find(|k| !KEYWORDS.contains(&k.as_str())) {
        bail!("unsupported keyword `{unknown}`");
    }
    if let Some(r) = map.get("$ref") {
        let r = r.as_str().ok_or_else(|| anyhow!("$ref is not text"))?;
        return r
            .strip_prefix("#/components/schemas/")
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("unsupported $ref {r}"));
    }
    for union in ["anyOf", "oneOf"] {
        if let Some(members) = map.get(union) {
            let members = members
                .as_array()
                .ok_or_else(|| anyhow!("{union} is not a list"))?;
            return Ok(join(
                members.iter().map(|m| ts(m, pad)).collect::<Result<_>>()?,
            ));
        }
    }
    if let Some(values) = map.get("enum") {
        let values = values
            .as_array()
            .ok_or_else(|| anyhow!("enum is not a list"))?;
        let literals = values
            .iter()
            .map(|v| match v {
                Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null => {
                    Ok(v.to_string())
                }
                _ => bail!("enum value {v} is not a literal"),
            })
            .collect::<Result<Vec<_>>>()?;
        return Ok(join(literals));
    }
    let kinds: Vec<&str> = match map.get("type") {
        None => return Ok("unknown".to_owned()),
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(ts)) => ts
            .iter()
            .map(|t| t.as_str().ok_or_else(|| anyhow!("type is not text")))
            .collect::<Result<_>>()?,
        Some(other) => bail!("type {other}"),
    };
    let mut out = Vec::new();
    for kind in kinds {
        out.push(match kind {
            "string" => "string".to_owned(),
            "integer" | "number" => "number".to_owned(),
            "boolean" => "boolean".to_owned(),
            "null" => "null".to_owned(),
            "array" => {
                let items = map
                    .get("items")
                    .ok_or_else(|| anyhow!("array without items"))?;
                let item = ts(items, pad)?;
                if is_simple(&item) {
                    format!("{item}[]")
                } else {
                    format!("Array<{item}>")
                }
            }
            "object" => object_type(map, pad)?,
            other => bail!("type `{other}`"),
        });
    }
    Ok(join(out))
}

fn object_type(map: &Map<String, Value>, pad: &str) -> Result<String> {
    let extra = map.get("additionalProperties");
    let Some(properties) = map.get("properties") else {
        return match extra {
            Some(Value::Object(schema)) => Ok(format!(
                "Record<string, {}>",
                ts(&Value::Object(schema.clone()), pad)?
            )),
            None | Some(Value::Bool(true)) => Ok("Record<string, unknown>".to_owned()),
            Some(other) => bail!("additionalProperties {other} without properties"),
        };
    };
    match extra {
        None | Some(Value::Bool(false)) => {}
        Some(other) => bail!("additionalProperties {other} beside properties"),
    }
    let required: Vec<&str> = map
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let properties = object(Some(properties), "properties")?;
    if let Some(missing) = required.iter().find(|r| !properties.contains_key(**r)) {
        bail!("required `{missing}` is not a property");
    }
    let inner = format!("{pad}  ");
    let mut out = String::from("{\n");
    for (name, schema) in properties {
        push_doc(&mut out, schema.get("description"), &inner);
        let ty = ts(schema, &inner).with_context(|| format!("property {name}"))?;
        let optional = if required.contains(&name.as_str()) {
            ""
        } else {
            "?"
        };
        let _ = writeln!(out, "{inner}{}{optional}: {ty};", key(name));
    }
    out.push_str(pad);
    out.push('}');
    Ok(out)
}

/// `/components/parameters/…` or `/components/responses/…` for a `$ref`.
fn resolve<'a>(doc: &'a Value, value: &'a Value) -> Result<&'a Value> {
    match value.get("$ref").and_then(Value::as_str) {
        None => Ok(value),
        Some(r) => r
            .strip_prefix('#')
            .and_then(|pointer| doc.pointer(pointer))
            .ok_or_else(|| anyhow!("cannot resolve {r}")),
    }
}

fn object<'a>(value: Option<&'a Value>, what: &str) -> Result<&'a Map<String, Value>> {
    value
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("{what} is not an object"))
}

fn text<'a>(value: &'a Value, member: &str) -> Result<&'a str> {
    value
        .get(member)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("no `{member}`"))
}

fn join(members: Vec<String>) -> String {
    let mut seen: Vec<String> = Vec::new();
    for m in members {
        if !seen.contains(&m) {
            seen.push(m);
        }
    }
    seen.join(" | ")
}

fn is_simple(ty: &str) -> bool {
    ty.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn quote(s: &str) -> String {
    Value::String(s.to_owned()).to_string()
}

/// A property name as written: bare when it is an identifier.
fn key(name: &str) -> String {
    let ident = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if ident { name.to_owned() } else { quote(name) }
}

fn push_doc(out: &mut String, description: Option<&Value>, pad: &str) {
    let Some(text) = description.and_then(Value::as_str) else {
        return;
    };
    // `*/` would end the comment early.
    let text = text.replace("*/", "*\\/");
    let lines: Vec<&str> = text.lines().collect();
    if let [line] = lines.as_slice() {
        let _ = writeln!(out, "{pad}/** {line} */");
        return;
    }
    let _ = writeln!(out, "{pad}/**");
    for line in lines {
        let _ = writeln!(out, "{pad} * {line}");
    }
    let _ = writeln!(out, "{pad} */");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The committed file is what the document gives: a change to the
    /// server's routes or schemas must come with `cargo xtask types`.
    #[test]
    fn generated_api_types_are_current() {
        let root = crate::root();
        let fresh = generate(&root).unwrap();
        let committed = fs::read_to_string(root.join(OUTPUT)).unwrap_or_default();
        assert!(
            committed == fresh,
            "{OUTPUT} is stale: run `cargo xtask types`"
        );
        let fresh = generate_sdk(&root).unwrap();
        let committed = fs::read_to_string(root.join(SDK_OUTPUT)).unwrap_or_default();
        assert!(
            committed == fresh,
            "{SDK_OUTPUT} is stale: run `cargo xtask types`"
        );
    }

    /// A schema keyword the generator does not model stops it, rather than
    /// being dropped from the type.
    #[test]
    fn refuses_what_it_does_not_model() {
        let doc =
            |schema: Value| json!({ "components": { "schemas": { "A": schema } }, "paths": {} });
        assert!(
            render(
                &doc(json!({ "type": "string", "pattern": "^a" })),
                Client::Console
            )
            .is_err()
        );
        assert!(render(&doc(json!({ "allOf": [] })), Client::Console).is_err());
        let ok = render(
            &doc(json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["a"],
                "properties": {
                    "a": { "type": ["string", "null"] },
                    "b-c": { "enum": ["x", "y"] },
                },
            })),
            Client::Console,
        )
        .unwrap();
        assert!(
            ok.contains("export interface A {\n  a: string | null;\n  \"b-c\"?: \"x\" | \"y\";\n}")
        );
    }
}
