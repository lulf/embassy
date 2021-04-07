#![feature(proc_macro_diagnostic)]
#![feature(concat_idents)]

extern crate proc_macro;

use darling::FromMeta;
use proc_macro::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::spanned::Spanned;
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

#[proc_macro_attribute]
pub fn main(args: TokenStream, item: TokenStream) -> TokenStream {
    let macro_args = syn::parse_macro_input!(args as syn::AttributeArgs);
    let task_fn = syn::parse_macro_input!(item as syn::ItemFn);

    let mut fail = false;
    if task_fn.sig.asyncness.is_none() {
        task_fn
            .sig
            .span()
            .unwrap()
            .error("task functions must be async")
            .emit();
        fail = true;
    }
    if !task_fn.sig.generics.params.is_empty() {
        task_fn
            .sig
            .span()
            .unwrap()
            .error("main function must not be generic")
            .emit();
        fail = true;
    }

    let args = task_fn.sig.inputs.clone();

    if args.len() != 1 {
        task_fn
            .sig
            .span()
            .unwrap()
            .error("main function must have one argument")
            .emit();
        fail = true;
    }

    if fail {
        return TokenStream::new();
    }

    let task_fn_body = task_fn.block.clone();

    let result = quote! {
        #[embassy::task]
        async fn __drogue_main(#args) {
            #task_fn_body
        }

        // TODO: Cortex-mi'ify #[cortex_m_rt::entry]
        fn main() -> ! {
            unsafe fn make_static<T>(t: &mut T) -> &'static mut T {
                ::core::mem::transmute(t)
            }

            let mut executor = embassy_std::Executor::new();
            let executor = unsafe { make_static(&mut executor) };

            executor.run(|spawner| {
                let mut device = device::Device::new();
                device.set_spawner(spawner);
                spawner.spawn(__drogue_main(device)).unwrap();
            })

        }
    };
    result.into()
}
