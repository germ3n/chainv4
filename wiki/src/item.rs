use crate::{Context, Lang, Param, Ret, Shape};
use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};
use quote::ToTokens;

pub fn is_document(attr: &syn::Attribute) -> bool {
    attr.path().is_ident("document")
}

pub fn attrs_of(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(item) => &item.attrs,
        syn::Item::Enum(item) => &item.attrs,
        syn::Item::ExternCrate(item) => &item.attrs,
        syn::Item::Fn(item) => &item.attrs,
        syn::Item::ForeignMod(item) => &item.attrs,
        syn::Item::Impl(item) => &item.attrs,
        syn::Item::Macro(item) => &item.attrs,
        syn::Item::Mod(item) => &item.attrs,
        syn::Item::Static(item) => &item.attrs,
        syn::Item::Struct(item) => &item.attrs,
        syn::Item::Trait(item) => &item.attrs,
        syn::Item::TraitAlias(item) => &item.attrs,
        syn::Item::Type(item) => &item.attrs,
        syn::Item::Union(item) => &item.attrs,
        syn::Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

pub fn context_of(item: &syn::Item) -> Context {
    let (ident, shape) = match item {
        syn::Item::Fn(item) => return fn_context(&item.sig),
        syn::Item::Mod(item) => (&item.ident, Shape::Module),
        syn::Item::Struct(item) => (&item.ident, Shape::Struct),
        syn::Item::Union(item) => (&item.ident, Shape::Struct),
        syn::Item::Enum(item) => (&item.ident, Shape::Enum),
        syn::Item::Trait(item) => (&item.ident, Shape::Trait),
        syn::Item::Type(item) => (&item.ident, Shape::Alias { ty: tidy(&item.ty) }),
        syn::Item::Const(item) => (&item.ident, Shape::Constant { ty: tidy(&item.ty) }),
        syn::Item::Static(item) => (&item.ident, Shape::Constant { ty: tidy(&item.ty) }),
        _ => return Context::default(),
    };

    Context {
        name: Some(ident.to_string()),
        shape: Some(shape),
        ..Context::default()
    }
}

pub fn fn_context(sig: &syn::Signature) -> Context {
    let mut params = Vec::new();

    for input in &sig.inputs {
        let (name, ty) = match input {
            syn::FnArg::Receiver(receiver) => ("self".to_string(), tidy(receiver)),
            syn::FnArg::Typed(typed) => (pattern_name(&typed.pat), tidy(&typed.ty)),
        };

        params.push(Param {
            name,
            ty,
            desc: String::new(),
            optional: false,
        });
    }

    let returns = match &sig.output {
        syn::ReturnType::Type(_, ty) => vec![Ret {
            ty: tidy(ty),
            desc: String::new(),
        }],
        syn::ReturnType::Default => Vec::new(),
    };

    Context {
        name: Some(sig.ident.to_string()),
        shape: Some(Shape::Function { params, returns }),
        ..Context::default()
    }
}

fn pattern_name(pat: &syn::Pat) -> String {
    match pat {
        syn::Pat::Ident(pat) => pat.ident.to_string(),
        other => tidy(other),
    }
}

/// The `#[document]` attributes on an enum's variants and a struct's fields, each with the
/// context of its member. `parent` and `lang` come from the item's own page.
pub fn members_of<'a>(
    item: &'a syn::Item,
    parent: &str,
    lang: Lang,
) -> Vec<(&'a syn::Attribute, Context)> {
    let mut out = Vec::new();

    match item {
        syn::Item::Enum(item) => {
            for variant in &item.variants {
                let shape = Shape::Variant {
                    params: field_params(&variant.fields),
                };
                let name = variant.ident.to_string();
                push_members(&mut out, &variant.attrs, &name, shape, parent, lang);
            }
        }
        syn::Item::Struct(item) => {
            for (index, field) in item.fields.iter().enumerate() {
                let name = match &field.ident {
                    Some(ident) => ident.to_string(),
                    None => index.to_string(),
                };
                let shape = Shape::Field {
                    ty: tidy(&field.ty),
                };
                push_members(&mut out, &field.attrs, &name, shape, parent, lang);
            }
        }
        _ => {}
    }

    out
}

fn push_members<'a>(
    out: &mut Vec<(&'a syn::Attribute, Context)>,
    attrs: &'a [syn::Attribute],
    name: &str,
    shape: Shape,
    parent: &str,
    lang: Lang,
) {
    for attr in attrs {
        if !is_document(attr) {
            continue;
        }

        out.push((
            attr,
            Context {
                lang,
                name: Some(name.to_string()),
                parent: Some(parent.to_string()),
                shape: Some(shape.clone()),
            },
        ));
    }
}

fn field_params(fields: &syn::Fields) -> Vec<Param> {
    let mut params = Vec::new();

    for (index, field) in fields.iter().enumerate() {
        let name = match &field.ident {
            Some(ident) => ident.to_string(),
            None => index.to_string(),
        };

        params.push(Param {
            name,
            ty: tidy(&field.ty),
            desc: String::new(),
            optional: false,
        });
    }

    params
}

/// Prints tokens spaced the way rustfmt writes a type or pattern: `[u8; 32]`, `Option<&'a str>`,
/// `impl Fn(u8) -> bool`, `self: Box<Self>`.
pub fn tidy(tokens: &impl ToTokens) -> String {
    let mut out = String::new();
    write_tokens(tokens.to_token_stream(), &mut out);

    out
}

fn write_tokens(stream: TokenStream, out: &mut String) {
    // The next token sits right against the previous one.
    let mut glue = true;
    // The previous token was a word, so a following `(` hugs it: `Fn(u8)` but `-> (u8, u8)`.
    let mut word = false;
    // The previous token was the first half of `::`.
    let mut colon = false;

    for tree in stream {
        match tree {
            TokenTree::Group(group) => {
                let (open, close) = match group.delimiter() {
                    Delimiter::Parenthesis => ("(", ")"),
                    Delimiter::Bracket => ("[", "]"),
                    Delimiter::Brace => ("{ ", " }"),
                    Delimiter::None => ("", ""),
                };

                if !glue && !(word && open == "(") {
                    out.push(' ');
                }

                out.push_str(open);
                write_tokens(group.stream(), out);
                out.push_str(close);
                glue = false;
                word = false;
                colon = false;
            }
            TokenTree::Ident(ident) => {
                if !glue {
                    out.push(' ');
                }

                out.push_str(&ident.to_string());
                glue = false;
                word = true;
                colon = false;
            }
            TokenTree::Literal(literal) => {
                if !glue {
                    out.push(' ');
                }

                out.push_str(&literal.to_string());
                glue = false;
                word = false;
                colon = false;
            }
            TokenTree::Punct(punct) => {
                let ch = punct.as_char();
                let joint = punct.spacing() == Spacing::Joint;

                if !glue && !matches!(ch, ',' | ';' | ':' | '.' | '<' | '>') {
                    out.push(' ');
                }

                out.push(ch);
                glue = joint
                    || match ch {
                        ',' | ';' | '=' | '+' | '>' => false,
                        ':' => colon,
                        _ => true,
                    };
                word = false;
                colon = ch == ':' && joint;
            }
        }
    }
}
