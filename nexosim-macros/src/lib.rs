use proc_macro::TokenStream;
use quote::quote;

#[proc_macro_derive(Model, attributes(Env, schedulable))]
pub fn model_derive(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).expect("Model: Can't parse derive input!");
    impl_model(&ast)
}

fn impl_model(ast: &syn::DeriveInput) -> TokenStream {
    let name = &ast.ident;

    let env_attr = ast
        .attrs
        .iter()
        .find(|a| a.meta.path().is_ident("Env"))
        .expect("Env attribute is required!");

    let env_arg: syn::Type = env_attr.parse_args().unwrap();

    let syn::Data::Struct(data_struct) = &ast.data else {
        panic!("Components Derive: Not a data struct!")
    };
    let members_despawn = data_struct.fields.members();
    let members_entities = data_struct.fields.members();

    let gen = quote! {
        impl Model for #name {
            type Env = #env_arg;

            // fn entities_str(&self, component: &str) -> std::collections::HashSet<Entity> {
            //     match component {
            //         #(stringify!(#members_entities) => self.#members_entities.entities(),)*
            //         _ => std::collections::HashSet::new()
            //     }
            // }
        }
        impl #name {

        }
    };
    gen.into()
}
