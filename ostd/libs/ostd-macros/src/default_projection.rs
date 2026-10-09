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
        Data::Enum(data) => expand_enum(&name, &generics, data.variants, input.vis),
        // A union has no discriminant that identifies which field is initialized.
        Data::Union(_) => Ok(expand_opaque(&name, &generics)),
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
    let pod_projection_value: syn::Path =
        parse_quote!(::ostd::orpc::oqueue::registry::PodProjectionValue);
    let opaque_projection_value: syn::Path =
        parse_quote!(::ostd::orpc::oqueue::registry::OpaqueProjectionValue);
    let projection_name = format_ident!("{}DefaultProjection", name);
    let mut projection_generics = generics.clone();

    let mut projection_fields = Vec::new();
    let mut field_initializers = Vec::new();
    let mut serialization_fields = Vec::new();
    let serialization = match &fields {
        Fields::Named(named) => {
            for field in &named.named {
                let ident = field.ident.as_ref().unwrap();
                let (projected_ty, initializer) = project_field(
                    field,
                    quote!(&self.#ident),
                    &mut projection_generics,
                    &pod_projection_value,
                    &opaque_projection_value,
                )?;
                projection_fields.push(quote! {
                    #ident: #projected_ty
                });
                field_initializers.push(quote!(#ident: #initializer));
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
                let (projected_ty, initializer) = project_field(
                    field,
                    quote!(&self.#member),
                    &mut projection_generics,
                    &pod_projection_value,
                    &opaque_projection_value,
                )?;
                projection_fields.push(quote! {
                    #field_name: #projected_ty
                });
                field_initializers.push(quote!(#field_name: #initializer));
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

    let (impl_generics, type_generics, where_clause) = projection_generics.split_for_impl();
    let projection_definition = if matches!(&fields, Fields::Unit) {
        quote!(#visibility struct #projection_name #projection_generics #where_clause;)
    } else {
        quote! {
            #visibility struct #projection_name #projection_generics #where_clause {
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

fn expand_enum(
    name: &syn::Ident,
    generics: &syn::Generics,
    variants: syn::punctuated::Punctuated<syn::Variant, syn::Token![,]>,
    visibility: syn::Visibility,
) -> syn::Result<proc_macro2::TokenStream> {
    let default_projection: syn::Path =
        parse_quote!(::ostd::orpc::oqueue::registry::DefaultProjection);
    let pod_projection_value: syn::Path =
        parse_quote!(::ostd::orpc::oqueue::registry::PodProjectionValue);
    let opaque_projection_value: syn::Path =
        parse_quote!(::ostd::orpc::oqueue::registry::OpaqueProjectionValue);
    let projection_name = format_ident!("{}DefaultProjection", name);
    let mut projection_generics = generics.clone();
    let mut projection_variants = Vec::new();
    let mut match_arms = Vec::new();

    for variant in variants {
        let variant_name = variant.ident;
        match variant.fields {
            Fields::Unit => {
                projection_variants.push(quote!(#variant_name));
                match_arms.push(quote! {
                    Self::#variant_name => #projection_name::#variant_name
                });
            }
            Fields::Unnamed(fields) => {
                let mut projected_types = Vec::new();
                let mut bindings = Vec::new();
                let mut projected_fields = Vec::new();

                for (index, field) in fields.unnamed.iter().enumerate() {
                    let binding = format_ident!("__field_{index}");
                    let (projected_ty, initializer) = project_field(
                        field,
                        quote!(#binding),
                        &mut projection_generics,
                        &pod_projection_value,
                        &opaque_projection_value,
                    )?;
                    projected_types.push(projected_ty);
                    bindings.push(binding.clone());
                    projected_fields.push(initializer);
                }

                projection_variants.push(quote!(
                    #variant_name(#(#projected_types),*)
                ));
                match_arms.push(quote! {
                    Self::#variant_name(#(#bindings),*) => {
                        #projection_name::#variant_name(#(#projected_fields),*)
                    }
                });
            }
            Fields::Named(fields) => {
                let mut projected_fields = Vec::new();
                let mut bindings = Vec::new();
                let mut initializers = Vec::new();

                for field in fields.named {
                    let ident = field.ident.as_ref().unwrap();
                    let binding = format_ident!("__field_{}", ident);
                    let (projected_ty, initializer) = project_field(
                        &field,
                        quote!(#binding),
                        &mut projection_generics,
                        &pod_projection_value,
                        &opaque_projection_value,
                    )?;
                    projected_fields.push(quote! {
                        #ident: #projected_ty
                    });
                    bindings.push(quote!(#ident: #binding));
                    initializers.push(quote!(#ident: #initializer));
                }

                projection_variants.push(quote! {
                    #variant_name { #(#projected_fields,)* }
                });
                match_arms.push(quote! {
                    Self::#variant_name { #(#bindings,)* } => {
                        #projection_name::#variant_name { #(#initializers,)* }
                    }
                });
            }
        }
    }

    let (impl_generics, type_generics, where_clause) = projection_generics.split_for_impl();

    Ok(quote! {
        #[doc(hidden)]
        #[derive(::serde::Serialize)]
        #visibility enum #projection_name #projection_generics #where_clause {
            #(#projection_variants,)*
        }

        impl #impl_generics ::core::marker::Copy for #projection_name #type_generics #where_clause {}

        impl #impl_generics ::core::clone::Clone for #projection_name #type_generics #where_clause {
            fn clone(&self) -> Self {
                *self
            }
        }

        impl #impl_generics #default_projection for #name #type_generics #where_clause {
            type Output = #projection_name #type_generics;

            fn default_projection(&self) -> Self::Output {
                match self {
                    #(#match_arms,)*
                }
            }
        }
    })
}

fn project_field(
    field: &syn::Field,
    value: proc_macro2::TokenStream,
    generics: &mut syn::Generics,
    pod_projection_value: &syn::Path,
    opaque_projection_value: &syn::Path,
) -> syn::Result<(proc_macro2::TokenStream, proc_macro2::TokenStream)> {
    let mut projection_kind = None;
    for attr in field
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("projection"))
    {
        if projection_kind.is_some() {
            return Err(syn::Error::new_spanned(
                attr,
                "only one `projection` attribute is allowed per field",
            ));
        }
        let kind: syn::Ident = attr.parse_args()?;
        if kind != "pod" && kind != "opaque" {
            return Err(syn::Error::new_spanned(kind, "expected `pod` or `opaque`"));
        }
        projection_kind = Some(kind);
    }

    let ty = &field.ty;
    if projection_kind.as_ref().is_some_and(|kind| kind == "pod") {
        let pod: syn::Path = parse_quote!(::ostd::orpc::oqueue::registry::Pod);
        generics
            .make_where_clause()
            .predicates
            .push(parse_quote!(#ty: #pod + ::core::marker::Send + 'static));
        Ok((
            quote!(#pod_projection_value<#ty>),
            quote!(#pod_projection_value::new(*#value)),
        ))
    } else {
        Ok((
            quote!(#opaque_projection_value<#ty>),
            quote!(#opaque_projection_value::new(
                ::core::ptr::from_ref(#value) as *const () as usize
            )),
        ))
    }
}
