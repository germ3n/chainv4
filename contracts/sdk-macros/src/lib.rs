//! The #[export] attribute. Re-exported by chainv4-sdk; use it from there.

use {
    proc_macro::TokenStream,
    proc_macro2::{Literal, Span},
    quote::quote,
    syn::{Ident, ItemFn, parse_macro_input, spanned::Spanned},
};

/// The custom section the engine reads reentrant functions from. Must match the node's.
const REENTRANT_SECTION: &str = "chainv4.reentrant";

/// Exports a function so transactions and other contracts can call it by name.
///
/// The function takes and returns nothing: read arguments with `input()` and set return values
/// with `output()`. While its contract is already running further up the chain of calls, the
/// function refuses to run, unless it is exported with `#[export(reentrant)]`.
#[proc_macro_attribute]
pub fn export(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    let reentrant = match attr.to_string().trim() {
        "" => false,
        "reentrant" => true,
        other => return error(Span::call_site(), &format!("unknown option `{other}`; the only one is `reentrant`")),
    };
    let signature = &function.sig;

    if !signature.inputs.is_empty() || !matches!(signature.output, syn::ReturnType::Default) {
        return error(
            signature.span(),
            "an exported function takes and returns nothing; use input() and output()",
        );
    }

    if !signature.generics.params.is_empty() || signature.asyncness.is_some() {
        return error(signature.span(), "an exported function cannot be generic or async");
    }

    let name = &signature.ident;
    let attrs = &function.attrs;
    let body = &function.block;
    let marker = reentrant.then(|| {
        let mut bytes = name.to_string().into_bytes();
        bytes.push(0);
        let len = bytes.len();
        let bytes = Literal::byte_string(&bytes);
        let marker = Ident::new(&format!("__CHAINV4_REENTRANT_{}", name.to_string().to_uppercase()), name.span());

        quote! {
            #[cfg(target_arch = "wasm32")]
            #[used]
            #[unsafe(link_section = #REENTRANT_SECTION)]
            static #marker: [u8; #len] = *#bytes;
        }
    });

    quote! {
        #(#attrs)*
        #[unsafe(no_mangle)]
        pub extern "C" fn #name() #body

        #marker
    }
    .into()
}

fn error(span: Span, message: &str) -> TokenStream {
    syn::Error::new(span, message).to_compile_error().into()
}
