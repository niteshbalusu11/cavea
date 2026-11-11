//! `cargo xtask types`: TypeScript types generated from the Rust definitions
//! (D-41). It reads the Rust source with `syn`, finds every type reachable
//! from a target's call table and roots, and models the JSON serde gives
//! each one from its `#[serde]` attributes: renames, enum tagging, fields
//! skipped when empty (absent, not `null`, as `exactOptionalPropertyTypes`
//! needs). A type that derives `Serialize` is described as serde writes it;
//! one that only derives `Deserialize`, as serde reads it (fields with a
//! default may be left out). An attribute it does not model, a type it
//! cannot resolve or a shape serde would refuse stops the generation with
//! an error, so the output is never quietly wrong.
//!
//! It writes two files per target: the TypeScript (types, the call table and
//! a class with one method per call) and a JSON schema of the same model,
//! with which tests check values crossing the boundary at run time.
//! `cargo xtask types --check`, and the test below, fail when the committed
//! files differ from what the Rust source gives.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Attribute, Expr, Fields, GenericArgument, Item, Lit, Meta, PathArguments, Token, Type};

/// What one generation reads and writes.
struct Target {
    /// Rust sources holding the definitions: files, or directories read
    /// recursively. Test files and `#[cfg(test)]` items are skipped.
    sources: &'static [&'static str],
    /// The file whose `calls! { … }` table lists the calls, if any.
    calls: Option<&'static str>,
    /// Types emitted whether or not a call names them.
    roots: &'static [&'static str],
    /// What serde attributes cannot say: a variant serialised `with` a
    /// function as a fixed string, named `Type::Variant`, and the string.
    overrides: &'static [(&'static str, &'static str)],
    /// The TypeScript and the schema, if one is wanted, relative to the
    /// workspace root.
    typescript: &'static str,
    schema: Option<&'static str>,
    /// What to change instead of the output, for its header.
    edit: &'static str,
}

const DESIGNER: Target = Target {
    sources: &[
        "crates/cavea-designer/src",
        "crates/cavea-edit/src",
        "crates/cavea-compiler/src/document.rs",
    ],
    calls: Some("crates/cavea-designer/src/calls.rs"),
    roots: &["Failure"],
    // `Outline::Auto` is written by `mod auto` as the string "auto".
    overrides: &[("Outline::Auto", "auto")],
    typescript: "packages/designer-core/src/generated/calls.ts",
    schema: Some("packages/designer-core/src/generated/schema.json"),
    edit: "change the Rust types or the call table in crates/cavea-designer/src/calls.rs",
};

/// The buyer SDK's side of the WASM (FND-007, D-71): the events and the
/// object the WASM writes as JSON, and the option names it takes. The
/// definitions are test-only mirrors in `cavea-web`, tied to the engine's
/// protocol and to the hand-written JSON by that crate's tests.
const RENDERER: Target = Target {
    sources: &["crates/cavea-web/src/schema.rs"],
    calls: None,
    roots: &[
        "EngineEvent",
        "ObjectDetails",
        "ThemeName",
        "GestureHandling",
    ],
    overrides: &[],
    typescript: "packages/renderer/src/generated/engine.ts",
    schema: None,
    edit: "change crates/cavea-web/src/schema.rs",
};

const TARGETS: &[Target] = &[DESIGNER, RENDERER];

/// Writes every target's files, or with `check` fails if one is stale.
pub fn write(check: bool) -> Result<()> {
    let root = super::root();
    let mut stale = Vec::new();
    let mut outputs = Vec::new();
    for target in TARGETS {
        let (typescript, schema) = generate(&root, target)?;
        outputs.push((target.typescript, typescript));
        if let Some(path) = target.schema {
            outputs.push((path, schema));
        }
    }
    // The console's API types, from the server's OpenAPI document (`api.rs`).
    outputs.push((crate::api::OUTPUT, crate::api::generate(&root)?));
    outputs.push((crate::api::SDK_OUTPUT, crate::api::generate_sdk(&root)?));
    for (path, text) in outputs {
        let file = root.join(path);
        let old = fs::read_to_string(&file).unwrap_or_default();
        if old == text {
            continue;
        }
        if check {
            stale.push(path);
        } else {
            if let Some(dir) = file.parent() {
                fs::create_dir_all(dir)?;
            }
            fs::write(&file, text).with_context(|| format!("cannot write {path}"))?;
            println!("wrote {path}");
        }
    }
    ensure!(
        stale.is_empty(),
        "generated types are stale, run `cargo xtask types`: {}",
        stale.join(", ")
    );
    if check {
        println!("generated types are current");
    }
    Ok(())
}

// ---------------------------------------------------------------- the model

/// A TypeScript type, as JSON sees it.
#[derive(Debug, Clone, PartialEq)]
enum Ty {
    Number,
    String,
    Boolean,
    Null,
    Literal(String),
    Array(Box<Ty>),
    Tuple(Vec<Ty>),
    /// A JSON object with string keys and values of one type.
    Record(Box<Ty>),
    Union(Vec<Ty>),
    Object(Vec<Prop>),
    /// Every part at once: a tag beside a struct's fields.
    All(Vec<Ty>),
    Ref(String, Vec<Ty>),
    Param(String),
}

#[derive(Debug, Clone, PartialEq)]
struct Prop {
    name: String,
    ty: Ty,
    optional: bool,
    doc: String,
}

#[derive(Debug)]
struct Decl {
    params: Vec<String>,
    ty: Ty,
    doc: String,
}

fn nullable(ty: Ty) -> Ty {
    match ty {
        Ty::Union(mut v) => {
            if !v.contains(&Ty::Null) {
                v.push(Ty::Null);
            }
            Ty::Union(v)
        }
        Ty::Null => Ty::Null,
        ty => Ty::Union(vec![ty, Ty::Null]),
    }
}

fn literal(s: &str) -> Ty {
    Ty::Literal(s.to_owned())
}

fn prop(name: &str, ty: Ty) -> Prop {
    Prop {
        name: name.to_owned(),
        ty,
        optional: false,
        doc: String::new(),
    }
}

// ------------------------------------------------------------ the sources

/// A definition found in the sources.
struct Def {
    item: Item,
    public: bool,
    file: PathBuf,
}

/// Every struct, enum and type alias in the sources, by name. Structs made
/// by a `macro_rules!` struct template in the same file are expanded.
fn scan(root: &Path, sources: &[&str]) -> Result<BTreeMap<String, Vec<Def>>> {
    let mut files = Vec::new();
    for source in sources {
        collect_files(&root.join(source), &mut files)?;
    }
    let mut defs: BTreeMap<String, Vec<Def>> = BTreeMap::new();
    for file in files {
        let text = fs::read_to_string(&file)?;
        let parsed =
            syn::parse_file(&text).with_context(|| format!("cannot parse {}", file.display()))?;
        let mut items = Vec::new();
        flatten_items(parsed.items, &mut items);
        let templates: BTreeMap<String, proc_macro2::TokenStream> = items
            .iter()
            .filter_map(|item| match item {
                Item::Macro(m) if m.mac.path.is_ident("macro_rules") => {
                    Some((m.ident.as_ref()?.to_string(), m.mac.tokens.clone()))
                }
                _ => None,
            })
            .collect();
        for item in items {
            let item = match item {
                Item::Macro(m) if m.ident.is_none() => {
                    let Some(name) = m.mac.path.get_ident().map(ToString::to_string) else {
                        continue;
                    };
                    let Some(template) = templates.get(&name) else {
                        continue;
                    };
                    match expand_template(template, &m.mac.tokens) {
                        Some(item) => item,
                        None => continue,
                    }
                }
                item => item,
            };
            let (name, public) = match &item {
                Item::Struct(s) => (s.ident.to_string(), is_pub(&s.vis)),
                Item::Enum(e) => (e.ident.to_string(), is_pub(&e.vis)),
                Item::Type(t) => (t.ident.to_string(), is_pub(&t.vis)),
                _ => continue,
            };
            defs.entry(name).or_default().push(Def {
                item,
                public,
                file: file.clone(),
            });
        }
    }
    Ok(defs)
}

fn collect_files(path: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if path.is_file() {
        out.push(path.to_path_buf());
        return Ok(());
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(path)
        .with_context(|| format!("cannot read {}", path.display()))?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    entries.sort();
    for entry in entries {
        let name = entry.file_name().unwrap_or_default().to_string_lossy();
        if entry.is_dir() {
            collect_files(&entry, out)?;
        } else if name.ends_with(".rs") && !name.ends_with("tests.rs") {
            out.push(entry);
        }
    }
    Ok(())
}

/// The file's items with inline modules opened, `#[cfg(test)]` ones left
/// out.
fn flatten_items(items: Vec<Item>, out: &mut Vec<Item>) {
    for item in items {
        let attrs: &[Attribute] = match &item {
            Item::Mod(m) => &m.attrs,
            Item::Struct(s) => &s.attrs,
            Item::Enum(e) => &e.attrs,
            _ => &[],
        };
        if attrs.iter().any(is_cfg_test) {
            continue;
        }
        match item {
            Item::Mod(m) => {
                if let Some((_, inner)) = m.content {
                    flatten_items(inner, out);
                }
            }
            item => out.push(item),
        }
    }
}

fn is_cfg_test(attr: &Attribute) -> bool {
    attr.path().is_ident("cfg")
        && matches!(&attr.meta, Meta::List(l) if l.tokens.to_string() == "test")
}

fn is_pub(vis: &syn::Visibility) -> bool {
    matches!(vis, syn::Visibility::Public(_))
}

/// Expands an invocation of a struct template: a `macro_rules!` with one
/// rule `($(#[$meta:meta])* $name:ident { $($body:tt)* }) => { … }` whose
/// expansion is one struct. `None` for any other macro.
fn expand_template(
    rules: &proc_macro2::TokenStream,
    call: &proc_macro2::TokenStream,
) -> Option<Item> {
    use proc_macro2::{Delimiter, TokenStream, TokenTree};
    let mut rules: Vec<TokenTree> = rules.clone().into_iter().collect();
    if matches!(rules.last(), Some(TokenTree::Punct(p)) if p.as_char() == ';') {
        rules.pop();
    }
    // `( matcher ) => { transcriber }`, one rule.
    let [
        TokenTree::Group(_),
        TokenTree::Punct(_),
        TokenTree::Punct(_),
        TokenTree::Group(body),
    ] = rules.as_slice()
    else {
        return None;
    };
    // The invocation: outer attributes, a name, a braced body.
    let call: Vec<TokenTree> = call.clone().into_iter().collect();
    let (name, fields) = match call.as_slice() {
        [.., TokenTree::Ident(name), TokenTree::Group(fields)]
            if fields.delimiter() == Delimiter::Brace =>
        {
            (name.clone(), fields.stream())
        }
        _ => return None,
    };
    let attrs: TokenStream = call[..call.len() - 2].iter().cloned().collect();

    fn substitute(
        stream: TokenStream,
        name: &proc_macro2::Ident,
        attrs: &TokenStream,
        fields: &TokenStream,
    ) -> TokenStream {
        let mut out = Vec::new();
        let mut tokens = stream.into_iter().peekable();
        while let Some(token) = tokens.next() {
            match token {
                TokenTree::Punct(p) if p.as_char() == '$' => match tokens.next() {
                    Some(TokenTree::Ident(i)) if i == "name" => {
                        out.push(TokenTree::Ident(name.clone()))
                    }
                    Some(TokenTree::Group(g)) => {
                        // `$( … )*`: the attributes or the body.
                        tokens.next();
                        let inner = g.stream().to_string();
                        if inner.contains("meta") {
                            out.extend(attrs.clone());
                        } else {
                            out.extend(fields.clone());
                        }
                    }
                    _ => {}
                },
                TokenTree::Group(g) => {
                    let mut group = proc_macro2::Group::new(
                        g.delimiter(),
                        substitute(g.stream(), name, attrs, fields),
                    );
                    group.set_span(g.span());
                    out.push(TokenTree::Group(group));
                }
                other => out.push(other),
            }
        }
        out.into_iter().collect()
    }

    let expanded = substitute(body.stream(), &name, &attrs, &fields);
    syn::parse2::<syn::ItemStruct>(expanded)
        .ok()
        .map(Item::Struct)
}

// --------------------------------------------------------------- the calls

struct Call {
    name: String,
    doc: String,
    args: Vec<(String, Type)>,
    ret: Type,
    error: Option<Type>,
}

struct CallTable(Vec<Call>);

impl Parse for CallTable {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut calls = Vec::new();
        while !input.is_empty() {
            let attrs = Attribute::parse_outer(input)?;
            input.parse::<Token![fn]>()?;
            let name: syn::Ident = input.parse()?;
            let content;
            syn::parenthesized!(content in input);
            content.parse::<syn::Ident>()?;
            let mut args = Vec::new();
            while !content.is_empty() {
                content.parse::<Token![,]>()?;
                let arg: syn::Ident = content.parse()?;
                content.parse::<Token![:]>()?;
                args.push((arg.to_string(), content.parse()?));
            }
            input.parse::<Token![->]>()?;
            let ret: Type = input.parse()?;
            let error = if input.peek(Token![|]) {
                input.parse::<Token![|]>()?;
                Some(input.parse()?)
            } else {
                None
            };
            input.parse::<syn::Block>()?;
            calls.push(Call {
                name: name.to_string(),
                doc: doc(&attrs),
                args,
                ret,
                error,
            });
        }
        Ok(Self(calls))
    }
}

fn read_calls(path: &Path) -> Result<Vec<Call>> {
    let text = fs::read_to_string(path)?;
    let file = syn::parse_file(&text)?;
    for item in file.items {
        if let Item::Macro(m) = item
            && m.ident.is_none()
            && m.mac.path.is_ident("calls")
        {
            return Ok(syn::parse2::<CallTable>(m.mac.tokens)?.0);
        }
    }
    bail!("no `calls! {{ … }}` table in {}", path.display())
}

// ---------------------------------------------------------- serde reading

fn doc(attrs: &[Attribute]) -> String {
    let lines: Vec<String> = attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            Meta::NameValue(nv) => match &nv.value {
                Expr::Lit(l) => match &l.lit {
                    Lit::Str(s) => Some(s.value()),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .map(|line| line.strip_prefix(' ').unwrap_or(&line).to_owned())
        .collect();
    lines.join("\n").trim().to_owned()
}

/// The `#[serde(…)]` settings of one item, variant or field: each key with
/// its string value, or an empty string for a bare word.
fn serde_attrs(
    attrs: &[Attribute],
    allowed: &[&str],
    what: &str,
) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for attr in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            let key = meta
                .path
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();
            let value = if meta.input.peek(Token![=]) {
                let lit: syn::LitStr = meta.value()?.parse()?;
                lit.value()
            } else {
                String::new()
            };
            out.insert(key, value);
            Ok(())
        })
        .map_err(|e| anyhow!("{what}: cannot read #[serde]: {e}"))?;
    }
    for key in out.keys() {
        ensure!(
            allowed.contains(&key.as_str()),
            "{what}: #[serde({key})] is not modelled by the type generator (xtask/src/types.rs)"
        );
    }
    Ok(out)
}

const CONTAINER: &[&str] = &[
    "rename_all",
    "rename_all_fields",
    "tag",
    "content",
    "untagged",
    "deny_unknown_fields",
    "default",
];
const VARIANT: &[&str] = &["rename", "rename_all", "skip", "with", "alias"];
const FIELD: &[&str] = &[
    "rename",
    "default",
    "skip",
    "skip_serializing_if",
    "deserialize_with",
    "alias",
];

/// Applies a `rename_all` rule to a field (`snake_case` in Rust) or a
/// variant (`PascalCase`), as serde does.
fn rename(rule: Option<&String>, name: &str, variant: bool, what: &str) -> Result<String> {
    let Some(rule) = rule else {
        return Ok(name.to_owned());
    };
    // Split into lower-case words.
    let words: Vec<String> = if variant {
        let mut words: Vec<String> = Vec::new();
        for c in name.chars() {
            if c.is_uppercase() || words.is_empty() {
                words.push(String::new());
            }
            if let Some(w) = words.last_mut() {
                w.extend(c.to_lowercase());
            }
        }
        words
    } else {
        name.split('_').map(str::to_owned).collect()
    };
    let cap = |w: &String| {
        let mut c = w.chars();
        c.next()
            .map(|f| f.to_uppercase().chain(c).collect::<String>())
            .unwrap_or_default()
    };
    Ok(match rule.as_str() {
        "lowercase" => words.concat(),
        "UPPERCASE" => words.concat().to_uppercase(),
        "camelCase" => words
            .iter()
            .enumerate()
            .map(|(i, w)| if i == 0 { w.clone() } else { cap(w) })
            .collect(),
        "PascalCase" => words.iter().map(cap).collect(),
        "snake_case" => words.join("_"),
        "kebab-case" => words.join("-"),
        "SCREAMING_SNAKE_CASE" => words.join("_").to_uppercase(),
        "SCREAMING-KEBAB-CASE" => words.join("-").to_uppercase(),
        other => bail!("{what}: unknown rename rule {other:?}"),
    })
}

// ----------------------------------------------------------- the builder

/// How a type crosses: as serde writes it, or as serde reads it.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Out,
    In,
}

struct Builder<'a> {
    defs: &'a BTreeMap<String, Vec<Def>>,
    overrides: &'a [(&'static str, &'static str)],
    decls: BTreeMap<String, Decl>,
    /// Names waiting to be declared.
    queue: Vec<String>,
    /// Newtype variants of internally tagged enums: (enum, variant,
    /// referenced struct), checked to be objects once all are declared.
    flattened: Vec<(String, String, Ty)>,
}

impl<'a> Builder<'a> {
    fn def(&self, name: &str) -> Result<&'a Def> {
        let found = self
            .defs
            .get(name)
            .ok_or_else(|| anyhow!("type {name} is not defined in the target's sources"))?;
        let public: Vec<&Def> = found.iter().filter(|d| d.public).collect();
        match (found.as_slice(), public.as_slice()) {
            ([one], _) => Ok(one),
            (_, [one]) => Ok(*one),
            _ => bail!(
                "type {name} is defined more than once: {}",
                found
                    .iter()
                    .map(|d| d.file.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// The model of a Rust type in `params`' scope.
    fn ty(&mut self, ty: &Type, params: &[String], what: &str) -> Result<Ty> {
        Ok(match ty {
            Type::Reference(r) => match &*r.elem {
                Type::Slice(s) => Ty::Array(Box::new(self.ty(&s.elem, params, what)?)),
                elem => self.ty(elem, params, what)?,
            },
            Type::Slice(s) => Ty::Array(Box::new(self.ty(&s.elem, params, what)?)),
            Type::Paren(p) => self.ty(&p.elem, params, what)?,
            Type::Group(g) => self.ty(&g.elem, params, what)?,
            Type::Tuple(t) if t.elems.is_empty() => Ty::Null,
            Type::Tuple(t) => Ty::Tuple(
                t.elems
                    .iter()
                    .map(|e| self.ty(e, params, what))
                    .collect::<Result<_>>()?,
            ),
            Type::Array(a) => {
                let Expr::Lit(syn::ExprLit {
                    lit: Lit::Int(n), ..
                }) = &a.len
                else {
                    bail!("{what}: an array length must be a number");
                };
                let n: usize = n.base10_parse()?;
                Ty::Tuple(vec![self.ty(&a.elem, params, what)?; n])
            }
            Type::Path(p) if p.qself.is_none() => {
                let last = p
                    .path
                    .segments
                    .last()
                    .ok_or_else(|| anyhow!("{what}: empty path"))?;
                let name = last.ident.to_string();
                let args: Vec<&Type> = match &last.arguments {
                    PathArguments::AngleBracketed(a) => a
                        .args
                        .iter()
                        .filter_map(|g| match g {
                            GenericArgument::Type(t) => Some(t),
                            _ => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                let arg = |i: usize, s: &mut Self| -> Result<Ty> {
                    let t = args
                        .get(i)
                        .ok_or_else(|| anyhow!("{what}: {name} needs a type argument"))?;
                    s.ty(t, params, what)
                };
                match name.as_str() {
                    "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64"
                    | "isize" | "f32" | "f64" => Ty::Number,
                    "bool" => Ty::Boolean,
                    "String" | "str" | "char" => Ty::String,
                    "Option" => nullable(arg(0, self)?),
                    "Vec" | "VecDeque" | "BTreeSet" | "HashSet" => {
                        Ty::Array(Box::new(arg(0, self)?))
                    }
                    "Box" | "Rc" | "Arc" => arg(0, self)?,
                    "BTreeMap" | "HashMap" => {
                        let key = arg(0, self)?;
                        ensure!(
                            matches!(key, Ty::String | Ty::Number),
                            "{what}: map keys must be strings or integers"
                        );
                        Ty::Record(Box::new(arg(1, self)?))
                    }
                    _ if params.contains(&name) => Ty::Param(name),
                    _ => {
                        let def = self.def(&name)?;
                        if let Item::Type(alias) = &def.item {
                            // Aliases are inlined; generic ones are not used.
                            ensure!(
                                alias.generics.params.is_empty(),
                                "{what}: generic alias {name}"
                            );
                            return self.ty(&alias.ty, &[], &name);
                        }
                        let args = (0..args.len())
                            .map(|i| arg(i, self))
                            .collect::<Result<Vec<_>>>()?;
                        if !self.decls.contains_key(&name) && !self.queue.contains(&name) {
                            self.queue.push(name.clone());
                        }
                        Ty::Ref(name, args)
                    }
                }
            }
            _ => bail!("{what}: this kind of type does not cross the boundary"),
        })
    }

    fn declare(&mut self, name: &str) -> Result<()> {
        let def = self.def(name)?;
        let (attrs, generics) = match &def.item {
            Item::Struct(s) => (&s.attrs, &s.generics),
            Item::Enum(e) => (&e.attrs, &e.generics),
            _ => bail!("{name} is not a struct or enum"),
        };
        let params: Vec<String> = generics
            .type_params()
            .map(|p| p.ident.to_string())
            .collect();
        let mode = mode(attrs, name)?;
        let container = serde_attrs(attrs, CONTAINER, name)?;
        let ty = match &def.item {
            Item::Struct(s) => self.fields(&s.fields, &container, &params, mode, name, None)?,
            Item::Enum(e) => {
                let mut variants = Vec::new();
                for v in &e.variants {
                    if let Some(ty) = self.variant(name, v, &container, &params, mode)? {
                        variants.push(ty);
                    }
                }
                Ty::Union(variants)
            }
            _ => bail!("{name} is not a struct or enum"),
        };
        self.decls.insert(
            name.to_owned(),
            Decl {
                params,
                ty,
                doc: doc(attrs),
            },
        );
        Ok(())
    }

    /// A struct's, or a variant's, fields as JSON.
    fn fields(
        &mut self,
        fields: &Fields,
        container: &BTreeMap<String, String>,
        params: &[String],
        mode: Mode,
        what: &str,
        rule: Option<&String>,
    ) -> Result<Ty> {
        let rule = rule.or(container.get("rename_all"));
        match fields {
            Fields::Unit => Ok(Ty::Null),
            Fields::Unnamed(f) => {
                let mut tys = Vec::new();
                for field in &f.unnamed {
                    serde_attrs(&field.attrs, &[], what)?;
                    tys.push(self.ty(&field.ty, params, what)?);
                }
                Ok(match <[Ty; 1]>::try_from(tys) {
                    Ok([one]) => one,
                    Err(tys) => Ty::Tuple(tys),
                })
            }
            Fields::Named(f) => {
                let mut props = Vec::new();
                for field in &f.named {
                    let ident = field
                        .ident
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default();
                    let at = format!("{what}.{ident}");
                    let serde = serde_attrs(&field.attrs, FIELD, &at)?;
                    if serde.contains_key("skip") {
                        continue;
                    }
                    let name = match serde.get("rename") {
                        Some(r) => r.clone(),
                        None => rename(rule, &ident, false, &at)?,
                    };
                    let full = self.ty(&field.ty, params, &at)?;
                    let is_option =
                        full != Ty::Null && matches!(&full, Ty::Union(v) if v.contains(&Ty::Null));
                    let (ty, optional) = match mode {
                        Mode::Out => match serde.get("skip_serializing_if").map(String::as_str) {
                            None => (full, false),
                            Some("Option::is_none") if is_option => (strip_null(full), true),
                            Some(_) => (full, true),
                        },
                        Mode::In => {
                            ensure!(
                                !serde.contains_key("deserialize_with"),
                                "{at}: deserialize_with in a type only read is not modelled"
                            );
                            let optional = serde.contains_key("default")
                                || container.contains_key("default")
                                || is_option;
                            (full, optional)
                        }
                    };
                    props.push(Prop {
                        name,
                        ty,
                        optional,
                        doc: doc(&field.attrs),
                    });
                }
                Ok(Ty::Object(props))
            }
        }
    }

    /// One variant as JSON, by the enum's tagging; `None` if skipped.
    fn variant(
        &mut self,
        enum_name: &str,
        v: &syn::Variant,
        container: &BTreeMap<String, String>,
        params: &[String],
        mode: Mode,
    ) -> Result<Option<Ty>> {
        let what = format!("{enum_name}::{}", v.ident);
        let serde = serde_attrs(&v.attrs, VARIANT, &what)?;
        if serde.contains_key("skip") {
            return Ok(None);
        }
        let name = match serde.get("rename") {
            Some(r) => r.clone(),
            None => rename(
                container.get("rename_all"),
                &v.ident.to_string(),
                true,
                &what,
            )?,
        };
        let rule = serde
            .get("rename_all")
            .or(container.get("rename_all_fields"));
        // The fields alone: no container rename applies to a variant's fields.
        let no_rename = BTreeMap::new();
        let content = if serde.contains_key("with") {
            match self.overrides.iter().find(|(k, _)| *k == what) {
                Some((_, s)) => literal(s),
                None => bail!("{what}: #[serde(with)] needs an override in xtask/src/types.rs"),
            }
        } else {
            self.fields(&v.fields, &no_rename, params, mode, &what, rule)?
        };
        let doc_text = doc(&v.attrs);
        let tag = container.get("tag");
        let untagged = container.contains_key("untagged");
        let unit = matches!(v.fields, Fields::Unit) && !serde.contains_key("with");
        let ty = match (tag, container.get("content"), untagged) {
            (_, _, true) => content,
            (Some(tag), Some(key), false) => {
                let mut props = vec![prop(tag, literal(&name))];
                if !unit {
                    props.push(prop(key, content));
                }
                Ty::Object(props)
            }
            (Some(tag), None, false) => match (&v.fields, content) {
                (Fields::Unit, _) => Ty::Object(vec![prop(tag, literal(&name))]),
                (Fields::Named(_), Ty::Object(mut props)) => {
                    props.insert(0, prop(tag, literal(&name)));
                    Ty::Object(props)
                }
                (Fields::Unnamed(f), inner) if f.unnamed.len() == 1 => {
                    self.flattened
                        .push((enum_name.to_owned(), name.clone(), inner.clone()));
                    Ty::All(vec![Ty::Object(vec![prop(tag, literal(&name))]), inner])
                }
                _ => bail!("{what}: serde cannot tag a tuple variant internally"),
            },
            (None, _, false) if unit => literal(&name),
            (None, _, false) => Ty::Object(vec![prop(&name, content)]),
        };
        // Carry the variant's doc on its tag property when it has one.
        Ok(Some(match ty {
            Ty::Object(mut props) if !doc_text.is_empty() => {
                if let Some(first) = props.first_mut()
                    && first.doc.is_empty()
                {
                    first.doc = doc_text;
                }
                Ty::Object(props)
            }
            Ty::All(mut parts) if !doc_text.is_empty() => {
                if let Some(Ty::Object(props)) = parts.first_mut()
                    && let Some(first) = props.first_mut()
                {
                    first.doc = doc_text;
                }
                Ty::All(parts)
            }
            ty => ty,
        }))
    }
}

fn strip_null(ty: Ty) -> Ty {
    match ty {
        Ty::Union(v) => {
            let mut v: Vec<Ty> = v.into_iter().filter(|t| *t != Ty::Null).collect();
            if v.len() == 1 {
                v.remove(0)
            } else {
                Ty::Union(v)
            }
        }
        ty => ty,
    }
}

/// `Out` for a type that derives `Serialize`, else `In` for one that
/// derives `Deserialize`.
fn mode(attrs: &[Attribute], name: &str) -> Result<Mode> {
    let mut derives = BTreeSet::new();
    for attr in attrs.iter().filter(|a| a.path().is_ident("derive")) {
        let paths = attr
            .parse_args_with(Punctuated::<syn::Path, Token![,]>::parse_terminated)
            .map_err(|e| anyhow!("{name}: cannot read #[derive]: {e}"))?;
        for path in paths {
            if let Some(last) = path.segments.last() {
                derives.insert(last.ident.to_string());
            }
        }
    }
    if derives.contains("Serialize") {
        Ok(Mode::Out)
    } else if derives.contains("Deserialize") {
        Ok(Mode::In)
    } else {
        bail!("{name} crosses the boundary but derives neither Serialize nor Deserialize")
    }
}

// ------------------------------------------------------------- generation

/// The TypeScript and the schema for `target`.
fn generate(root: &Path, target: &Target) -> Result<(String, String)> {
    let defs = scan(root, target.sources)?;
    let calls = match target.calls {
        Some(path) => read_calls(&root.join(path))?,
        None => Vec::new(),
    };
    let mut builder = Builder {
        defs: &defs,
        overrides: target.overrides,
        decls: BTreeMap::new(),
        queue: target.roots.iter().map(|r| (*r).to_owned()).collect(),
        flattened: Vec::new(),
    };
    let mut table = Vec::new();
    for call in &calls {
        let what = format!("call {}", call.name);
        let args = call
            .args
            .iter()
            .map(|(name, ty)| Ok((name.clone(), builder.ty(ty, &[], &what)?)))
            .collect::<Result<Vec<_>>>()?;
        let ret = builder.ty(&call.ret, &[], &what)?;
        let error = match &call.error {
            Some(e) => Some(builder.ty(e, &[], &what)?),
            None => None,
        };
        table.push((call, args, ret, error));
    }
    while let Some(name) = builder.queue.pop() {
        if !builder.decls.contains_key(&name) {
            builder.declare(&name)?;
        }
    }
    for (enum_name, variant, inner) in &builder.flattened {
        let is_object = match inner {
            Ty::Ref(name, _) => matches!(
                builder.decls.get(name),
                Some(Decl {
                    ty: Ty::Object(_),
                    ..
                })
            ),
            Ty::Object(_) => true,
            _ => false,
        };
        ensure!(
            is_object,
            "{enum_name}::{variant}: serde can tag a newtype variant internally only around a struct"
        );
    }
    let decls = builder.decls;

    // The TypeScript.
    let mut ts = String::new();
    let _ = write!(
        ts,
        "// Generated by `cargo xtask types` from the Rust definitions (D-41). Do not edit:\n\
         // {}.\n\n",
        target.edit
    );
    for (name, decl) in &decls {
        push_doc(&mut ts, &decl.doc, "");
        let params = if decl.params.is_empty() {
            String::new()
        } else {
            format!("<{}>", decl.params.join(", "))
        };
        let body = block(&decl.ty, "", true);
        let sep = if body.starts_with('\n') { "" } else { " " };
        let _ = writeln!(ts, "export type {name}{params} ={sep}{body};\n");
    }
    if !table.is_empty() {
        ts.push_str(
            "/** Every call: its arguments, its result and the error that refuses it. */\n",
        );
        ts.push_str("export interface CallTable {\n");
        for (call, args, ret, error) in &table {
            let args: Vec<String> = args
                .iter()
                .map(|(n, t)| format!("{}: {}", camel(n), inline(t)))
                .collect();
            let error = error.as_ref().map_or("never".to_owned(), inline);
            let _ = writeln!(
                ts,
                "  {}: {{ args: [{}]; returns: {}; error: {error} }};",
                call.name,
                args.join(", "),
                inline(ret)
            );
        }
        ts.push_str("}\n\n");
        ts.push_str(
            "/** One method per call of the table. A refused call throws; see `Session`. */\n\
             export abstract class Calls {\n  \
             protected abstract call<M extends keyof CallTable>(\n    \
             method: M,\n    \
             args: CallTable[M][\"args\"],\n  \
             ): CallTable[M][\"returns\"];\n",
        );
        for (call, args, ret, _) in &table {
            ts.push('\n');
            push_doc(&mut ts, &call.doc, "  ");
            let params: Vec<String> = args
                .iter()
                .map(|(n, t)| format!("{}: {}", camel(n), inline(t)))
                .collect();
            let names: Vec<String> = args.iter().map(|(n, _)| camel(n)).collect();
            let invoke = format!("this.call(\"{}\", [{}])", call.name, names.join(", "));
            if *ret == Ty::Null {
                let _ = writeln!(
                    ts,
                    "  {}({}): void {{\n    {invoke};\n  }}",
                    camel(&call.name),
                    params.join(", ")
                );
            } else {
                let _ = writeln!(
                    ts,
                    "  {}({}): {} {{\n    return {invoke};\n  }}",
                    camel(&call.name),
                    params.join(", "),
                    inline(ret)
                );
            }
        }
        ts.push_str("}\n");
    }

    // The schema.
    let types: serde_json::Map<String, Value> = decls
        .iter()
        .map(|(name, d)| {
            (
                name.clone(),
                json!({"params": d.params, "type": schema(&d.ty)}),
            )
        })
        .collect();
    let methods: serde_json::Map<String, Value> = table
        .iter()
        .map(|(call, _, ret, _)| {
            (
                camel(&call.name),
                json!({"call": call.name, "returns": schema(ret)}),
            )
        })
        .collect();
    let schema = serde_json::to_string_pretty(&json!({"types": types, "methods": methods}))? + "\n";
    Ok((ts, schema))
}

fn camel(snake: &str) -> String {
    rename(Some(&"camelCase".to_owned()), snake, false, snake).unwrap_or_else(|_| snake.to_owned())
}

fn push_doc(out: &mut String, doc: &str, indent: &str) {
    if doc.is_empty() {
        return;
    }
    let doc = doc.replace("*/", "*\\/");
    if !doc.contains('\n') {
        let _ = writeln!(out, "{indent}/** {doc} */");
        return;
    }
    let _ = writeln!(out, "{indent}/**");
    for line in doc.lines() {
        let _ = writeln!(
            out,
            "{indent} *{}{line}",
            if line.is_empty() { "" } else { " " }
        );
    }
    let _ = writeln!(out, "{indent} */");
}

fn is_ident(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// The type on one line, without docs.
fn inline(ty: &Ty) -> String {
    let list = |ts: &[Ty], sep: &str| ts.iter().map(inline).collect::<Vec<_>>().join(sep);
    match ty {
        Ty::Number => "number".into(),
        Ty::String => "string".into(),
        Ty::Boolean => "boolean".into(),
        Ty::Null => "null".into(),
        Ty::Literal(s) => serde_json::to_string(s).unwrap_or_default(),
        Ty::Array(t) if matches!(**t, Ty::Union(_) | Ty::All(_)) => format!("({})[]", inline(t)),
        Ty::Array(t) => format!("{}[]", inline(t)),
        Ty::Tuple(ts) => format!("[{}]", list(ts, ", ")),
        Ty::Record(t) => format!("Record<string, {}>", inline(t)),
        Ty::Union(ts) if ts.is_empty() => "never".into(),
        Ty::Union(ts) => list(ts, " | "),
        Ty::All(ts) => ts
            .iter()
            .map(|t| match t {
                Ty::Union(_) => format!("({})", inline(t)),
                t => inline(t),
            })
            .collect::<Vec<_>>()
            .join(" & "),
        Ty::Object(props) if props.is_empty() => "Record<string, never>".into(),
        Ty::Object(props) => format!(
            "{{ {} }}",
            props
                .iter()
                .map(|p| field(p, &inline(&p.ty)))
                .collect::<Vec<_>>()
                .join("; ")
        ),
        Ty::Ref(name, args) if args.is_empty() => name.clone(),
        Ty::Ref(name, args) => format!("{name}<{}>", list(args, ", ")),
        Ty::Param(p) => p.clone(),
    }
}

fn field(p: &Prop, ty: &str) -> String {
    let name = if is_ident(&p.name) {
        p.name.clone()
    } else {
        serde_json::to_string(&p.name).unwrap_or_default()
    };
    format!("{name}{}: {ty}", if p.optional { "?" } else { "" })
}

/// Whether a type carries docs that only the multi-line form shows.
fn documented(ty: &Ty) -> bool {
    match ty {
        Ty::Object(props) => props.iter().any(|p| !p.doc.is_empty() || documented(&p.ty)),
        Ty::Union(ts) | Ty::All(ts) => ts.iter().any(documented),
        _ => false,
    }
}

/// The type laid out over lines below `pad` where it is long or documented,
/// as a formatter would: objects one field a line, unions one member a line.
/// `open` lays an object out over lines even when it is short.
fn block(ty: &Ty, pad: &str, open: bool) -> String {
    let flat = inline(ty);
    let fits = !documented(ty) && pad.len() + flat.len() <= 80;
    match ty {
        Ty::Object(props) if !props.is_empty() && (open || !fits) => {
            let mut out = String::from("{\n");
            let inner = format!("{pad}  ");
            for p in props {
                push_doc(&mut out, &p.doc, &inner);
                let _ = writeln!(out, "{inner}{};", field(p, &block(&p.ty, &inner, false)));
            }
            out + pad + "}"
        }
        Ty::Union(ts) if ts.len() > 1 && !fits => {
            let member = format!("{pad}    ");
            ts.iter()
                .map(|t| format!("\n{pad}  | {}", block(t, &member, true)))
                .collect()
        }
        Ty::All(ts) if !fits => ts
            .iter()
            .map(|t| block(t, pad, false))
            .collect::<Vec<_>>()
            .join(" & "),
        _ => flat,
    }
}

fn schema(ty: &Ty) -> Value {
    match ty {
        Ty::Number => json!("number"),
        Ty::String => json!("string"),
        Ty::Boolean => json!("boolean"),
        Ty::Null => json!("null"),
        Ty::Literal(s) => json!({ "literal": s }),
        Ty::Array(t) => json!({ "array": schema(t) }),
        Ty::Tuple(ts) => json!({ "tuple": ts.iter().map(schema).collect::<Vec<_>>() }),
        Ty::Record(t) => json!({ "record": schema(t) }),
        Ty::Union(ts) => json!({ "union": ts.iter().map(schema).collect::<Vec<_>>() }),
        Ty::All(ts) => json!({ "all": ts.iter().map(schema).collect::<Vec<_>>() }),
        Ty::Object(props) => json!({
            "object": props
                .iter()
                .map(|p| json!({ "name": p.name, "type": schema(&p.ty), "optional": p.optional }))
                .collect::<Vec<_>>()
        }),
        Ty::Ref(name, args) => {
            json!({ "ref": name, "args": args.iter().map(schema).collect::<Vec<_>>() })
        }
        Ty::Param(p) => json!({ "param": p }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed TypeScript and schema are what the Rust source gives:
    /// a change to a type or a call that crosses the boundary must come with
    /// `cargo xtask types`.
    #[test]
    fn generated_types_are_current() {
        let root = crate::root();
        for target in TARGETS {
            let (typescript, schema) = generate(&root, target).unwrap();
            let schema = target.schema.map(|path| (path, schema));
            for (path, text) in std::iter::once((target.typescript, typescript)).chain(schema) {
                let committed = fs::read_to_string(root.join(path)).unwrap_or_default();
                assert!(
                    committed == text,
                    "{path} is stale: run `cargo xtask types`"
                );
            }
        }
    }

    fn model(source: &str, name: &str) -> Result<Ty> {
        let file = syn::parse_file(source)?;
        let defs = file
            .items
            .into_iter()
            .filter_map(|item| {
                let name = match &item {
                    Item::Struct(s) => s.ident.to_string(),
                    Item::Enum(e) => e.ident.to_string(),
                    _ => return None,
                };
                Some((
                    name,
                    vec![Def {
                        item,
                        public: true,
                        file: PathBuf::new(),
                    }],
                ))
            })
            .collect();
        let mut b = Builder {
            defs: &defs,
            overrides: &[],
            decls: BTreeMap::new(),
            queue: Vec::new(),
            flattened: Vec::new(),
        };
        b.declare(name)?;
        Ok(b.decls.remove(name).map(|d| d.ty).unwrap_or(Ty::Null))
    }

    /// The serde rules the generator models, against what serde_json
    /// writes for the same attributes (checked by hand, and at run time by
    /// the package's boundary test for every real type).
    #[test]
    fn models_serde_attributes() {
        let ts = |src: &str, name: &str| inline(&model(src, name).unwrap());
        // Optional fields: absent when skipped, `null` only when written.
        assert_eq!(
            ts(
                "#[derive(Serialize)] #[serde(rename_all = \"camelCase\")]
                 struct A { #[serde(skip_serializing_if = \"Option::is_none\")] focal_point: Option<u32>,
                            #[serde(skip_serializing_if = \"Vec::is_empty\")] list: Vec<u64>,
                            plain: Option<String>, #[serde(skip)] gone: u8 }",
                "A"
            ),
            "{ focalPoint?: number; list?: number[]; plain: string | null }"
        );
        // Read only: defaults and options may be left out.
        assert_eq!(
            ts(
                "#[derive(Deserialize)] struct B { #[serde(default)] a: u32, b: Option<u32>, c: bool }",
                "B"
            ),
            "{ a?: number; b?: number | null; c: boolean }"
        );
        // The four enum representations; variant fields are not renamed by
        // the container's `rename_all`.
        let e = "enum E { Unit, New(u32), Pair(u32, String), Named { seat_pitch: f64 } }";
        assert_eq!(
            ts(
                &format!("#[derive(Serialize)] #[serde(rename_all = \"camelCase\")] {e}"),
                "E"
            ),
            "\"unit\" | { new: number } | { pair: [number, string] } | { named: { seat_pitch: number } }"
        );
        assert_eq!(
            ts(
                &format!(
                    "#[derive(Serialize)] #[serde(tag = \"k\", content = \"d\", rename_all = \"camelCase\", rename_all_fields = \"camelCase\")] {e}"
                ),
                "E"
            ),
            "{ k: \"unit\" } | { k: \"new\"; d: number } | { k: \"pair\"; d: [number, string] } | { k: \"named\"; d: { seatPitch: number } }"
        );
        assert_eq!(
            ts(
                "#[derive(Serialize)] #[serde(untagged)] enum U { N(u32), S(String) }",
                "U"
            ),
            "number | string"
        );
        // Internal tagging cannot hold a tuple or a primitive.
        assert!(
            model(
                "#[derive(Serialize)] #[serde(tag = \"k\")] enum T { P(u32, u32) }",
                "T"
            )
            .is_err()
        );
        // Anything not modelled stops the generation.
        assert!(
            model(
                "#[derive(Serialize)] struct F { #[serde(flatten)] a: u32 }",
                "F"
            )
            .is_err()
        );
        assert!(
            model(
                "#[derive(Serialize)] #[serde(transparent)] struct G(u32);",
                "G"
            )
            .is_err()
        );
        assert!(model("struct H { a: u32 }", "H").is_err());
    }
}
