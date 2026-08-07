// SPDX-License-Identifier: MPL-2.0

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{GenericParam, Generics, ItemStruct, parse_macro_input};

use crate::parsing_utils::generics_to_phantom;

/// Derive macro for the [`Element`] trait.
///
/// This macro generates an [`ElementDescriptor`] for types with lifetime parameters, or uses
/// [`ReflessElementDescriptor`] for types without lifetime parameters. These are used to provide
/// [`Element::Descriptor`]
///
/// # Examples
///
/// Type with one lifetime parameter:
/// ```ignore
/// #[derive(Element)]
/// struct ConnectCall<'a> {
///     self_: &'a StreamSocket,
///     socket_addr: &'a SocketAddr,
/// }
/// ```
///
/// Type without lifetime parameters:
/// ```ignore
/// #[derive(Element)]
/// struct Message {
///     id: u32,
///     payload: String,
/// }
/// ```
pub fn element_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as ItemStruct);
    let struct_ident = &input.ident;
    let vis = &input.vis;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Count lifetime parameters
    let lifetime_count = generics
        .params
        .iter()
        .filter(|param| matches!(param, GenericParam::Lifetime(_)))
        .count();

    // Generate code based on lifetime count
    let expanded = match lifetime_count {
        0 => {
            // No lifetimes: use ReflessElementDescriptor
            quote! {
                impl #impl_generics ::ostd::orpc::oqueue::Element for #struct_ident #ty_generics #where_clause {
                    type Descriptor = ::ostd::orpc::oqueue::ReflessElementDescriptor<#struct_ident #ty_generics>;
                }
            }
        }
        1 if generics
            .params
            .iter()
            .any(|p| matches!(p, GenericParam::Const(_))) =>
        {
            quote! {
                compile_error!("Const parameters are not supported with lifetime parameter. This is not a fundamental limitation and could be removed.");
            }
        }
        1 => {
            // One lifetime: generate a custom descriptor
            let descriptor_ident = format_ident!("{}Descriptor", struct_ident);

            // The generics without the lifetime parameter.
            let struct_generics_without_lifetime: Vec<GenericParam> = generics
                .params
                .iter()
                .filter(|param| !matches!(param, GenericParam::Lifetime(_)))
                .cloned()
                .collect();

            let struct_with_lifetime =
                quote! { #struct_ident <'a, #( #struct_generics_without_lifetime ),*> };

            // Create a Generics struct without the lifetime parameter for PhantomData
            let descriptor_generics = Generics {
                params: struct_generics_without_lifetime
                    .clone()
                    .into_iter()
                    .collect(),
                ..Default::default()
            };

            // Create a PhantomData for the descriptor's generic parameters (excluding lifetime)
            let phantom_data = generics_to_phantom(&descriptor_generics);

            quote! {
                #vis struct #descriptor_ident < #( #struct_generics_without_lifetime ),* > {
                    _phantom: #phantom_data,
                }

                impl < #( #struct_generics_without_lifetime: 'static ),* > ::ostd::orpc::oqueue::ElementDescriptor for #descriptor_ident < #( #struct_generics_without_lifetime ),* > #where_clause {
                    type Element<'a> = #struct_with_lifetime;
                }

                impl <'a, #( #struct_generics_without_lifetime: 'static ),*> ::ostd::orpc::oqueue::Element for #struct_with_lifetime #where_clause {
                    type Descriptor = #descriptor_ident < #( #struct_generics_without_lifetime ),* >;
                }
            }
        }
        _ => {
            // More than one lifetime: error
            quote! {
                compile_error!("Element derive macro only supports 0 or 1 lifetime parameter");
            }
        }
    };

    expanded.into()
}
