use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{ItemFn, ReturnType, parse_macro_input};

#[proc_macro_attribute]
pub fn main(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = parse_macro_input!(item as ItemFn);

    if func.sig.ident != "main" {
        return syn::Error::new_spanned(
            &func.sig.ident,
            "#[hecate::main] must be used on a function named `main`",
        )
        .to_compile_error()
        .into();
    }

    if func.sig.constness.is_some() {
        return syn::Error::new_spanned(&func.sig.constness, "const main is not supported")
            .to_compile_error()
            .into();
    }

    if func.sig.asyncness.is_some() {
        return syn::Error::new_spanned(&func.sig.asyncness, "async main is not supported")
            .to_compile_error()
            .into();
    }

    if !func.sig.inputs.is_empty() {
        return syn::Error::new_spanned(
            &func.sig.inputs,
            "main function must not accept arguments",
        )
        .to_compile_error()
        .into();
    }

    if !func.sig.generics.params.is_empty() {
        return syn::Error::new_spanned(&func.sig.generics, "generic main is not supported")
            .to_compile_error()
            .into();
    }

    let vis = func.vis;
    let attrs = func.attrs;
    let mut sig = func.sig;
    let block = func.block;
    let user_main = format_ident!("__hecate_user_main");
    sig.ident = user_main.clone();

    let invoke = match sig.output {
        ReturnType::Default => quote! {
            let code: ::hecate::ExitCode =
                ::hecate::MainReturn::into_exit_code(#user_main());
            ::hecate::process::exit(code)
        },
        ReturnType::Type(_, _) => quote! {
            let code: ::hecate::ExitCode =
                ::hecate::MainReturn::into_exit_code(#user_main());
            ::hecate::process::exit(code)
        },
    };

    let expanded = quote! {
        #(#attrs)*
        #vis #sig #block

        #[unsafe(no_mangle)]
        pub extern "C" fn _start() -> ! {
            #invoke
        }

        #[panic_handler]
        fn __hecate_panic(_info: &core::panic::PanicInfo<'_>) -> ! {
            ::hecate::hprintln!("panic");
            ::hecate::process::exit(1)
        }
    };

    expanded.into()
}
