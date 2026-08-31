use proc_macro::TokenStream;
use quote::ToTokens;
use syn::{parse_macro_input, parse_quote, Block, ItemFn};

#[proc_macro_attribute]
pub fn instrument(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut fn_item = parse_macro_input!(input as ItemFn);

    let fn_name = &fn_item.sig.ident;

    // Wrap the body in debug logging
    let body = &fn_item.block;
    let stmts = &body.stmts;

    let new_body: Block = parse_quote!({
        #[cfg(feature = "debug")]
        {
            ::inkdb::debug::log_enter(stringify!(#fn_name));
        }

        let result = (|| { #(#stmts)* })();

        #[cfg(feature = "debug")]
        {
            ::inkdb::debug::log_exit(stringify!(#fn_name));
        }

        result
    });

    fn_item.block = Box::new(new_body);
    TokenStream::from(fn_item.into_token_stream())
}
