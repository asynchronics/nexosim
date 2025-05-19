use std::str::FromStr;

use proc_macro::{Span, TokenStream};
use quote::quote;
use syn::{
    punctuated::Punctuated,
    spanned::Spanned,
    token::{Paren, PathSep},
    Expr, ExprPath, FnArg, Ident, ImplItem, Path, PathSegment, Type, TypeTuple,
};

#[proc_macro_attribute]
pub fn schedulable(attr: TokenStream, input: TokenStream) -> TokenStream {
    input
}

#[proc_macro_attribute]
pub fn init(attr: TokenStream, input: TokenStream) -> TokenStream {
    input
}

#[proc_macro_attribute]
pub fn Model(attr: TokenStream, input: TokenStream) -> TokenStream {
    let ast = syn::parse(input.clone()).expect("Model: Can't parse macro input!");
    let attr = syn::parse(attr).expect("Model: Can't parse macro attributes!");
    let gen = impl_model(&ast, &attr);
    let mut output = input;
    output.extend(gen);
    output
}

fn impl_model(ast: &syn::ItemImpl, attr: &syn::Expr) -> TokenStream {
    let name = &ast.self_ty;

    let name_ident = if let Type::Path(path) = &**name {
        path.path.get_ident().unwrap()
    } else {
        panic!();
    };

    // Find custom init impl.
    let init = ast
        .items
        .iter()
        .filter_map(|a| match a {
            ImplItem::Fn(f) => Some(f),
            _ => None,
        })
        .find(|f| f.attrs.iter().any(|a| a.meta.path().is_ident("init")))
        .map(|a| a.sig.ident.clone());

    // Wrap init tokens into an Option.
    let init = init.and_then(|init| { quote! {
        fn init(self, cx: &mut nexosim::model::Context<Self>) -> impl std::future::Future<Output = nexosim::model::InitializedModel<Self>> + Send {
            self.#init(cx)
        }
    }.into()});

    let funcs = ast
        .items
        .iter()
        .filter_map(|a| match a {
            ImplItem::Fn(f) => Some(f),
            _ => None,
        })
        .filter(|f| {
            f.attrs
                .iter()
                .any(|a| a.meta.path().is_ident("schedulable"))
        });

    let reg_funcs = funcs.clone().map(|a| {
        let mut segments = Punctuated::new();
        segments.push_value(PathSegment {
            ident: name_ident.clone(),
            arguments: syn::PathArguments::None,
        });
        segments.push_punct(PathSep::default());
        segments.push_value(PathSegment {
            ident: a.sig.ident.clone(),
            arguments: syn::PathArguments::None,
        });
        Expr::Path(ExprPath {
            path: Path {
                leading_colon: None,
                segments,
            },
            attrs: Vec::new(),
            qself: None,
        })
    });

    let mut gen = quote! {
        impl Model for #name {
            type #attr;

            fn register_schedulables(&mut self, cx: &mut nexosim::model::BuildContext<impl nexosim::model::ProtoModel<Model = Self>>) -> nexosim::model::ModelRegistry {
                let mut registry = nexosim::model::ModelRegistry::default();
                #(
                    registry.add(cx.register_schedulable(#reg_funcs));
                )*
                registry
            }

            #init
        }
    };
    for (i, func) in funcs.enumerate() {
        let fname = Ident::new(&format!("__{}", func.sig.ident), func.sig.ident.span());
        let vis = &func.vis;

        // Find argument type token.
        let ty = func
            .sig
            .inputs
            .iter()
            .filter_map(|a| {
                if let FnArg::Typed(t) = a {
                    Some(t)
                } else {
                    None
                }
            })
            .map(|a| a.ty.clone())
            .next();

        let ty = match ty {
            Some(t) => t,
            // If no arg is provided, construct a unit type.
            None => Box::new(Type::Tuple(TypeTuple {
                paren_token: Paren(func.sig.span()),
                elems: Punctuated::new(),
            })),
        };

        gen.extend(quote! {
            impl #name {
                  #vis fn #fname () -> nexosim::model::RegistryId<Self, #ty>
                  {
                      nexosim::model::RegistryId::new(#i)
                  }
            }
        });
    }
    gen.into()
}
