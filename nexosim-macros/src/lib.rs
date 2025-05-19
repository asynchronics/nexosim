use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use syn::{
    punctuated::Punctuated,
    spanned::Spanned,
    token::{Paren, PathSep},
    Expr, ExprAssign, ExprPath, FnArg, Ident, ImplItem, ImplItemFn, Meta, Path, PathSegment, Type,
    TypeTuple,
};

#[allow(non_snake_case)]
#[proc_macro_attribute]
pub fn Model(attr: TokenStream, input: TokenStream) -> TokenStream {
    let mut ast = syn::parse(input.clone()).expect("Model: Can't parse macro input!");
    let env_expr = syn::parse(attr).ok();

    let added_tokens: TokenStream =
        impl_model(&mut ast, env_expr).unwrap_or_else(|e| e.to_compile_error().into());
    let mut output: TokenStream = ast.to_token_stream().into();
    output.extend(added_tokens);
    output
}

fn impl_model(ast: &mut syn::ItemImpl, env_expr: Option<Expr>) -> Result<TokenStream, syn::Error> {
    let name = &ast.self_ty;

    let name_ident = if let Type::Path(path) = &**name {
        path.path.get_ident().unwrap()
    } else {
        panic!();
    };

    let env_expr = match env_expr {
        Some(e) => e,
        None => {
            // TODO use better span ?
            let unit = Expr::Verbatim(
                TypeTuple {
                    paren_token: Paren(name.span()),
                    elems: Punctuated::new(),
                }
                .to_token_stream(),
            );
            Expr::Assign(ExprAssign {
                attrs: Vec::new(),
                left: Box::new(Expr::Verbatim(
                    Ident::new("Env", name.span()).to_token_stream(),
                )),
                eq_token: syn::Token![=](name.span()),
                right: Box::new(unit),
            })
        }
    };

    let mut init = None;
    let mut restore = None;
    let mut schedulables = Vec::new();

    for item in ast.items.iter_mut() {
        if let ImplItem::Fn(f) = item {
            if consume_method_attribute(f, "schedulable")? {
                schedulables.push(f.clone());
            }
            if consume_method_attribute(f, "init")? {
                init = Some(f.sig.ident.clone());
            }
            if consume_method_attribute(f, "restore")? {
                restore = Some(f.sig.ident.clone());
            }
        }
    }

    // Wrap init tokens into an Option.
    let init = init.and_then(|init| { quote! {
        fn init(self, cx: &mut nexosim::model::Context<Self>) -> impl std::future::Future<Output = nexosim::model::InitializedModel<Self>> + Send {
            self.#init(cx)
        }
    }.into()});

    // Wrap restore tokens into an Option.
    let restore = restore.and_then(|restore| { quote! {
        fn restore(self, cx: &mut nexosim::model::Context<Self>) -> impl std::future::Future<Output = nexosim::model::InitializedModel<Self>> + Send {
            self.#restore(cx)
        }
    }.into()});

    // Get MyModel::input method paths from scheduled inputs.
    let registered_methods = schedulables.iter().map(|a| {
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
        impl nexosim::model::Model for #name {
            type #env_expr;

            fn register_schedulables(&mut self, cx: &mut nexosim::model::BuildContext<impl nexosim::model::ProtoModel<Model = Self>>) -> nexosim::model::ModelRegistry {
                let mut registry = nexosim::model::ModelRegistry::default();
                #(
                    registry.add(cx.register_schedulable(#registered_methods));
                )*
                registry
            }

            #init

            #restore
        }
    };

    let mut hidden_methods = Vec::new();

    for (i, func) in schedulables.iter().enumerate() {
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

        // If no arg is provided, construct a unit type.
        let ty = match ty {
            Some(t) => t,
            None => Box::new(Type::Tuple(TypeTuple {
                paren_token: Paren(func.sig.span()),
                elems: Punctuated::new(),
            })),
        };

        hidden_methods.push(quote! {
            #vis fn #fname () -> nexosim::model::RegistryId<Self, #ty>
            {
              nexosim::model::RegistryId::new(#i)
            }
        });
    }

    // Write hidden methods block.
    gen.extend(quote! {
        impl #name {
            #( #hidden_methods )*
        }
    });

    Ok(gen.into())
}

// Check whether method has an attributte in the form of `nexosim(attr)`. If so
// remove it.
fn consume_method_attribute(f: &mut ImplItemFn, attr: &str) -> Result<bool, syn::Error> {
    let mut idx = None;
    for (i, a) in f.attrs.iter().enumerate() {
        if !a.meta.path().is_ident("nexosim") {
            continue;
        }
        if let Meta::List(meta) = &a.meta {
            let args: Expr = meta.parse_args().expect("Can't parse nexosim attribute!");
            if let Expr::Path(path) = args {
                if path.path.segments.len() != 1 {
                    return Err(syn::Error::new_spanned(
                        meta,
                        "attribute `nexosim` should have exactly one argument!",
                    ));
                    // panic!("Attribute `nexosim` should have exactly one
                    // argument!");
                }
                if path.path.segments[0].ident == attr {
                    idx = Some(i);
                }
            } else {
                return Err(syn::Error::new_spanned(
                    meta,
                    "invalid `nexosim` attribute!",
                ));
                // panic!("Invalid `nexosim` attribute!");
            }
        }
    }

    if let Some(idx) = idx {
        f.attrs.remove(idx);
        return Ok(true);
    }
    Ok(false)
}
