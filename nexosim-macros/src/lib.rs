use proc_macro::TokenStream;
use quote::quote;
use syn::{
    punctuated::Punctuated, token::PathSep, Expr, ExprPath, Ident, ImplItem, Path, PathSegment,
    Type,
};

#[proc_macro_attribute]
pub fn schedulable(attr: TokenStream, input: TokenStream) -> TokenStream {
    input
}

#[proc_macro_attribute]
pub fn methods(attr: TokenStream, input: TokenStream) -> TokenStream {
    let ast = syn::parse(input.clone()).expect("Model: Can't parse macro input!");
    let gen = impl_methods(&ast);
    let mut output = input;
    output.extend(gen);
    output
}

fn impl_methods(ast: &syn::ItemImpl) -> TokenStream {
    let name = &ast.self_ty;

    let name_ident = if let Type::Path(path) = &**name {
        path.path.get_ident().unwrap()
    } else {
        panic!();
    };

    // let init = ast
    //     .items
    //     .iter()
    //     .filter_map(|a| match a {
    //         ImplItem::Fn(f) => Some(f),
    //         _ => None,
    //     })
    //     .find(|f| f.attrs.iter().any(|a| a.meta.path().is_ident("init")))
    //     .map(|a| a.sig.ident.clone());

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
        impl nexosim::model::Schedulable for #name {
            fn register(&mut self, cx: &mut nexosim::model::BuildContext<impl nexosim::model::ProtoModel<Model = Self>>) {
                println!("Register");
                #(
                    cx.register_input(#reg_funcs);
                )*
            }


        }
    };
    for (i, func) in funcs.enumerate() {
        let fname = Ident::new(&format!("__{}", func.sig.ident), func.sig.ident.span());
        let vis = &func.vis;
        gen.extend(quote! {
            impl #name {
                  #vis fn #fname () -> usize
                  {
                      println!("generated {}", #i);
                      #i
                  }
            }
        });
    }
    gen.into()
}
