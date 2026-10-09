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
    let projection_name = format_ident!("{}DefaultProjection", name);
    let mut projection_bounds = generics.clone();

    let mut named_fields = Vec::new();
    let mut named_initializers = Vec::new();
    let mut tuple_fields = Vec::new();
    let mut tuple_initializers = Vec::new();
    let mut projected_types = Vec::new();

    match &fields {
        Fields::Named(fields) => {
            for field in &fields.named {
                let ident = field.ident.as_ref().unwrap();
                let (projected_ty, initializer) = project_field(
                    field,
                    quote!(&self.#ident),
                    &mut projection_bounds,
                    &default_projection,
                )?;
                let output_param = format_ident!("__ProjectionField{}", projected_types.len());
                projected_types.push(projected_ty);
                named_fields.push(quote!(#ident: #output_param));
                named_initializers.push(quote!(#ident: #initializer));
            }
        }
        Fields::Unnamed(fields) => {
            for (index, field) in fields.unnamed.iter().enumerate() {
                let member = Index::from(index);
                let (projected_ty, initializer) = project_field(
                    field,
                    quote!(&self.#member),
                    &mut projection_bounds,
                    &default_projection,
                )?;
                let output_param = format_ident!("__ProjectionField{}", projected_types.len());
                projected_types.push(projected_ty);
                tuple_fields.push(output_param);
                tuple_initializers.push(initializer);
            }
        }
        Fields::Unit => {}
    }

    let output_params = (0..projected_types.len())
        .map(|index| format_ident!("__ProjectionField{index}"))
        .collect::<Vec<_>>();
    let helper_generics = generic_arguments(&output_params);
    let output_type = generic_arguments(&projected_types);
    let (impl_generics, type_generics, where_clause) = projection_bounds.split_for_impl();

    let (projection_definition, projection_construction) = match &fields {
        Fields::Named(_) => (
            quote! {
                #visibility struct #projection_name #helper_generics {
                    #(#named_fields,)*
                }
            },
            quote!(#projection_name { #(#named_initializers,)* }),
        ),
        Fields::Unnamed(_) => (
            quote! {
                #visibility struct #projection_name #helper_generics (
                    #(#tuple_fields,)*
                );
            },
            quote!(#projection_name(#(#tuple_initializers,)*)),
        ),
        Fields::Unit => (
            quote!(#visibility struct #projection_name;),
            quote!(#projection_name),
        ),
    };
    let output_type = if projected_types.is_empty() {
        quote!(#projection_name)
    } else {
        quote!(#projection_name #output_type)
    };

    Ok(quote! {
        #[doc(hidden)]
        #[derive(Clone, Copy, ::serde::Serialize)]
        #projection_definition

        impl #impl_generics #default_projection for #name #type_generics #where_clause {
            type Output = #output_type;

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
    let projection_name = format_ident!("{}DefaultProjection", name);
    let mut projection_bounds = generics.clone();
    let mut projection_variants = Vec::new();
    let mut match_arms = Vec::new();
    let mut projected_types = Vec::new();

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
                let mut output_params = Vec::new();
                let mut bindings = Vec::new();
                let mut projected_fields = Vec::new();

                for (index, field) in fields.unnamed.iter().enumerate() {
                    let binding = format_ident!("__field_{index}");
                    let (projected_ty, initializer) = project_field(
                        field,
                        quote!(#binding),
                        &mut projection_bounds,
                        &default_projection,
                    )?;
                    let output_param = format_ident!("__ProjectionField{}", projected_types.len());
                    projected_types.push(projected_ty);
                    output_params.push(output_param);
                    bindings.push(binding);
                    projected_fields.push(initializer);
                }

                projection_variants.push(quote!(
                    #variant_name(#(#output_params),*)
                ));
                match_arms.push(quote! {
                    Self::#variant_name(#(#bindings),*) => {
                        #projection_name::#variant_name(#(#projected_fields),*)
                    }
                });
            }
            Fields::Named(fields) => {
                let mut output_fields = Vec::new();
                let mut bindings = Vec::new();
                let mut initializers = Vec::new();

                for field in fields.named {
                    let ident = field.ident.as_ref().unwrap();
                    let binding = format_ident!("__field_{}", ident);
                    let (projected_ty, initializer) = project_field(
                        &field,
                        quote!(#binding),
                        &mut projection_bounds,
                        &default_projection,
                    )?;
                    let output_param = format_ident!("__ProjectionField{}", projected_types.len());
                    projected_types.push(projected_ty);
                    output_fields.push(quote!(#ident: #output_param));
                    bindings.push(quote!(#ident: #binding));
                    initializers.push(quote!(#ident: #initializer));
                }

                projection_variants.push(quote! {
                    #variant_name { #(#output_fields,)* }
                });
                match_arms.push(quote! {
                    Self::#variant_name { #(#bindings,)* } => {
                        #projection_name::#variant_name { #(#initializers,)* }
                    }
                });
            }
        }
    }

    let output_params = (0..projected_types.len())
        .map(|index| format_ident!("__ProjectionField{index}"))
        .collect::<Vec<_>>();
    let helper_generics = generic_arguments(&output_params);
    let output_type = generic_arguments(&projected_types);
    let output_type = if projected_types.is_empty() {
        quote!(#projection_name)
    } else {
        quote!(#projection_name #output_type)
    };
    let (impl_generics, type_generics, where_clause) = projection_bounds.split_for_impl();

    Ok(quote! {
        #[doc(hidden)]
        #[derive(Clone, Copy, ::serde::Serialize)]
        #visibility enum #projection_name #helper_generics {
            #(#projection_variants,)*
        }

        impl #impl_generics #default_projection for #name #type_generics #where_clause {
            type Output = #output_type;

            fn default_projection(&self) -> Self::Output {
                match self {
                    #(#match_arms,)*
                }
            }
        }
    })
}

fn generic_arguments<T: quote::ToTokens>(arguments: &[T]) -> proc_macro2::TokenStream {
    if arguments.is_empty() {
        quote!()
    } else {
        quote!(<#(#arguments),*>)
    }
}

fn project_field(
    field: &syn::Field,
    value: proc_macro2::TokenStream,
    generics: &mut syn::Generics,
    default_projection: &syn::Path,
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
        if kind != "opaque" {
            return Err(syn::Error::new_spanned(kind, "expected `opaque`"));
        }
        projection_kind = Some(kind);
    }

    let ty = &field.ty;
    if projection_kind
        .as_ref()
        .is_some_and(|kind| kind == "opaque")
    {
        Ok((
            quote!(usize),
            quote!(::core::ptr::from_ref(#value) as *const () as usize),
        ))
    } else {
        generics
            .make_where_clause()
            .predicates
            .push(parse_quote!(#ty: #default_projection));
        generics.make_where_clause().predicates.push(parse_quote!(
            <#ty as #default_projection>::Output:
                ::core::marker::Copy + ::core::marker::Send + ::serde::Serialize + 'static
        ));
        Ok((
            quote!(<#ty as #default_projection>::Output),
            quote!(<#ty as #default_projection>::default_projection(#value)),
        ))
    }
}
