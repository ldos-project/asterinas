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
            type Projection = usize;

            fn default_projection(&self) -> Self::Projection {
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
                projected_types.push(projected_ty.clone());
                named_fields.push(quote!(#ident: #projected_ty));
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
                projected_types.push(projected_ty.clone());
                tuple_fields.push(projected_ty);
                tuple_initializers.push(initializer);
            }
        }
        Fields::Unit => {}
    }

    let helper_generics = projection_generics(generics, &projection_bounds, &projected_types);
    let serde_bound = serialize_bounds(&projected_types);
    let (impl_generics, type_generics, where_clause) = projection_bounds.split_for_impl();
    let (helper_impl_generics, helper_type_generics, helper_where_clause) =
        helper_generics.split_for_impl();

    let (projection_definition, projection_construction) = match &fields {
        Fields::Named(_) => (
            quote! {
                #visibility struct #projection_name #helper_generics #helper_where_clause {
                    #(#named_fields,)*
                }
            },
            quote!(#projection_name { #(#named_initializers,)* }),
        ),
        Fields::Unnamed(_) => (
            quote! {
                #visibility struct #projection_name #helper_generics #helper_where_clause (
                    #(#tuple_fields,)*
                );
            },
            quote!(#projection_name(#(#tuple_initializers,)*)),
        ),
        Fields::Unit => (
            quote!(#visibility struct #projection_name #helper_generics #helper_where_clause;),
            quote!(#projection_name),
        ),
    };
    let output_type = quote!(#projection_name #helper_type_generics);

    Ok(quote! {
        #[doc(hidden)]
        #[derive(::serde::Serialize)]
        #[serde(bound(serialize = #serde_bound))]
        #projection_definition

        impl #helper_impl_generics ::core::clone::Clone for #projection_name #helper_type_generics #helper_where_clause {
            fn clone(&self) -> Self {
                *self
            }
        }

        impl #helper_impl_generics ::core::marker::Copy for #projection_name #helper_type_generics #helper_where_clause {}

        impl #impl_generics #default_projection for #name #type_generics #where_clause {
            type Projection = #output_type;

            fn default_projection(&self) -> Self::Projection {
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
                let mut bindings = Vec::new();
                let mut projected_field_types = Vec::new();
                let mut projected_field_values = Vec::new();

                for (index, field) in fields.unnamed.iter().enumerate() {
                    let binding = format_ident!("__field_{index}");
                    let (projected_ty, initializer) = project_field(
                        field,
                        quote!(#binding),
                        &mut projection_bounds,
                        &default_projection,
                    )?;
                    projected_types.push(projected_ty.clone());
                    projected_field_types.push(projected_ty);
                    bindings.push(binding);
                    projected_field_values.push(initializer);
                }

                projection_variants.push(quote!(
                    #variant_name(#(#projected_field_types),*)
                ));
                match_arms.push(quote! {
                    Self::#variant_name(#(#bindings),*) => {
                        #projection_name::#variant_name(#(#projected_field_values),*)
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
                        &mut projection_bounds,
                        &default_projection,
                    )?;
                    projected_types.push(projected_ty.clone());
                    projected_fields.push(quote!(#ident: #projected_ty));
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

    let helper_generics = projection_generics(generics, &projection_bounds, &projected_types);
    let serde_bound = serialize_bounds(&projected_types);
    let (impl_generics, type_generics, where_clause) = projection_bounds.split_for_impl();
    let (helper_impl_generics, helper_type_generics, helper_where_clause) =
        helper_generics.split_for_impl();
    let output_type = quote!(#projection_name #helper_type_generics);

    Ok(quote! {
        #[doc(hidden)]
        #[derive(::serde::Serialize)]
        #[serde(bound(serialize = #serde_bound))]
        #visibility enum #projection_name #helper_generics #helper_where_clause {
            #(#projection_variants,)*
        }

        impl #helper_impl_generics ::core::clone::Clone for #projection_name #helper_type_generics #helper_where_clause {
            fn clone(&self) -> Self {
                *self
            }
        }

        impl #helper_impl_generics ::core::marker::Copy for #projection_name #helper_type_generics #helper_where_clause {}

        impl #impl_generics #default_projection for #name #type_generics #where_clause {
            type Projection = #output_type;

            fn default_projection(&self) -> Self::Projection {
                match self {
                    #(#match_arms,)*
                }
            }
        }
    })
}

fn projection_generics(
    source: &syn::Generics,
    bounds: &syn::Generics,
    projected_types: &[proc_macro2::TokenStream],
) -> syn::Generics {
    use syn::{GenericParam, TypeParamBound};

    fn contains_ident(tokens: proc_macro2::TokenStream, ident: &syn::Ident) -> bool {
        tokens.into_iter().any(|token| match token {
            proc_macro2::TokenTree::Ident(token_ident) => token_ident == *ident,
            proc_macro2::TokenTree::Group(group) => contains_ident(group.stream(), ident),
            _ => false,
        })
    }

    let all_param_idents = source
        .params
        .iter()
        .map(|param| match param {
            GenericParam::Type(param) => param.ident.clone(),
            GenericParam::Lifetime(param) => param.lifetime.ident.clone(),
            GenericParam::Const(param) => param.ident.clone(),
        })
        .collect::<Vec<_>>();
    let used_param_idents = all_param_idents
        .iter()
        .filter(|ident| {
            projected_types
                .iter()
                .any(|ty| contains_ident(ty.clone(), ident))
        })
        .cloned()
        .collect::<Vec<_>>();

    let mut result = source.clone();
    result.params = source
        .params
        .iter()
        .filter(|param| {
            let ident = match param {
                GenericParam::Type(param) => &param.ident,
                GenericParam::Lifetime(param) => &param.lifetime.ident,
                GenericParam::Const(param) => &param.ident,
            };
            used_param_idents.contains(ident)
        })
        .cloned()
        .map(|mut param| {
            match &mut param {
                GenericParam::Type(param) => {
                    param.bounds = param
                        .bounds
                        .iter()
                        .filter(|bound| {
                            matches!(
                                bound,
                                TypeParamBound::Trait(bound)
                                    if matches!(
                                        bound.modifier,
                                        syn::TraitBoundModifier::Maybe(_)
                                    )
                                        && bound.path.is_ident("Sized")
                            )
                        })
                        .cloned()
                        .collect();
                    param.colon_token = (!param.bounds.is_empty()).then(Default::default);
                    param.eq_token = None;
                    param.default = None;
                }
                GenericParam::Lifetime(param) => param.bounds.clear(),
                GenericParam::Const(param) => {
                    param.eq_token = None;
                    param.default = None;
                }
            }
            param
        })
        .collect();

    let base_predicate_count = source
        .where_clause
        .as_ref()
        .map_or(0, |where_clause| where_clause.predicates.len());
    let source_predicates = source
        .where_clause
        .iter()
        .flat_map(|where_clause| &where_clause.predicates)
        .filter(|predicate| {
            let tokens = quote!(#predicate);
            all_param_idents.iter().all(|ident| {
                used_param_idents.contains(ident) || !contains_ident(tokens.clone(), ident)
            })
        });
    let projection_predicates = bounds
        .where_clause
        .iter()
        .flat_map(|where_clause| &where_clause.predicates)
        .skip(base_predicate_count);
    let predicates = source_predicates
        .chain(projection_predicates)
        .cloned()
        .collect::<syn::punctuated::Punctuated<_, syn::Token![,]>>();
    result.where_clause = if predicates.is_empty() {
        None
    } else {
        Some(syn::WhereClause {
            where_token: Default::default(),
            predicates,
        })
    };
    result
}

fn serialize_bounds(projected_types: &[proc_macro2::TokenStream]) -> syn::LitStr {
    let predicates = projected_types
        .iter()
        .map(|ty| format!("{}: ::serde::Serialize", quote!(#ty)))
        .collect::<Vec<_>>()
        .join(", ");
    syn::LitStr::new(&predicates, proc_macro2::Span::call_site())
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
            <#ty as #default_projection>::Projection:
                ::core::marker::Copy + ::core::marker::Send + ::serde::Serialize + 'static
        ));
        Ok((
            quote!(<#ty as #default_projection>::Projection),
            quote!(<#ty as #default_projection>::default_projection(#value)),
        ))
    }
}
