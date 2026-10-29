use std::{env, fs, path::PathBuf};

use quote::quote;
#[path = "src/generator.rs"]
mod generator;

fn main() {
    println!("cargo:rerun-if-changed=src/upstream/bindgen_bundled_version.rs");

    let source = include_str!("src/upstream/bindgen_bundled_version.rs");
    let (declarations, functions) = generator::parse_bindings(source)
        .unwrap_or_else(|error| panic!("invalid pinned bindings: {error}"));
    let field_declarations = functions.iter().map(|function| {
        let name = &function.sig.ident;
        let argument_types = function.sig.inputs.iter().map(|input| match input {
            syn::FnArg::Typed(argument) => argument.ty.as_ref(),
            syn::FnArg::Receiver(_) => panic!("unexpected receiver for {name}"),
        });
        let output = &function.sig.output;
        quote!(pub(crate) #name: unsafe extern "C" fn(#(#argument_types),*) #output)
    });
    let symbol_fields = functions.iter().map(|function| {
        let name = &function.sig.ident;
        let name_string = name.to_string();
        let mut symbol_name = name_string.as_bytes().to_vec();
        symbol_name.push(0);
        let symbol_literal = syn::LitByteStr::new(&symbol_name, name.span());
        let argument_types = function.sig.inputs.iter().map(|input| match input {
            syn::FnArg::Typed(argument) => argument.ty.as_ref(),
            syn::FnArg::Receiver(_) => panic!("unexpected receiver for {name}"),
        });
        let output = &function.sig.output;
        quote! {
            #name: unsafe {
                library.get::<unsafe extern "C" fn(#(#argument_types),*) #output>(#symbol_literal)
                    .map(|symbol| *symbol)
                    .map_err(|_| stringify!(#name).to_owned())?
            },
        }
    });
    let wrappers = functions.iter().map(|function| {
        let attrs = &function.attrs;
        let name = &function.sig.ident;
        let inputs = &function.sig.inputs;
        let output = &function.sig.output;
        let call_arguments = function.sig.inputs.iter().filter_map(|input| match input {
            syn::FnArg::Typed(argument) => match argument.pat.as_ref() {
                syn::Pat::Ident(identifier) => Some(identifier.ident.clone()),
                _ => panic!("unsupported argument pattern for {name}"),
            },
            syn::FnArg::Receiver(_) => unreachable!(),
        });
        quote! {
            #(#attrs)*
            pub unsafe fn #name(#inputs) #output {
                let api_fn = super::runtime::functions().#name;
                unsafe { api_fn(#(#call_arguments),*) }
            }
        }
    });
    let function_names = functions.iter().map(|function| {
        let name = function.sig.ident.to_string();
        quote!(#name)
    });
    let bindings = quote! {
        #(#declarations)*
        pub const REQUIRED_FUNCTIONS: &[&str] = &[#(#function_names),*];
        pub(crate) struct RuntimeFunctions {
            #(#field_declarations,)*
        }
        pub(crate) fn load_functions(library: &libloading::Library) -> Result<RuntimeFunctions, String> {
            Ok(RuntimeFunctions { #(#symbol_fields)* })
        }
        #(#wrappers)*
    };
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set")).join("runtime_bindings.rs");
    fs::write(output, bindings.to_string()).expect("write generated bindings");
}
