use proc_macro::TokenStream;
use quote::quote;

/// Documents the item it sits on for the wiki and checks the documentation at compile time.
///
/// On an enum or struct, `#[document]` may also sit on variants and fields to document those
/// members. The compiler would reject the attribute there, so this strips them after reading.
#[proc_macro_attribute]
pub fn document(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = proc_macro2::TokenStream::from(attr);
    let item = proc_macro2::TokenStream::from(item);

    match syn::parse2::<syn::Item>(item.clone()) {
        // syn reads a trait method without a body as a verbatim item.
        Ok(syn::Item::Verbatim(_)) | Err(_) => expand_other(attr, item),
        Ok(parsed) => expand_item(attr, parsed),
    }
    .into()
}

fn expand_item(attr: proc_macro2::TokenStream, mut item: syn::Item) -> proc_macro2::TokenStream {
    let context = wiki::context_of(&item);
    let mut errors = Vec::new();
    let (parent, lang) = match wiki::parse_document(attr, &context, "<macro>") {
        Ok(page) => (page.name, page.lang),
        Err(err) => {
            errors.push(quote! { compile_error!(#err); });
            (context.name.clone().unwrap_or_default(), context.lang)
        }
    };

    for (member, member_context) in wiki::members_of(&item, &parent, lang) {
        if let Err(err) = wiki::parse_attribute(member, &member_context, "<macro>") {
            errors.push(syn::Error::new_spanned(member, err).to_compile_error());
        }
    }

    strip_members(&mut item);

    quote! {
        #(#errors)*
        #[allow(dead_code)]
        #item
    }
}

/// A trait method without a body is not an item, but it can still be documented.
fn expand_other(
    attr: proc_macro2::TokenStream,
    item: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let context = syn::parse2::<syn::TraitItemFn>(item.clone())
        .map(|func| wiki::fn_context(&func.sig))
        .unwrap_or_default();

    match wiki::parse_document(attr, &context, "<macro>") {
        Ok(_) => quote! {
            #[allow(dead_code)]
            #item
        },
        Err(err) => quote! {
            compile_error!(#err);
            #item
        },
    }
}

fn strip_members(item: &mut syn::Item) {
    match item {
        syn::Item::Enum(item) => {
            for variant in &mut item.variants {
                variant.attrs.retain(|attr| !wiki::is_document(attr));
            }
        }
        syn::Item::Struct(item) => {
            for field in item.fields.iter_mut() {
                field.attrs.retain(|attr| !wiki::is_document(attr));
            }
        }
        _ => {}
    }
}
