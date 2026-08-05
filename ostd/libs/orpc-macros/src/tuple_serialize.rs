// SPDX-License-Identifier: MPL-2.0
use proc_macro::TokenStream;
use quote::{quote, quote_spanned};
use syn::{Fields, ItemStruct, parse_macro_input, spanned::Spanned};

pub fn tuple_serialize_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as ItemStruct);

    let fields = match &input.fields {
        Fields::Named(fields) => &fields.named,
        Fields::Unnamed(_) | Fields::Unit => {
            let span = input.ident.span();
            let error = quote_spanned!(span =>
                compile_error!("Use serde::Serialize directly for tuple and unit structs.")
            );
            return error.into();
        }
    };

    let struct_name = &input.ident;
    let field_count = fields.len();
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let field_serializations = fields.iter().map(|field| {
        let field_name = &field.ident;
        quote_spanned! { field_name.span() =>
            ::serde::ser::SerializeTuple::serialize_element(&mut tup, &self.#field_name)?;
        }
    });

    let expanded = quote! {
        impl #impl_generics ::ostd::orpc::serialization::TupleSerialize for #struct_name #ty_generics #where_clause {}
        impl #impl_generics ::serde::Serialize for #struct_name #ty_generics #where_clause {
            fn serialize<S: ::serde::Serializer>(
                &self,
                serializer: S,
            ) -> ::core::result::Result<S::Ok, S::Error> {
                let mut tup = serializer.serialize_tuple(#field_count)?;
                #(#field_serializations)*
                ::serde::ser::SerializeTuple::end(tup)
            }
        }
    };

    expanded.into()
}
