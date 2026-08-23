use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use syn::{
    parse_macro_input, parse_quote, Block, FnArg, Ident, ItemFn, Pat, PatType, ReturnType, Stmt, Type, TypePath,
};

struct InstrumentAttr {
    kind: syn::Path,
}

impl syn::parse::Parse for InstrumentAttr {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let kind: syn::Path = input.parse()?;
        Ok(Self { kind })
    }
}

#[proc_macro_attribute]
pub fn instrument(args: TokenStream, input: TokenStream) -> TokenStream {
    let attr = parse_macro_input!(args as InstrumentAttr);
    let mut fn_item = parse_macro_input!(input as ItemFn);

    let kind = &attr.kind;
    let fn_name = &fn_item.sig.ident;
    let inputs = &fn_item.sig.inputs;
    let output = &fn_item.sig.output;
    let generics = &fn_item.sig.generics;
    let attrs = &fn_item.attrs;
    let vis = &fn_item.vis;
    let asyncness = &fn_item.sig.asyncness;
    let constness = &fn_item.sig.constness;
    let unsafety = &fn_item.sig.unsafety;
    let abi = &fn_item.sig.abi;

    let body = &fn_item.block;
    let stmts = &body.stmts;

    let has_result_return = matches!(&output, ReturnType::Type(_, ty) if is_result_type(ty));

    let success_event = if has_result_return {
        quote! {
            if let Ok(ref __result) = __result {
                trace_event!(#kind, build_success_data(#kind, __result, stringify!(#fn_name)));
            } else if let Err(ref __err) = __result {
                trace_event!(#kind, build_error_data(#kind, __err, stringify!(#fn_name)));
            }
        }
    } else {
        quote! {}
    };

    let new_body: Block = parse_quote!({
        #[cfg(feature = "debug-instrument")]
        {
            use crate::debug::trace::{EventData, EventKind};
            use crate::debug::trace::{build_success_data, build_error_data};
            use crate::debug::seq::{next_seq, push_ctx, pop_ctx, current_ctx};
            use crate::trace_event;

            let __seq = next_seq();
            let __parent = current_ctx().unwrap_or(0);
            push_ctx(__seq);
            trace_event!(#kind, EventData::default(), {
                let __result = (|| { #(#stmts)* })();
                #success_event
                pop_ctx();
                __result
            })
        }
        #[cfg(not(feature = "debug-instrument"))]
        {
            #(#stmts)*
        }
    });

    let new_fn = ItemFn {
        attrs: attrs.clone(),
        vis: vis.clone(),
        sig: syn::Signature {
            constness: constness.clone(),
            asyncness: asyncness.clone(),
            unsafety: unsafety.clone(),
            abi: abi.clone(),
            fn_token: Default::default(),
            ident: fn_name.clone(),
            generics: generics.clone(),
            paren_token: Default::default(),
            inputs: inputs.clone(),
            variadic: None,
            output: output.clone(),
        },
        block: Box::new(new_body),
    };

    new_fn.into_token_stream().into()
}

fn is_result_type(ty: &Type) -> bool {
    if let Type::Path(TypePath { path, .. }) = ty {
        path.segments.last().map(|s| s.ident == "Result").unwrap_or(false)
    } else {
        false
    }
}

#[proc_macro]
pub fn trace_point(_input: TokenStream) -> TokenStream {
    TokenStream::new()
}