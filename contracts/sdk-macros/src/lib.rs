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
/// Its parameters are read from the call's arguments, in order, by their types' `FromValue`; a
/// call that passes too many, leaves one out or passes the wrong kind aborts, naming the
/// parameter. `Option` parameters may be left out. What it returns becomes the call's return
/// values by `IntoOutput`: nothing, one value, a tuple of them, or a `Result` whose error aborts.
/// A function that takes and returns nothing is exported as it is, and may read its arguments
/// with `input()` and set its return values with `output()` itself.
///
/// While its contract is already running further up the chain of calls, the function refuses to
/// run, unless it is exported with `#[export(reentrant)]`.
#[proc_macro_attribute]
pub fn export(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    let reentrant = match attr.to_string().trim() {
        "" => false,
        "reentrant" => true,
        other => return error(Span::call_site(), &format!("unknown option `{other}`; the only one is `reentrant`")),
    };
    let signature = &function.sig;

    if !signature.generics.params.is_empty() || signature.asyncness.is_some() {
        return error(signature.span(), "an exported function cannot be generic or async");
    }

    let name = &signature.ident;
    let attrs = &function.attrs;
    let body = &function.block;
    let typed = !signature.inputs.is_empty() || !matches!(signature.output, syn::ReturnType::Default);
    let entry = match typed {
        true => match typed_entry(&function) {
            Ok(entry) => entry,
            Err(err) => return err.to_compile_error().into(),
        },
        false => quote! {
            #(#attrs)*
            #[unsafe(no_mangle)]
            pub extern "C" fn #name() #body
        },
    };
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
        #entry

        #marker
    }
    .into()
}

/// The entry point for a function with parameters or a result: it reads the call's arguments
/// into the parameters, runs the function as written, and returns what it returns.
fn typed_entry(function: &ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    let signature = &function.sig;
    let name = &signature.ident;
    let label = name.to_string();
    let attrs = &function.attrs;
    let mut bindings = Vec::new();
    let mut arguments = Vec::new();

    for input in &signature.inputs {
        let syn::FnArg::Typed(typed) = input else {
            return Err(syn::Error::new(input.span(), "an exported function takes no self"));
        };
        let syn::Pat::Ident(pattern) = &*typed.pat else {
            return Err(syn::Error::new(typed.pat.span(), "an exported function's parameters must be plain names"));
        };
        let parameter = &pattern.ident;
        let parameter_name = parameter.to_string();
        let ty = &typed.ty;

        bindings.push(quote! { let #parameter: #ty = __args.take(#parameter_name); });
        arguments.push(quote! { #parameter });
    }

    let count = arguments.len();

    Ok(quote! {
        #[unsafe(no_mangle)]
        pub extern "C" fn #name() {
            #(#attrs)*
            #function

            let __values = ::chainv4_sdk::input();
            let mut __args = ::chainv4_sdk::args::Args::new(&__values, #label, #count);
            #(#bindings)*
            let __output = ::chainv4_sdk::IntoOutput::into_output(#name(#(#arguments),*));
            ::chainv4_sdk::output(&__output);
        }
    })
}

fn error(span: Span, message: &str) -> TokenStream {
    syn::Error::new(span, message).to_compile_error().into()
}
