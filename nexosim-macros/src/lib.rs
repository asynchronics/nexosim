use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use syn::{
    punctuated::Punctuated,
    spanned::Spanned,
    token::{Paren, PathSep},
    Expr, ExprAssign, ExprPath, FnArg, Ident, ImplItem, ImplItemFn, Meta, Path, PathSegment, Type,
    TypeTuple,
};

#[proc_macro]
pub fn schedulable(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).unwrap();
    impl_schedulable(&ast).unwrap_or_else(|e| e.to_compile_error().into())
}

fn impl_schedulable(ast: &Path) -> Result<TokenStream, syn::Error> {
    if ast.segments.len() != 2 {
        return Err(syn::Error::new_spanned(
            ast,
            "invalid associated method path",
        ));
    }
    let hidden_name = Ident::new(&format!("__{}", ast.segments[1].ident), ast.span());

    let mut segments = ast.segments.clone();
    segments[1].ident = hidden_name.clone();
    let path = Path {
        leading_colon: None,
        segments,
    };

    let err_name = ast.segments[1].ident.to_string();
    // Argument formatting not possible in the const context as of Rust >= 1.87
    let err_msg = format!(
        "method `{}` is not a valid schedulable input for the model! Perhaps you forgot to include the #[nexosim(schedulable)] attribute or are using a method from another model.",
        err_name
    );

    let gen = quote! {
        {
            // Call a hidden method in the array type definition
            // to cast a custom error during a type-checking compilation phase.
            let _: [(); { if !Self::____is_schedulable(stringify!(#hidden_name)) {
                panic!(#err_msg)
            }; 0} ] = [];
            &#path
        }
    };
    Ok(gen.into())
}

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
        return Err(syn::Error::new_spanned(
            name,
            "invalid impl. block name identifier",
        ));
    };

    let env_expr = parse_env_expr(env_expr);

    let (init, restore, schedulables) = parse_tagged_methods(&mut ast.items)?;

    let registered_methods = get_registered_method_paths(name_ident, &schedulables);
    let mut gen = get_impl_model_trait(name, &env_expr, init, restore, registered_methods);
    let hidden_methods = get_hidden_method_impls(&schedulables);

    // Write hidden methods block.
    gen.extend(quote! {
        impl #name {
            #( #hidden_methods )*
        }
    });

    Ok(gen.into())
}

/// Checks whether Env type is provided by the user.
/// If not uses `()` as a default.
fn parse_env_expr(expr: Option<Expr>) -> Expr {
    match expr {
        Some(e) => e,
        None => {
            let unit = Expr::Verbatim(
                TypeTuple {
                    paren_token: Paren(expr.span()),
                    elems: Punctuated::new(),
                }
                .to_token_stream(),
            );
            Expr::Assign(ExprAssign {
                attrs: Vec::new(),
                left: Box::new(Expr::Verbatim(
                    Ident::new("Env", expr.span()).to_token_stream(),
                )),
                eq_token: syn::Token![=](expr.span()),
                right: Box::new(unit),
            })
        }
    }
}

/// Get MyModel::input method paths from scheduled inputs.
fn get_registered_method_paths<'a>(
    name_ident: &'a Ident,
    schedulables: &'a [ImplItemFn],
) -> impl Iterator<Item = Expr> + use<'a> {
    schedulables.iter().map(|a| {
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
    })
}

/// Finds methods tagged as `init`, `restore` or `schedulable`.
/// Clears found tags from the original token stream.
fn parse_tagged_methods(
    items: &mut [ImplItem],
) -> Result<
    (
        Option<proc_macro2::TokenStream>,
        Option<proc_macro2::TokenStream>,
        Vec<ImplItemFn>,
    ),
    syn::Error,
> {
    let mut init = None;
    let mut restore = None;
    let mut schedulables = Vec::new();

    // Find tagged methods.
    for item in items.iter_mut() {
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

    // Wrap init and restore tokens into Options for conditional rendering.
    let init = init.and_then(|init| {
        quote! {
            fn init(
                self, cx: &mut nexosim::model::Context<Self>
            ) -> impl std::future::Future<Output = nexosim::model::InitializedModel<Self>> + Send {
                self.#init(cx)
            }
        }
        .into()
    });

    let restore = restore.and_then(|restore| {
        quote! {
            fn restore(
                self, cx: &mut nexosim::model::Context<Self>
            ) -> impl std::future::Future<Output = nexosim::model::InitializedModel<Self>> + Send {
                self.#restore(cx)
            }
        }
        .into()
    });

    Ok((init, restore, schedulables))
}

/// Renders the impl Model for MyModel block.
fn get_impl_model_trait(
    name: &Type,
    env: &Expr,
    init: Option<proc_macro2::TokenStream>,
    restore: Option<proc_macro2::TokenStream>,
    registered_methods: impl Iterator<Item = Expr>,
) -> proc_macro2::TokenStream {
    quote! {
        impl nexosim::model::Model for #name {
            type #env;

            fn register_schedulables(
                &mut self, cx: &mut nexosim::model::BuildContext<impl nexosim::model::ProtoModel<Model = Self>>
            ) -> nexosim::model::ModelRegistry {
                let mut registry = nexosim::model::ModelRegistry::default();
                #(
                    registry.add(cx.register_schedulable(#registered_methods));
                )*
                registry
            }

            #init

            #restore
        }
    }
}

/// Renders MyModel::__input associated consts.
fn get_hidden_method_impls(schedulables: &[ImplItemFn]) -> Vec<proc_macro2::TokenStream> {
    let mut hidden_methods = Vec::new();
    let mut registered_schedulables = Vec::new();

    for (i, func) in schedulables.iter().enumerate() {
        let fname = Ident::new(&format!("__{}", func.sig.ident), func.sig.ident.span());

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
            #[doc(hidden)]
            #[allow(non_upper_case_globals)]
            const #fname: nexosim::model::SchedulableId<Self, #ty> = nexosim::model::SchedulableId::__from_decorated(#i);
        });
        registered_schedulables.push(fname);
    }

    let byte_literals = registered_schedulables
        .iter()
        .map(|a| proc_macro2::Literal::byte_string(a.to_string().as_bytes()));

    // Add a hidden method used for producing more meaningful compilation errors,
    // when a user tries to schedule an undecorated method.
    hidden_methods.push(quote! {
        #[doc(hidden)]
        const fn ____is_schedulable(fname: &'static str) -> bool {
            match fname.as_bytes() {
                #(#byte_literals => true,)*
                _ => false
            }
        }
    });

    hidden_methods
}

/// Check whether method has an attributte in the form of `nexosim(attr)`. If so
/// remove it.
fn consume_method_attribute(f: &mut ImplItemFn, attr: &str) -> Result<bool, syn::Error> {
    let mut idx = None;
    for (i, a) in f.attrs.iter().enumerate() {
        if !a.meta.path().is_ident("nexosim") {
            continue;
        }
        if let Meta::List(meta) = &a.meta {
            let args: Expr = meta
                .parse_args()
                .map_err(|_| syn::Error::new_spanned(meta, "Can't parse nexosim attribute!"))?;
            if let Expr::Path(path) = args {
                if path.path.segments.len() != 1 {
                    return Err(syn::Error::new_spanned(
                        meta,
                        "attribute `nexosim` should have exactly one argument!",
                    ));
                }
                if path.path.segments[0].ident == attr {
                    idx = Some(i);
                }
            } else {
                return Err(syn::Error::new_spanned(
                    meta,
                    "invalid `nexosim` attribute!",
                ));
            }
        }
    }

    if let Some(idx) = idx {
        f.attrs.remove(idx);
        return Ok(true);
    }
    Ok(false)
}
