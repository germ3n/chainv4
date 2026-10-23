use crate::item::{attrs_of, context_of, fn_context, is_document, members_of};
use crate::{interpret, Access, Context, Kind, Lang, Page, Param, RawDoc, Realm, Ret};
use proc_macro2::Ident;
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{token, LitBool, LitStr, Token};

pub fn parse_document(
    input: proc_macro2::TokenStream,
    context: &Context,
    origin: &str,
) -> Result<Page, String> {
    let raw = syn::parse2::<RawDoc>(input).map_err(|err| format!("{origin}: {err}"))?;
    let mut page = interpret(raw, context).map_err(|err| format!("{origin}: {err}"))?;
    page.origin = origin.to_string();

    Ok(page)
}

pub fn parse_attribute(
    attr: &syn::Attribute,
    context: &Context,
    origin: &str,
) -> Result<Page, String> {
    let raw = attr
        .parse_args_with(RawDoc::parse)
        .map_err(|err| format!("{origin}: {err}"))?;
    let mut page = interpret(raw, context).map_err(|err| format!("{origin}: {err}"))?;
    page.origin = origin.to_string();

    Ok(page)
}

pub fn parse_rust_source(origin: &str, source: &str) -> Result<Vec<Page>, String> {
    let file = syn::parse_file(source).map_err(|err| format!("{origin}: {err}"))?;
    let mut collector = Collector {
        origin,
        pages: Vec::new(),
        errors: Vec::new(),
    };
    collector.visit_file(&file);

    if !collector.errors.is_empty() {
        return Err(collector.errors.join("\n"));
    }

    Ok(collector.pages)
}

pub fn parse_lua_source(origin: &str, source: &str) -> Result<Vec<Page>, String> {
    let mut pages = Vec::new();
    let mut errors = Vec::new();
    let bytes = source.as_bytes();
    let mut idx = 0usize;

    while idx + 4 < bytes.len() {
        let opener = lua_opener(bytes, idx);

        if let Some((opener_len, closer)) = opener {
            let content_at = idx + opener_len;
            let line = source[..idx].bytes().filter(|byte| *byte == b'\n').count() + 1;
            let rest = &source[content_at..];
            let trimmed = rest.trim_start();
            let skipped = rest.len() - trimmed.len();
            let word = "document";

            if let Some(after) = trimmed.strip_prefix(word) {
                let boundary = after.chars().next();
                let continues = boundary.is_some_and(|ch| ch == '_' || ch.is_ascii_alphanumeric());

                if !continues {
                    if let Some(rel) = source[content_at..].find(closer) {
                        let end = content_at + rel;
                        let body = source[content_at + skipped + word.len()..end].trim();
                        let block_origin = format!("{origin}:{line}");

                        match parse_lua_body(&block_origin, body) {
                            Ok(page) => pages.push(page),
                            Err(err) => errors.push(err),
                        }

                        idx = end + closer.len();

                        continue;
                    }

                    errors.push(format!("{origin}:{line}: unclosed document comment"));

                    break;
                }
            }

            if let Some(rel) = source[content_at..].find(closer) {
                idx = content_at + rel + closer.len();

                continue;
            }
        }

        idx += 1;
    }

    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }

    Ok(pages)
}

fn parse_lua_body(origin: &str, body: &str) -> Result<Page, String> {
    let stream = body
        .parse::<proc_macro2::TokenStream>()
        .map_err(|err| format!("{origin}: {err}"))?;

    parse_document(stream, &Context::lang(Lang::Lua), origin)
}

fn lua_opener(bytes: &[u8], idx: usize) -> Option<(usize, &'static str)> {
    if bytes[idx] != b'-' || bytes[idx + 1] != b'-' || bytes[idx + 2] != b'[' {
        return None;
    }

    if bytes[idx + 3] == b'[' {
        return Some((4, "]]"));
    }

    if idx + 5 <= bytes.len() && bytes[idx + 3] == b'=' && bytes[idx + 4] == b'[' {
        return Some((5, "]=]"));
    }

    None
}

struct Collector<'a> {
    origin: &'a str,
    pages: Vec<Page>,
    errors: Vec<String>,
}

impl<'ast> Visit<'ast> for Collector<'_> {
    fn visit_item(&mut self, node: &'ast syn::Item) {
        let context = context_of(node);
        let owner = self.take(attrs_of(node), &context);
        let (parent, lang) =
            owner.unwrap_or_else(|| (context.name.clone().unwrap_or_default(), context.lang));

        for (attr, member) in members_of(node, &parent, lang) {
            self.push(attr, &member);
        }

        syn::visit::visit_item(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.take(&node.attrs, &fn_context(&node.sig));
        syn::visit::visit_impl_item_fn(self, node);
    }

    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        self.take(&node.attrs, &fn_context(&node.sig));
        syn::visit::visit_trait_item_fn(self, node);
    }
}

impl Collector<'_> {
    /// Records every `#[document]` among `attrs`. Returns the name and language of the first
    /// page, which the item's documented variants or fields belong to.
    fn take(&mut self, attrs: &[syn::Attribute], context: &Context) -> Option<(String, Lang)> {
        let mut owner = None;

        for attr in attrs {
            if !is_document(attr) {
                continue;
            }

            if let Some(page) = self.push(attr, context) {
                owner.get_or_insert(page);
            }
        }

        owner
    }

    fn push(&mut self, attr: &syn::Attribute, context: &Context) -> Option<(String, Lang)> {
        let line = attr.span().start().line;
        let origin = format!("{}:{line}", self.origin);

        match parse_attribute(attr, context, &origin) {
            Ok(page) => {
                let owner = (page.name.clone(), page.lang);
                self.pages.push(page);

                Some(owner)
            }
            Err(err) => {
                self.errors.push(err);

                None
            }
        }
    }
}

enum Value {
    Str(String),
    Bool(bool),
    Map(Vec<(String, Value)>),
    List(Vec<Value>),
}

impl Parse for RawDoc {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut raw = RawDoc::default();

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            let name = key.to_string();

            if input.peek(Token![=]) {
                input.parse::<Token![=]>()?;
                let value = parse_value(input)?;
                apply(&mut raw, &name, value).map_err(|err| syn::Error::new(key.span(), err))?;
            } else {
                apply_flag(&mut raw, &name).map_err(|err| syn::Error::new(key.span(), err))?;
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(raw)
    }
}

fn parse_value(input: ParseStream) -> syn::Result<Value> {
    if input.peek(LitStr) {
        let lit: LitStr = input.parse()?;

        return Ok(Value::Str(lit.value()));
    }

    if input.peek(LitBool) {
        let lit: LitBool = input.parse()?;

        return Ok(Value::Bool(lit.value()));
    }

    if input.peek(token::Brace) {
        let content;
        syn::braced!(content in input);

        if content.is_empty() {
            return Ok(Value::Map(Vec::new()));
        }

        if content.peek(token::Brace) {
            let mut items = Vec::new();

            while !content.is_empty() {
                items.push(parse_value(&content)?);

                if content.peek(Token![,]) {
                    content.parse::<Token![,]>()?;
                }
            }

            return Ok(Value::List(items));
        }

        let mut fields = Vec::new();

        while !content.is_empty() {
            let key = parse_key(&content)?;
            content.parse::<Token![=]>()?;
            let value = parse_value(&content)?;
            fields.push((key, value));

            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            }
        }

        return Ok(Value::Map(fields));
    }

    Err(input.error("expected a string, bool, or table"))
}

fn parse_key(input: ParseStream) -> syn::Result<String> {
    if input.peek(LitStr) {
        let lit: LitStr = input.parse()?;

        return Ok(lit.value());
    }

    let ident: Ident = input.parse()?;

    Ok(ident.to_string())
}

fn apply(raw: &mut RawDoc, key: &str, value: Value) -> Result<(), String> {
    match key {
        "parent" => set_string(&mut raw.parent, key, value)?,
        "name" => set_string(&mut raw.name, key, value)?,
        "kind" => {
            let text = expect_string(key, value)?;
            if raw.kind.is_some() {
                return Err(format!("duplicate field {key}"));
            }
            raw.kind = Some(parse_kind(&text)?);
        }
        "lang" => {
            let text = expect_string(key, value)?;
            set_lang(raw, parse_lang(&text)?)?;
        }
        "category" => set_string(&mut raw.category, key, value)?,
        "realm" => {
            let text = expect_string(key, value)?;
            if raw.realm.is_some() {
                return Err(format!("duplicate field {key}"));
            }
            raw.realm = Some(parse_realm(&text)?);
        }
        "summary" => set_string(&mut raw.summary, key, value)?,
        "params" => {
            if !raw.params.is_empty() {
                return Err("duplicate field params".to_string());
            }
            raw.params = params_from(value)?;
        }
        "returns" => {
            if !raw.returns.is_empty() {
                return Err("duplicate field returns".to_string());
            }
            raw.returns = returns_from(value)?;
        }
        "example" => set_string(&mut raw.example, key, value)?,
        "note" => set_string(&mut raw.note, key, value)?,
        "panics" => set_string(&mut raw.panics, key, value)?,
        "safety" => set_string(&mut raw.safety, key, value)?,
        "since" => set_string(&mut raw.since, key, value)?,
        "see_also" => {
            if !raw.see_also.is_empty() {
                return Err("duplicate field see_also".to_string());
            }
            raw.see_also = see_also_from(value)?;
        }
        "deprecated" => set_string(&mut raw.deprecated, key, value)?,
        "deprecated_since" => set_string(&mut raw.deprecated_since, key, value)?,
        "unimplemented" => {
            if raw.unimplemented.is_some() {
                return Err("duplicate field unimplemented".to_string());
            }
            raw.unimplemented = Some(Some(expect_string(key, value)?));
        }
        "access" => {
            let text = expect_string(key, value)?;
            set_access(raw, parse_access(&text)?)?;
        }
        "internal" => {
            let flag = match value {
                Value::Bool(flag) => flag,
                _ => {
                    return Err("internal must be a bool".to_string());
                }
            };
            let access = if flag {
                Access::Internal
            } else {
                Access::Public
            };
            set_access(raw, access)?;
        }
        other => {
            return Err(format!("unknown field {other}"));
        }
    }

    Ok(())
}

fn apply_flag(raw: &mut RawDoc, key: &str) -> Result<(), String> {
    match key {
        "unimplemented" => {
            if raw.unimplemented.is_some() {
                return Err("duplicate field unimplemented".to_string());
            }

            raw.unimplemented = Some(None);

            Ok(())
        }
        "internal" => set_access(raw, Access::Internal),
        "public" => set_access(raw, Access::Public),
        "rust" => set_lang(raw, Lang::Rust),
        "lua" => set_lang(raw, Lang::Lua),
        "wasm" => set_lang(raw, Lang::Wasm),
        other => Err(format!("unknown field {other}")),
    }
}

fn set_access(raw: &mut RawDoc, access: Access) -> Result<(), String> {
    if raw.access.is_some() {
        return Err("duplicate field access".to_string());
    }

    raw.access = Some(access);

    Ok(())
}

fn set_lang(raw: &mut RawDoc, lang: Lang) -> Result<(), String> {
    if raw.lang.is_some() {
        return Err("duplicate field lang".to_string());
    }

    raw.lang = Some(lang);

    Ok(())
}

fn parse_access(text: &str) -> Result<Access, String> {
    match text {
        "public" => Ok(Access::Public),
        "internal" => Ok(Access::Internal),
        other => Err(format!("unknown access \"{other}\"")),
    }
}

fn parse_lang(text: &str) -> Result<Lang, String> {
    match text {
        "rust" => Ok(Lang::Rust),
        "lua" => Ok(Lang::Lua),
        "wasm" => Ok(Lang::Wasm),
        other => Err(format!("unknown lang \"{other}\", expected \"rust\", \"lua\" or \"wasm\"")),
    }
}

fn set_string(slot: &mut Option<String>, key: &str, value: Value) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("duplicate field {key}"));
    }

    *slot = Some(expect_string(key, value)?);

    Ok(())
}

fn expect_string(key: &str, value: Value) -> Result<String, String> {
    match value {
        Value::Str(text) => Ok(text),
        _ => Err(format!("{key} must be a string")),
    }
}

fn parse_kind(text: &str) -> Result<Kind, String> {
    match text {
        "library" => Ok(Kind::Library),
        "class" => Ok(Kind::Class),
        "hook" => Ok(Kind::Hook),
        "module" => Ok(Kind::Module),
        "struct" => Ok(Kind::Struct),
        "enum" => Ok(Kind::Enum),
        "trait" => Ok(Kind::Trait),
        "type" => Ok(Kind::Type),
        "variant" => Ok(Kind::Variant),
        "constant" => Ok(Kind::Constant),
        "function" => Ok(Kind::Function),
        "method" => Ok(Kind::Method),
        "field" => Ok(Kind::Field),
        other => Err(format!("unknown kind \"{other}\"")),
    }
}

fn parse_realm(text: &str) -> Result<Realm, String> {
    let (mut client, mut server, mut menu) = (false, false, false);

    for part in text.split([',', '+']) {
        match part.trim() {
            "client" => client = true,
            "server" => server = true,
            "shared" => {
                client = true;
                server = true;
            }
            "menu" => menu = true,
            other => return Err(format!("unknown realm \"{other}\"")),
        }
    }

    Realm::from_flags(client, server, menu).ok_or_else(|| "empty realm".to_string())
}

fn params_from(value: Value) -> Result<Vec<Param>, String> {
    let Value::Map(entries) = value else {
        return Err("params must be a table".to_string());
    };
    let mut params = Vec::new();

    for (name, value) in entries {
        match value {
            Value::Str(desc) => params.push(Param {
                name,
                ty: String::new(),
                desc,
                optional: false,
            }),
            Value::Map(fields) => {
                let mut ty = String::new();
                let mut desc = String::new();
                let mut optional = false;

                for (key, field) in fields {
                    match key.as_str() {
                        "ty" => ty = expect_string("ty", field)?,
                        "desc" => desc = expect_string("desc", field)?,
                        "optional" => {
                            optional = match field {
                                Value::Bool(flag) => flag,
                                _ => {
                                    return Err("optional must be a bool".to_string());
                                }
                            };
                        }
                        other => {
                            return Err(format!("unknown param field {other}"));
                        }
                    }
                }

                params.push(Param {
                    name,
                    ty,
                    desc,
                    optional,
                });
            }
            _ => {
                return Err(format!("param {name} must be a string or table"));
            }
        }
    }

    Ok(params)
}

fn returns_from(value: Value) -> Result<Vec<Ret>, String> {
    match value {
        Value::Str(desc) => Ok(vec![Ret {
            ty: String::new(),
            desc,
        }]),
        Value::Map(_) => Ok(vec![ret_from(value)?]),
        Value::List(items) => {
            let mut out = Vec::new();

            for item in items {
                out.push(ret_from(item)?);
            }

            Ok(out)
        }
        _ => Err("returns must be a string or table".to_string()),
    }
}

fn ret_from(value: Value) -> Result<Ret, String> {
    let Value::Map(fields) = value else {
        return Err("return must be a table".to_string());
    };
    let mut ty = String::new();
    let mut desc = String::new();

    for (key, field) in fields {
        match key.as_str() {
            "ty" => ty = expect_string("ty", field)?,
            "desc" => desc = expect_string("desc", field)?,
            other => {
                return Err(format!("unknown return field {other}"));
            }
        }
    }

    Ok(Ret { ty, desc })
}

fn see_also_from(value: Value) -> Result<Vec<String>, String> {
    let Value::Str(text) = value else {
        return Err("see_also must be a string".to_string());
    };

    Ok(text
        .split(',')
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect())
}
