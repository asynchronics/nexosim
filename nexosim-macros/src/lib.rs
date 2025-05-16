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
pub fn init(attr: TokenStream, input: TokenStream) -> TokenStream {
    input
}

#[proc_macro_attribute]
pub fn model(attr: TokenStream, input: TokenStream) -> TokenStream {
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

    let init = ast
        .items
        .iter()
        .filter_map(|a| match a {
            ImplItem::Fn(f) => Some(f),
            _ => None,
        })
        .find(|f| f.attrs.iter().any(|a| a.meta.path().is_ident("init")))
        .map(|a| a.sig.ident.clone());

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

            fn register(&mut self, cx: &mut nexosim::model::BuildContext<impl nexosim::model::ProtoModel<Model = Self>>) {
                println!("Register");
                #(
                    cx.register_input(#reg_funcs);
                )*
            }

            fn init(self, cx: &mut nexosim::model::Context<Self>) -> impl std::future::Future<Output = nexosim::model::InitializedModel<Self>> + Send {
                println!("Init");
                // if let Some(_) = #init {
                //     println!("{:?}", ());
                // }
                // } else {
                //     async { self.into() }
                // }
                // async { self.into() }
                self.#init(cx)
            }

        }
    };
    for (i, func) in funcs.enumerate() {
        let fname = Ident::new(&format!("__{}", func.sig.ident), func.sig.ident.span());
        let vis = &func.vis;
        // let ty = func.sig;
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
