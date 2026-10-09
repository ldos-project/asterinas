// SPDX-License-Identifier: MPL-2.0

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, Index, parse_macro_input, parse_quote};

pub fn derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = input.ident;
    let generics = input.generics;

    match input.data {
        Data::Struct(data) => expand_struct(&name, &generics, data.fields, input.vis),
        Data::Enum(_) | Data::Union(_) => Ok(expand_opaque(&name, &generics)),
    }
}

fn expand_opaque(name: &syn::Ident, generics: &syn::Generics) -> proc_macro2::TokenStream {
    let default_projection: syn::Path =
        parse_quote!(::ostd::orpc::oqueue::registry::DefaultProjection);
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();

    quote! {
        impl #impl_generics #default_projection for #name #type_generics #where_clause {
            type Output = usize;

            fn default_projection(&self) -> Self::Output {
                ::core::ptr::from_ref(self) as *const () as usize
            }
        }
    }
}

fn expand_struct(
    name: &syn::Ident,
    generics: &syn::Generics,
    fields: Fields,
    visibility: syn::Visibility,
) -> syn::Result<proc_macro2::TokenStream> {
    let default_projection: syn::Path =
        parse_quote!(::ostd::orpc::oqueue::registry::DefaultProjection);
    let field_projection: syn::Path = parse_quote!(::ostd::orpc::oqueue::registry::ProjectionField);
    let projection_name = format_ident!("{}DefaultProjection", name);
    let mut projection_generics = generics.clone();

    let mut projection_fields = Vec::new();
    let mut field_initializers = Vec::new();
    let mut serialization_fields = Vec::new();
    let mut field_types = Vec::new();

    let serialization = match &fields {
        Fields::Named(named) => {
            for field in &named.named {
                let ident = field.ident.as_ref().unwrap();
                let ty = &field.ty;
                field_types.push(ty.clone());
                projection_fields.push(quote! {
                    #ident: <#ty as #field_projection>::Output
                });
                field_initializers.push(quote! {
                    #ident: <#ty as #field_projection>::project_field(&self.#ident)
                });
                let key = ident.to_string();
                serialization_fields.push(quote! {
                    ::serde::ser::SerializeStruct::serialize_field(
                        &mut __state,
                        #key,
                        &self.#ident,
                    )?;
                });
            }
            let count = named.named.len();
            quote! {
                let mut __state = ::serde::Serializer::serialize_struct(
                    serializer,
                    stringify!(#projection_name),
                    #count,
                )?;
                #(#serialization_fields)*
                ::serde::ser::SerializeStruct::end(__state)
            }
        }
        Fields::Unnamed(unnamed) => {
            for (index, field) in unnamed.unnamed.iter().enumerate() {
                let member = Index::from(index);
                let field_name = format_ident!("field_{index}");
                let ty = &field.ty;
                field_types.push(ty.clone());
                projection_fields.push(quote! {
                    #field_name: <#ty as #field_projection>::Output
                });
                field_initializers.push(quote! {
                    #field_name: <#ty as #field_projection>::project_field(&self.#member)
                });
                serialization_fields.push(quote! {
                    ::serde::ser::SerializeTuple::serialize_element(
                        &mut __state,
                        &self.#field_name,
                    )?;
                });
            }
            let count = unnamed.unnamed.len();
            quote! {
                let mut __state = ::serde::Serializer::serialize_tuple(serializer, #count)?;
                #(#serialization_fields)*
                ::serde::ser::SerializeTuple::end(__state)
            }
        }
        Fields::Unit => quote! {
            ::serde::Serializer::serialize_unit_struct(
                serializer,
                stringify!(#projection_name),
            )
        },
    };

    for ty in &field_types {
        projection_generics
            .make_where_clause()
            .predicates
            .push(parse_quote!(#ty: #field_projection));
    }

    let (impl_generics, type_generics, where_clause) = projection_generics.split_for_impl();
    let projection_definition = if matches!(&fields, Fields::Unit) {
        quote!(#visibility struct #projection_name #projection_generics;)
    } else {
        quote! {
            #visibility struct #projection_name #projection_generics {
                #(#projection_fields,)*
            }
        }
    };
    let projection_construction = if matches!(&fields, Fields::Unit) {
        quote!(#projection_name)
    } else {
        quote!(#projection_name { #(#field_initializers,)* })
    };

    Ok(quote! {
        #[doc(hidden)]
        #projection_definition

        impl #impl_generics ::core::marker::Copy for #projection_name #type_generics #where_clause {}

        impl #impl_generics ::core::clone::Clone for #projection_name #type_generics #where_clause {
            fn clone(&self) -> Self {
                *self
            }
        }

        impl #impl_generics ::serde::Serialize for #projection_name #type_generics #where_clause {
            fn serialize<__Serializer>(
                &self,
                serializer: __Serializer,
            ) -> ::core::result::Result<__Serializer::Ok, __Serializer::Error>
            where
                __Serializer: ::serde::Serializer,
            {
                #serialization
            }
        }

        impl #impl_generics #default_projection for #name #type_generics #where_clause {
            type Output = #projection_name #type_generics;

            fn default_projection(&self) -> Self::Output {
                #projection_construction
            }
        }
    })
}
