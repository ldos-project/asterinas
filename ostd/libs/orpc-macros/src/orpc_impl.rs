// SPDX-License-Identifier: MPL-2.0
use quote::{ToTokens, quote, quote_spanned};
use syn::{Expr, Item, ItemImpl, Stmt, parse_quote, spanned::Spanned};

use crate::parsing_utils::{ORPCMethodKind, make_oqueues_field_name};

/// Implementation of the `orpc_impl` attribute macro.
pub fn orpc_impl_macro_impl(
    _attr: proc_macro::TokenStream,
    input: syn::ItemImpl,
) -> proc_macro2::TokenStream {
    // The implementations of the trait methods
    let mut method_implementations = Vec::new();
    let mut errors = Vec::new();

    for item in &input.items {
        match item {
            syn::ImplItem::Method(method) => {
                match ORPCMethodKind::of(&method.sig) {
                    Some(ORPCMethodKind::Orpc { .. }) => {
                        method_implementations.push(process_orpc_method(method));
                    }
                    Some(ORPCMethodKind::OQueue { .. }) => {
                        // We need the trait name, which requires that this impl be of a trait (not inherent). This this
                        // check fails. This macro will generate an error later in this function anyway, so dropping the
                        // member is fine.
                        if let Some((_, path, _)) = &input.trait_ {
                            process_oqueue_method(
                                &mut method_implementations,
                                &mut errors,
                                path,
                                method,
                            );
                        }
                    }
                    None => {
                        // No error here because this is guaranteed to create a normal error due to a mismatched method type.
                        method_implementations.push(method.to_token_stream());
                    }
                }
            }

            _ => errors.push(quote_spanned! { item.span() =>
                compile_error!("only methods are allowed")
            }),
        }
    }

    let trait_impl = {
        if let ItemImpl {
            attrs,
            defaultness,
            unsafety,
            impl_token,
            generics,
            trait_: Some((bang, trait_path, for_token)),
            self_ty,
            brace_token: _,
            items: _,
        } = &input
        {
            // Rebuild the impl using the new method implementations
            quote! {
                #(#attrs)*
                #defaultness #unsafety #impl_token #generics #bang #trait_path #for_token #self_ty {
                    #(#method_implementations)*
                }
            }
        } else {
            quote! {
                compile_error!("orpc_impl must apply to an impl of an ORPC trait (it is applied to an inherent impl)")
            }
        }
    };

    let output = quote! {
        #trait_impl

        #(#errors;)*
    };
    output
}

/// Generate the method implementation for an ORPC method.
fn process_orpc_method(method: &syn::ImplItemMethod) -> proc_macro2::TokenStream {
    let syn::ImplItemMethod {
        attrs,
        vis,
        defaultness,
        sig,
        block: body,
    } = method;

    // Don't check for illegal signatures. Those will already have been checked at the trait definition.

    let new_body = generate_orpc_method_body(sig, body);

    quote! {
        #(#attrs)*
        #vis
        #defaultness
        #sig {
            #new_body
        }
    }
}

/// Generate the body of an ORPC method implementation from the original body and signature. This is
/// used for both impls ([`process_orpc_method`]) and default implementations in traits
/// ([`crate::orpc_trait::process_orpc_method`]).
pub(crate) fn generate_orpc_method_body(
    sig: &syn::Signature,
    body: &syn::Block,
) -> proc_macro2::TokenStream {
    let ret_type = &sig.output;
    let base_ref: Expr = parse_quote! { ::ostd::orpc::framework::Server::orpc_server_base(self) };

    quote! {
        let body = move || #ret_type #body;
        #base_ref.call_in_context(body)
    }
}

/// Generate the method implementation for an OQueue access method. The method just extract the OQueueRef from inside
/// the servers `orpc_internal` struct.
fn process_oqueue_method(
    method_implementations: &mut Vec<proc_macro2::TokenStream>,
    errors: &mut Vec<proc_macro2::TokenStream>,
    trait_path: &syn::Path,
    method: &syn::ImplItemMethod,
) {
    let syn::ImplItemMethod {
        attrs,
        vis,
        defaultness,
        sig,
        block: body,
    } = method;

    if body.stmts.len() != 1
        || match body.stmts.first() {
            Some(Stmt::Item(Item::Verbatim(_))) => {
                // Ideally this would check for an actual `;` but it's hard and I can't see any case where anything else
                // would appear there without being a syntax error anyway.
                false
            }
            _ => true,
        }
    {
        errors.push(quote_spanned! { body.span() =>
            compile_error!("OQueue method definition should not provide a body. (They should look like their declaration in the trait.)")
        });
    }

    let oqueue_field_ident = make_oqueues_field_name(errors, trait_path);
    let ident = &sig.ident;

    // Don't check for illegal signatures. Those will already have been checked at the trait definition.

    let new_body = quote! {
        self.orpc_internal.#oqueue_field_ident
        .#ident
        .clone()
    };

    method_implementations.push(quote! {
        #(#attrs)*
        #vis
        #defaultness
        #sig {
            #new_body
        }
    });
}
