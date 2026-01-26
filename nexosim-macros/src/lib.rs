use proc_macro::TokenStream;
use quote::{ToTokens, quote, quote_token};
use syn::{
    Expr, ExprPath, FnArg, Generics, Ident, ImplItem, ImplItemFn, ItemType, Meta, Path,
    PathArguments, PathSegment, Signature, Token, Type, TypeTuple,
    punctuated::Punctuated,
    spanned::Spanned,
    token::{Paren, PathSep},
};

const INIT_ATTR: &str = "init";
const SCHEDULABLE_ATTR: &str = "schedulable";
const AVAILABLE_ATTRS: &[&str] = &[INIT_ATTR, SCHEDULABLE_ATTR];

macro_rules! handle_parse_result {
    ($call:expr) => {
        match $call {
            Ok(data) => data,
            Err(err) => return syn::__private::TokenStream::from(err.to_compile_error()),
        }
    };
}

#[proc_macro_attribute]
pub fn __erase(_: TokenStream, _: TokenStream) -> TokenStream {
    <_>::default()
}

/// A helper macro that enables schema generation for the server endpoint
/// data.
#[proc_macro_derive(Message)]
pub fn message_derive(input: TokenStream) -> TokenStream {
    [
        stringify!(
            #[
                ::core::prelude::v1::derive(
                    ::nexosim::JsonSchema
                )
            ]
            #[schemars(crate = "nexosim::schemars")]
            #[::nexosim::nexosim_macros::__erase]
        ),
        &input.to_string(),
    ]
    .concat()
    .parse()
    .unwrap()
}

#[proc_macro]
pub fn schedulable(input: TokenStream) -> TokenStream {
    let ast = handle_parse_result!(syn::parse(input));
    impl_schedulable(&ast).unwrap_or_else(|e| e.to_compile_error().into())
}

fn impl_schedulable(ast: &Path) -> Result<TokenStream, syn::Error> {
    if ast.segments.len() != 2 {
        return Err(syn::Error::new_spanned(
            ast,
            "invalid associated method path",
        ));
    }

    let ty = ast.segments[0].clone();
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
        "method `{err_name}` is not a valid schedulable input for the model! Perhaps you forgot to include the #[nexosim(schedulable)] attribute or are using a method from another model."
    );

    let is_generic = matches!(ty.arguments, PathArguments::AngleBracketed(_));

    // Generic types cannot be used in a const context. Therefore we are not
    // able to use our custom error message.
    let tokens = if !is_generic {
        quote! {
            {
                // Call a hidden method in the array type definition to cast a
                // custom error during a type-checking compilation phase.
                let _: [(); { if !#ty::____is_schedulable(stringify!(#hidden_name)) {
                    panic!(#err_msg)
                }; 0} ] = [];
                &#path
            }
        }
    } else {
        quote! {&#path}
    };
    Ok(tokens.into())
}

#[allow(non_snake_case)]
#[proc_macro_attribute]
pub fn Model(attr: TokenStream, input: TokenStream) -> TokenStream {
    let mut ast: syn::ItemImpl = handle_parse_result!(syn::parse(input.clone()));
    let env = handle_parse_result!(parse_env(attr));
    let added_tokens = handle_parse_result!(impl_model(&mut ast, env));

    let mut output: TokenStream = ast.to_token_stream().into();
    output.extend(added_tokens);
    output
}

fn impl_model(ast: &mut syn::ItemImpl, env: ItemType) -> Result<TokenStream, syn::Error> {
    let name = &ast.self_ty;

    let (init, schedulables) = parse_tagged_methods(&mut ast.items)?;

    let registered_methods = get_registered_method_paths(&schedulables);
    let mut tokens = get_impl_model_trait(name, &env, &ast.generics, init, registered_methods);
    let hidden_methods = get_hidden_method_impls(&schedulables);

    // We do not use ty_generics as they're already present in `name`
    let (impl_generics, _, where_clause) = ast.generics.split_for_impl();

    // Write hidden methods block.
    tokens.extend(quote! {
        impl #impl_generics #name #where_clause {
            #( #hidden_methods )*
        }
    });

    Ok(tokens.into())
}

/// Checks whether Env type is provided by the user.
/// If not uses `()` as a default.
fn parse_env(tokens: TokenStream) -> Result<ItemType, syn::Error> {
    if tokens.is_empty() {
        // No tokens found -> generate `type Env=();`.
        let span = proc_macro2::Span::call_site();
        return Ok(ItemType {
            attrs: vec![],
            vis: syn::Visibility::Inherited,
            type_token: Token![type](span),
            ident: Ident::new("Env", span),
            generics: Generics::default(),
            eq_token: Token![=](span),
            ty: Box::new(Type::Tuple(TypeTuple {
                paren_token: Paren(span),
                elems: Punctuated::new(),
            })),
            semi_token: Token![;](span),
        });
    }

    // Append semicolon at the end of the found token stream.
    let mut with_semicolon = tokens.clone().into();
    quote_token!(; with_semicolon);
    syn::parse(with_semicolon.into())
}

/// Get MyModel::input method paths from scheduled inputs.
fn get_registered_method_paths<'a>(
    schedulables: &'a [ImplItemFn],
) -> impl Iterator<Item = Expr> + use<'a> {
    schedulables.iter().map(|a| {
        let mut segments = Punctuated::new();
        segments.push_value(PathSegment {
            ident: Ident::new("Self", a.span()),
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

/// Finds methods tagged as `init` or `schedulable`.
/// Clears found tags from the original token stream.
#[allow(clippy::type_complexity)]
fn parse_tagged_methods(
    items: &mut [ImplItem],
) -> Result<(Option<proc_macro2::TokenStream>, Vec<ImplItemFn>), syn::Error> {
    let mut init = None;
    let mut schedulables = Vec::new();

    // Find tagged methods.
    for item in items.iter_mut() {
        if let ImplItem::Fn(f) = item {
            let attrs = collect_nexosim_attributes(f)?;
            if attrs.contains(&SCHEDULABLE_ATTR) {
                schedulables.push(f.clone());
            }
            if attrs.contains(&INIT_ATTR) {
                init = Some(fn_with_optional_cx_env(&f.sig)?);
            }
        }
    }

    // Wrap init tokens into an Option for conditional rendering.
    let init = init.and_then(|init| {
        quote! {
            fn init(
                self, cx: &nexosim::model::Context<Self>, env: &mut Self::Env,
            ) -> impl std::future::Future<Output = nexosim::model::InitializedModel<Self>> + Send {
                #init
            }
        }
        .into()
    });

    Ok((init, schedulables))
}

/// Renders the impl Model for MyModel block.
fn get_impl_model_trait(
    name: &Type,
    env: &ItemType,
    generics: &Generics,
    init: Option<proc_macro2::TokenStream>,
    registered_methods: impl Iterator<Item = Expr>,
) -> proc_macro2::TokenStream {
    // We do not use ty_generics as they're already present in `name`
    let (impl_generics, _, where_clause) = generics.split_for_impl();

    quote! {
        #[automatically_derived]
        impl #impl_generics nexosim::model::Model for #name #where_clause {
            #env

            fn register_schedulables(
                cx: &mut nexosim::model::BuildContext<impl nexosim::model::ProtoModel<Model = Self>>
            ) -> nexosim::model::ModelRegistry {
                let mut registry = nexosim::model::ModelRegistry::default();
                #(
                    registry.add(cx.register_schedulable(#registered_methods));
                )*
                registry
            }

            #init
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

fn collect_nexosim_attributes(f: &mut ImplItemFn) -> Result<Vec<&'static str>, syn::Error> {
    let mut attrs = Vec::new();
    let mut indices = Vec::new();

    'outer: for (i, attr) in f.attrs.iter().enumerate() {
        if !attr.meta.path().is_ident("nexosim") {
            continue;
        }
        indices.push(i);

        match &attr.meta {
            Meta::List(meta) => {
                if let Ok(Expr::Path(path)) = meta.parse_args::<Expr>()
                    && let Some(segment) = path.path.segments.first()
                {
                    for attr in AVAILABLE_ATTRS {
                        if segment.ident == attr {
                            attrs.push(*attr);
                            continue 'outer;
                        }
                    }
                }

                if meta.tokens.clone().into_iter().count() > 1 {
                    return Err(syn::Error::new_spanned(
                        meta,
                        "attribute `nexosim` should have exactly one argument!",
                    ));
                }
                return Err(syn::Error::new_spanned(
                    meta,
                    "invalid `nexosim` attribute!",
                ));
            }
            _ => {
                return Err(syn::Error::new_spanned(
                    &attr.meta,
                    "invalid `nexosim` attribute!",
                ));
            }
        }
    }

    for i in indices.iter().rev() {
        f.attrs.remove(*i);
    }

    Ok(attrs)
}

fn fn_with_optional_cx_env(sig: &Signature) -> Result<proc_macro2::TokenStream, syn::Error> {
    let ident = sig.ident.clone();
    match sig.inputs.len() {
        1 => Ok(quote!({ self.#ident() })),
        2 => Ok(quote!({ self.#ident(cx) })),
        3 => Ok(quote!({ self.#ident(cx, env) })),
        _ => Err(syn::Error::new_spanned(sig, "invalid number of arguments")),
    }
}
