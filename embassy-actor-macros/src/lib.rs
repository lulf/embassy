extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn;

#[proc_macro_derive(ActorProcessor)]
pub fn hello_macro_derive(input: TokenStream) -> TokenStream {
    // Construct a representation of Rust code as a syntax tree
    // that we can manipulate
    let ast = syn::parse(input).unwrap();

    // Build the trait implementation
    impl_hello_macro(&ast)
}

fn impl_hello_macro(ast: &syn::DeriveInput) -> TokenStream {
    let name = &ast.ident;
    let gen = quote! {
        #[embassy::task]
        async fn handle_myactor(state: &'static ActorState<'static, MyActor>) {
            let channel = &state.channel;
            let mut actor = state.actor.borrow_mut();
            loop {
                log::info!("Awaiting request");
                let request = channel.receive().await;
                #name::process(&mut actor, request).await;
            }
        }
    };
    gen.into()
}
