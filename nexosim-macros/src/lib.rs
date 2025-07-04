use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn __erase(_: TokenStream, _: TokenStream) -> TokenStream {
    <_>::default()
}

#[proc_macro_derive(Message)]
pub fn event_derive(input: TokenStream) -> TokenStream {
    [
        stringify!(
            #[
                ::core::prelude::v1::derive(
                    ::nexosim::Schema
                )
            ]
            #[::nexosim_macros::__erase]
        ),
        &input.to_string(),
    ]
    .concat()
    .parse()
    .unwrap()
}
