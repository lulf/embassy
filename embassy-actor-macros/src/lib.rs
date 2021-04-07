#![feature(concat_idents)]

extern crate proc_macro;

use paste::paste;
use proc_macro::TokenStream;
use quote::quote;
use syn::{self, Ident};

#[proc_macro_derive(ActorProcess)]
pub fn actor_process_macro_derive(input: TokenStream) -> TokenStream {
    // Construct a representation of Rust code as a syntax tree
    // that we can manipulate
    let ast = syn::parse(input).unwrap();

    // Build the trait implementation
    impl_actor_process_macro(&ast)
}

fn impl_actor_process_macro(ast: &syn::DeriveInput) -> TokenStream {
    let name = &ast.ident;
    //    let lowercase_name = Ident::new(&name.to_string().to_lowercase(), name.span());
    let gen = quote! {
        #[embassy::task]
        async fn actor_myactor(state: &'static ActorState<'static, #name>) {
            let channel = &state.channel;
            let mut actor = state.actor.borrow_mut();
            loop {
                let request = channel.receive().await;
                #name::process(&mut actor, request).await;
            }
        }
    };
    gen.into()
}
