use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

pub mod guides;
mod html;
mod item;
mod parse;

pub use item::{attrs_of, context_of, fn_context, is_document, members_of, tidy};
pub use parse::{parse_attribute, parse_document, parse_lua_source, parse_rust_source};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Library,
    Class,
    Hook,
    Module,
    Struct,
    Enum,
    Trait,
    Type,
    Variant,
    Constant,
    Function,
    Method,
    Field,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Library => "library",
            Kind::Class => "class",
            Kind::Hook => "hook",
            Kind::Module => "module",
            Kind::Struct => "struct",
            Kind::Enum => "enum",
            Kind::Trait => "trait",
            Kind::Type => "type",
            Kind::Variant => "variant",
            Kind::Constant => "constant",
            Kind::Function => "function",
            Kind::Method => "method",
            Kind::Field => "field",
        }
    }

    /// Lua pages are libraries, classes and hooks; Rust pages are modules, types, variants and
    /// constants; WebAssembly pages are libraries, the modules a contract imports from.
    /// Functions, methods and fields belong to all.
    pub fn fits(self, lang: Lang) -> bool {
        match self {
            Kind::Function | Kind::Method | Kind::Field => true,
            Kind::Library => lang != Lang::Rust,
            Kind::Class | Kind::Hook => lang == Lang::Lua,
            Kind::Module
            | Kind::Struct
            | Kind::Enum
            | Kind::Trait
            | Kind::Type
            | Kind::Variant
            | Kind::Constant => lang == Lang::Rust,
        }
    }

    /// Kinds that own members. They always sit at the top of their reference.
    pub fn is_container(self) -> bool {
        matches!(
            self,
            Kind::Library | Kind::Class | Kind::Module | Kind::Struct | Kind::Enum | Kind::Trait
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Realm {
    Client,
    Server,
    Shared,
    Menu,
    #[serde(rename = "client,menu")]
    ClientMenu,
    #[serde(rename = "server,menu")]
    ServerMenu,
    #[serde(rename = "shared,menu")]
    All,
}

impl Realm {
    pub fn flags(self) -> (bool, bool, bool) {
        match self {
            Realm::Client => (true, false, false),
            Realm::Server => (false, true, false),
            Realm::Shared => (true, true, false),
            Realm::Menu => (false, false, true),
            Realm::ClientMenu => (true, false, true),
            Realm::ServerMenu => (false, true, true),
            Realm::All => (true, true, true),
        }
    }

    pub fn from_flags(client: bool, server: bool, menu: bool) -> Option<Realm> {
        match (client, server, menu) {
            (true, false, false) => Some(Realm::Client),
            (false, true, false) => Some(Realm::Server),
            (true, true, false) => Some(Realm::Shared),
            (false, false, true) => Some(Realm::Menu),
            (true, false, true) => Some(Realm::ClientMenu),
            (false, true, true) => Some(Realm::ServerMenu),
            (true, true, true) => Some(Realm::All),
            (false, false, false) => None,
        }
    }
}

/// Which reference a page belongs to. They are kept apart: each has its own ids, navigation,
/// search and signature style. Wasm is the host interface WebAssembly contracts import.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    Rust,
    Lua,
    Wasm,
}

impl Lang {
    pub fn name(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::Lua => "lua",
            Lang::Wasm => "wasm",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    Public,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Param {
    pub name: String,
    pub ty: String,
    pub desc: String,
    pub optional: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Ret {
    pub ty: String,
    pub desc: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Page {
    pub id: String,
    pub lang: Lang,
    pub parent: String,
    pub name: String,
    pub kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub realm: Option<Realm>,
    pub summary: String,
    pub syntax: String,
    pub params: Vec<Param>,
    pub returns: Vec<Ret>,
    pub example: Option<String>,
    pub note: Option<String>,
    pub panics: Option<String>,
    pub safety: Option<String>,
    pub since: Option<String>,
    pub see_also: Vec<String>,
    pub deprecated: Option<String>,
    pub deprecated_since: Option<String>,
    pub unimplemented: Option<String>,
    pub access: Access,
    #[serde(skip)]
    pub origin: String,
}

impl Page {
    /// Unique across both references: `rust/Tx::hash`, `lua/ents.create`.
    pub fn key(&self) -> String {
        key(self.lang, &self.id)
    }
}

fn key(lang: Lang, id: &str) -> String {
    format!("{}/{id}", lang.name())
}

#[derive(Clone, Debug, Default)]
pub struct RawDoc {
    pub parent: Option<String>,
    pub name: Option<String>,
    pub kind: Option<Kind>,
    pub lang: Option<Lang>,
    pub category: Option<String>,
    pub realm: Option<Realm>,
    pub summary: Option<String>,
    pub params: Vec<Param>,
    pub returns: Vec<Ret>,
    pub example: Option<String>,
    pub note: Option<String>,
    pub panics: Option<String>,
    pub safety: Option<String>,
    pub since: Option<String>,
    pub see_also: Vec<String>,
    pub deprecated: Option<String>,
    pub deprecated_since: Option<String>,
    pub unimplemented: Option<Option<String>>,
    pub access: Option<Access>,
}

/// What the documented item says about itself. The attribute only has to spell out what this
/// leaves open, and whatever it does spell out wins.
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub lang: Lang,
    pub name: Option<String>,
    pub parent: Option<String>,
    pub shape: Option<Shape>,
}

impl Context {
    pub fn lang(lang: Lang) -> Context {
        Context {
            lang,
            ..Context::default()
        }
    }
}

/// The Rust item under the attribute. Pages take their default kind from it, and Rust pages
/// also take their parameter and return types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shape {
    Function { params: Vec<Param>, returns: Vec<Ret> },
    Module,
    Struct,
    Enum,
    Trait,
    Alias { ty: String },
    Constant { ty: String },
    Variant { params: Vec<Param> },
    Field { ty: String },
}

impl Shape {
    fn kind(&self, lang: Lang, parent: &str) -> Kind {
        match (self, lang) {
            (Shape::Function { .. }, _) => default_kind(parent),
            (Shape::Module, Lang::Rust) => Kind::Module,
            (Shape::Module, Lang::Lua | Lang::Wasm) => Kind::Library,
            (Shape::Struct, Lang::Rust) => Kind::Struct,
            (Shape::Enum, Lang::Rust) => Kind::Enum,
            (Shape::Trait, Lang::Rust) => Kind::Trait,
            (Shape::Alias { .. }, Lang::Rust) => Kind::Type,
            (Shape::Struct | Shape::Enum | Shape::Trait | Shape::Alias { .. }, Lang::Lua) => {
                Kind::Class
            }
            (Shape::Struct | Shape::Enum | Shape::Trait | Shape::Alias { .. }, Lang::Wasm) => {
                Kind::Library
            }
            (Shape::Constant { .. }, Lang::Rust) => Kind::Constant,
            (Shape::Variant { .. }, Lang::Rust) => Kind::Variant,
            (Shape::Constant { .. } | Shape::Variant { .. } | Shape::Field { .. }, _) => {
                Kind::Field
            }
        }
    }

    fn fill(
        &self,
        kind: Kind,
        params: Vec<Param>,
        returns: Vec<Ret>,
    ) -> Result<(Vec<Param>, Vec<Ret>), String> {
        match (self, kind) {
            (
                Shape::Function {
                    params: found,
                    returns: found_returns,
                },
                Kind::Function | Kind::Method,
            ) => Ok((merge_params(params, found)?, merge_returns(returns, found_returns))),
            (Shape::Variant { params: found }, Kind::Variant) => {
                Ok((merge_params(params, found)?, returns))
            }
            (
                Shape::Alias { ty } | Shape::Constant { ty } | Shape::Field { ty },
                Kind::Type | Kind::Constant | Kind::Field,
            ) => {
                let found = [Ret {
                    ty: ty.clone(),
                    desc: String::new(),
                }];

                Ok((params, merge_returns(returns, &found)))
            }
            _ => Ok((params, returns)),
        }
    }
}

fn default_kind(parent: &str) -> Kind {
    if parent.chars().next().is_some_and(|ch| ch.is_uppercase()) {
        Kind::Method
    } else {
        Kind::Function
    }
}

/// The signature decides which parameters exist and in what order. The attribute adds the
/// descriptions and may override a type, but naming a parameter the signature lacks is an error.
fn merge_params(documented: Vec<Param>, found: &[Param]) -> Result<Vec<Param>, String> {
    for param in &documented {
        if !found.iter().any(|item| item.name == param.name) {
            return Err(format!("param {} is not in the signature", param.name));
        }
    }

    let mut merged = Vec::new();

    for item in found {
        let Some(param) = documented.iter().find(|param| param.name == item.name) else {
            merged.push(item.clone());

            continue;
        };
        let ty = if param.ty.is_empty() {
            item.ty.clone()
        } else {
            param.ty.clone()
        };

        merged.push(Param {
            name: param.name.clone(),
            ty,
            desc: param.desc.clone(),
            optional: param.optional,
        });
    }

    Ok(merged)
}

fn merge_returns(mut documented: Vec<Ret>, found: &[Ret]) -> Vec<Ret> {
    if documented.is_empty() {
        return found.to_vec();
    }

    if let ([only], [item]) = (documented.as_mut_slice(), found)
        && only.ty.is_empty()
    {
        only.ty = item.ty.clone();
    }

    documented
}

pub fn interpret(raw: RawDoc, context: &Context) -> Result<Page, String> {
    let summary = match raw.summary {
        Some(summary) if !summary.is_empty() => summary,
        _ => {
            return Err("missing summary".to_string());
        }
    };
    // A page named after something other than the item it sits on describes that other thing,
    // so the item's signature says nothing about it.
    let own = raw
        .name
        .as_ref()
        .is_none_or(|name| name.is_empty() || context.name.as_ref() == Some(name));
    let name = match raw.name {
        Some(name) if !name.is_empty() => name,
        _ => match &context.name {
            Some(name) if !name.is_empty() => name.clone(),
            _ => {
                return Err("missing name".to_string());
            }
        },
    };
    let lang = raw.lang.unwrap_or(context.lang);
    let mut parent = raw
        .parent
        .or_else(|| context.parent.clone())
        .unwrap_or_default();
    let kind = match (raw.kind, &context.shape) {
        (Some(kind), _) => kind,
        (None, Some(shape)) => shape.kind(lang, &parent),
        (None, None) => default_kind(&parent),
    };

    if !kind.fits(lang) {
        return Err(format!(
            "kind \"{}\" is not used for {} pages",
            kind.name(),
            lang.name()
        ));
    }

    if kind.is_container() {
        parent.clear();
    } else if parent.is_empty() && !matches!(kind, Kind::Function | Kind::Constant | Kind::Type) {
        return Err("missing parent".to_string());
    }

    let (params, returns) = match &context.shape {
        Some(shape) if own && lang == Lang::Rust => shape.fill(kind, raw.params, raw.returns)?,
        _ => (raw.params, raw.returns),
    };
    let realm = raw.realm;
    let access = raw.access.unwrap_or(Access::Public);
    let category = nonempty(raw.category);
    let example = nonempty(raw.example);
    let note = nonempty(raw.note);
    let panics = nonempty(raw.panics);
    let safety = nonempty(raw.safety);
    let since = nonempty(raw.since);
    let deprecated = nonempty(raw.deprecated);
    let deprecated_since = nonempty(raw.deprecated_since);
    let unimplemented = match raw.unimplemented {
        None => None,
        Some(None) => Some("Not implemented.".to_string()),
        Some(Some(text)) if text.is_empty() => Some("Not implemented.".to_string()),
        Some(Some(text)) => Some(text),
    };
    let syntax = syntax(lang, kind, &parent, &name, &params, &returns);
    let id = page_id(lang, kind, &parent, &name);

    Ok(Page {
        id,
        lang,
        parent,
        name,
        kind,
        category,
        realm,
        summary,
        syntax,
        params,
        returns,
        example,
        note,
        panics,
        safety,
        since,
        see_also: raw.see_also,
        deprecated,
        deprecated_since,
        unimplemented,
        access,
        origin: String::new(),
    })
}

fn nonempty(value: Option<String>) -> Option<String> {
    match value {
        Some(text) if !text.is_empty() => Some(text),
        _ => None,
    }
}

fn page_id(lang: Lang, kind: Kind, parent: &str, name: &str) -> String {
    if kind.is_container() || parent.is_empty() {
        return name.to_string();
    }

    format!("{parent}{}{name}", separator(lang, kind))
}

/// How a member joins its parent: `Entity:spawn` and `ents.create` in Lua, `Tx::hash` in Rust,
/// `chain.storage_get` for an import, and `Tx.from` for a field in any.
fn separator(lang: Lang, kind: Kind) -> &'static str {
    match (lang, kind) {
        (_, Kind::Field) => ".",
        (Lang::Lua, Kind::Method | Kind::Hook) => ":",
        (Lang::Lua | Lang::Wasm, _) => ".",
        (Lang::Rust, _) => "::",
    }
}

fn syntax(
    lang: Lang,
    kind: Kind,
    parent: &str,
    name: &str,
    params: &[Param],
    returns: &[Ret],
) -> String {
    let path = page_id(lang, kind, parent, name);

    match lang {
        Lang::Lua => lua_syntax(kind, &path, params, returns),
        Lang::Rust => rust_syntax(kind, &path, params, returns),
        Lang::Wasm => wasm_syntax(kind, &path, params, returns),
    }
}

/// Imports read like the functions they are: `chain.storage_get(key: i32, key_len: i32) -> i32`.
fn wasm_syntax(kind: Kind, path: &str, params: &[Param], returns: &[Ret]) -> String {
    match kind {
        Kind::Library => format!("module {path}"),
        Kind::Field => match first_type(returns) {
            Some(ty) => format!("{path}: {ty}"),
            None => path.to_string(),
        },
        _ => format!("{path}({}){}", rust_args(params), rust_returns(returns)),
    }
}

fn lua_syntax(kind: Kind, path: &str, params: &[Param], returns: &[Ret]) -> String {
    match kind {
        Kind::Field => match first_type(returns) {
            Some(ty) => format!("{ty} {path}"),
            None => path.to_string(),
        },
        Kind::Library => path.to_string(),
        Kind::Class => {
            if params.is_empty() {
                return path.to_string();
            }

            format!("{path}({})", lua_args(params))
        }
        _ => format!("{path}({})", lua_args(params)),
    }
}

fn rust_syntax(kind: Kind, path: &str, params: &[Param], returns: &[Ret]) -> String {
    let ty = first_type(returns);

    match kind {
        Kind::Module => format!("mod {path}"),
        Kind::Struct | Kind::Enum | Kind::Trait => format!("{} {path}", kind.name()),
        Kind::Type => match ty {
            Some(ty) => format!("type {path} = {ty}"),
            None => format!("type {path}"),
        },
        Kind::Constant => match ty {
            Some(ty) => format!("const {path}: {ty}"),
            None => format!("const {path}"),
        },
        Kind::Field => match ty {
            Some(ty) => format!("{path}: {ty}"),
            None => path.to_string(),
        },
        Kind::Variant => {
            if params.is_empty() {
                return path.to_string();
            }

            if is_tuple(params) {
                let types: Vec<&str> = params
                    .iter()
                    .map(|param| if param.ty.is_empty() { "_" } else { param.ty.as_str() })
                    .collect();

                return format!("{path}({})", types.join(", "));
            }

            format!("{path} {{ {} }}", rust_args(params))
        }
        _ => format!("fn {path}({}){}", rust_args(params), rust_returns(returns)),
    }
}

fn first_type(returns: &[Ret]) -> Option<&str> {
    returns
        .first()
        .map(|item| item.ty.as_str())
        .filter(|ty| !ty.is_empty())
}

/// Tuple variants document their fields as `0`, `1`, and so on.
fn is_tuple(params: &[Param]) -> bool {
    params
        .iter()
        .all(|param| !param.name.is_empty() && param.name.bytes().all(|byte| byte.is_ascii_digit()))
}

fn lua_args(params: &[Param]) -> String {
    let mut parts = Vec::new();

    for param in params {
        let mut piece = String::new();

        if !param.ty.is_empty() {
            piece.push_str(&param.ty);
            piece.push(' ');
        }

        piece.push_str(&param.name);

        if param.optional {
            piece.push_str(" = nil");
        }

        parts.push(piece);
    }

    parts.join(", ")
}

fn rust_args(params: &[Param]) -> String {
    let mut parts = Vec::new();

    for param in params {
        // The receiver keeps its own spelling: `&self`, `&mut self`, `self: Box<Self>`.
        if param.name == "self" {
            parts.push(if param.ty.is_empty() {
                "self".to_string()
            } else {
                param.ty.clone()
            });

            continue;
        }

        if param.ty.is_empty() {
            parts.push(param.name.clone());

            continue;
        }

        parts.push(format!("{}: {}", param.name, param.ty));
    }

    parts.join(", ")
}

fn rust_returns(returns: &[Ret]) -> String {
    let types: Vec<&str> = returns
        .iter()
        .map(|item| item.ty.as_str())
        .filter(|ty| !ty.is_empty())
        .collect();

    match types.as_slice() {
        [] => String::new(),
        [only] => format!(" -> {only}"),
        many => format!(" -> ({})", many.join(", ")),
    }
}

pub fn assemble(mut pages: Vec<Page>) -> Result<Vec<Page>, String> {
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut errors = Vec::new();

    for page in &pages {
        if let Some(previous) = seen.insert(page.key(), page.origin.clone()) {
            errors.push(format!(
                "duplicate {}\n  {previous}\n  {}",
                page.key(),
                page.origin
            ));
        }
    }

    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }

    let mut extra = Vec::new();
    let mut considered = HashSet::new();

    for page in &pages {
        if page.parent.is_empty() {
            continue;
        }

        let parent = key(page.lang, &page.parent);

        if seen.contains_key(&parent) || !considered.insert(parent) {
            continue;
        }

        extra.push(synthetic(&pages, page.lang, &page.parent));
    }

    pages.extend(extra);
    inherit(&mut pages);
    let known: HashSet<String> = pages.iter().map(Page::key).collect();
    let mut unknown = Vec::new();

    for page in &mut pages {
        let origin = page.origin.clone();
        let id = page.id.clone();
        let lang = page.lang;

        for target in &mut page.see_also {
            match resolve_reference(&known, lang, target) {
                Some(resolved) => *target = resolved,
                None => unknown.push(format!(
                    "{id} lists see_also \"{target}\", which is not a page\n  {origin}"
                )),
            }
        }
    }

    if !unknown.is_empty() {
        return Err(unknown.join("\n"));
    }

    pages.sort_by(|left, right| {
        (left.lang.name(), &left.id).cmp(&(right.lang.name(), &right.id))
    });

    Ok(pages)
}

/// Members without a category or realm take their parent's, so a type only has to say once
/// where it belongs and where it runs.
fn inherit(pages: &mut [Page]) {
    let parents: HashMap<String, (Option<String>, Option<Realm>)> = pages
        .iter()
        .map(|page| (page.key(), (page.category.clone(), page.realm)))
        .collect();

    for page in pages.iter_mut() {
        if page.parent.is_empty() {
            continue;
        }

        let Some((category, realm)) = parents.get(&key(page.lang, &page.parent)) else {
            continue;
        };

        if page.category.is_none() {
            page.category = category.clone();
        }

        if page.realm.is_none() {
            page.realm = *realm;
        }
    }
}

/// Finds the page a see_also entry means, in the same reference unless it starts with `rust/`,
/// `lua/` or `wasm/`. `Thing.poke`, `Thing:poke` and `Thing::poke` all find whichever one exists.
fn resolve_reference(known: &HashSet<String>, lang: Lang, target: &str) -> Option<String> {
    let (lang, target) = match target.split_once('/') {
        Some(("rust", rest)) => (Lang::Rust, rest),
        Some(("lua", rest)) => (Lang::Lua, rest),
        Some(("wasm", rest)) => (Lang::Wasm, rest),
        _ => (lang, target),
    };
    let exact = key(lang, target);

    if known.contains(&exact) {
        return Some(exact);
    }

    let at = target.rfind(['.', ':'])?;
    let parent = target[..at].trim_end_matches(':');
    let name = &target[at + 1..];

    if parent.is_empty() || name.is_empty() {
        return None;
    }

    [".", ":", "::"]
        .iter()
        .map(|separator| key(lang, &format!("{parent}{separator}{name}")))
        .find(|candidate| known.contains(candidate))
}

fn parent_kind(pages: &[Page], lang: Lang, parent: &str) -> Kind {
    let kids: Vec<Kind> = pages
        .iter()
        .filter(|page| page.lang == lang && page.parent == parent)
        .map(|page| page.kind)
        .collect();
    let has = |kinds: &[Kind]| kids.iter().any(|kind| kinds.contains(kind));

    match lang {
        Lang::Lua if has(&[Kind::Method, Kind::Hook, Kind::Field]) => Kind::Class,
        Lang::Lua | Lang::Wasm => Kind::Library,
        Lang::Rust if has(&[Kind::Variant]) => Kind::Enum,
        Lang::Rust if has(&[Kind::Method, Kind::Field]) => Kind::Struct,
        Lang::Rust => Kind::Module,
    }
}

fn synthetic(pages: &[Page], lang: Lang, name: &str) -> Page {
    let kind = parent_kind(pages, lang, name);
    let category = pages
        .iter()
        .filter(|page| page.lang == lang && page.parent == name)
        .find_map(|page| page.category.clone());

    Page {
        id: name.to_string(),
        lang,
        parent: String::new(),
        name: name.to_string(),
        kind,
        category,
        realm: None,
        summary: format!("Reference for {name}."),
        syntax: syntax(lang, kind, "", name, &[], &[]),
        params: Vec::new(),
        returns: Vec::new(),
        example: None,
        note: None,
        panics: None,
        safety: None,
        since: None,
        see_also: Vec::new(),
        deprecated: None,
        deprecated_since: None,
        unimplemented: None,
        access: Access::Public,
        origin: "<generated>".to_string(),
    }
}

pub fn scan(root: &Path) -> Result<Vec<Page>, String> {
    let mut pages = Vec::new();
    walk(root, &mut pages)?;

    assemble(pages)
}

fn walk(dir: &Path, pages: &mut Vec<Page>) -> Result<(), String> {
    let listing =
        fs::read_dir(dir).map_err(|err| format!("failed to read {}: {err}", dir.display()))?;
    let mut entries: Vec<_> = listing
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| err.to_string())?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();

        if path.is_dir() {
            walk(&path, pages)?;

            continue;
        }

        let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };

        if ext != "rs" && ext != "lua" {
            continue;
        }

        let source = fs::read_to_string(&path)
            .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
        let label = path.display().to_string();
        let found = if ext == "rs" {
            parse_rust_source(&label, &source)?
        } else {
            parse_lua_source(&label, &source)?
        };
        pages.extend(found);
    }

    Ok(())
}

pub fn render(pages: &[Page], guides: &[guides::Guide]) -> String {
    html::render(pages, guides)
}

pub fn generate(src: &Path, dest: &Path) -> Result<(), String> {
    let pages = scan(src)?;
    // The guides live beside src: docs/ and the README of the same repository.
    let guides = match src.parent() {
        Some(root) => guides::read(root)?,
        None => Vec::new(),
    };
    let html = render(&pages, &guides);

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }

    fs::write(dest, html).map_err(|err| format!("failed to write {}: {err}", dest.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests;
