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
                    Some(ORPCMethodKind::ORPC { return_type: _ }) => {
                        process_orpc_method(&mut method_implementations, &mut errors, method);
                    }
                    Some(ORPCMethodKind::OQueue { return_type: _ }) => {
                        // We need the trait name, which requires that this impl be of a trait (not inherent). This this
                        // check fails. This macro will generate an error later in this function anyway, so dropping the
                        // member is fine.
                        if let Some((_, path, _)) = &input.trait_ {
                            process_oqueue_method(&mut method_implementations, &mut errors, path, method);
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
            quote_spanned! { input.span() =>
                #(#attrs)*
                #defaultness #unsafety #impl_token #generics #bang #trait_path #for_token #self_ty {
                    #(#method_implementations)*
                }
            }
        } else {
            quote_spanned! { input.span() =>
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
fn process_orpc_method(
    method_implementations: &mut Vec<proc_macro2::TokenStream>,
    _errors: &mut Vec<proc_macro2::TokenStream>,
    method: &syn::ImplItemMethod,
) {
    let syn::ImplItemMethod {
        attrs,
        vis,
        defaultness,
        sig,
        block: body,
    } = method;

    let ret_type = &sig.output;

    // Don't check for illegal signatures. Those will already have been checked at the trait definition.

    let base_ref: Expr = parse_quote! { ::orpc::orpc_impl::Server::orpc_server_base(self) };
    let abort_check = quote_spanned! { sig.span() =>
        if #base_ref.is_aborted() {
            return ::core::result::Result::Err(::orpc::orpc_impl::errors::RPCError::ServerMissing.into());
        }
    };
    let enter_server_context = quote_spanned! { sig.span() =>
            let _server_context = ::orpc::orpc_impl::framework::CurrentServer::enter_server_context(
                self
            );
    };
    let error_cases = quote! {
        {
            Ok(ret) => ret,
            Err(payload) => {
                let e = ::orpc::orpc_impl::errors::RPCError::from_panic(payload);
                #base_ref.abort(&e);
                Err(e.into())
            }
        }
    };

    let new_body = quote_spanned! { body.span() =>
        #abort_check

        match ::std::panic::catch_unwind(|| {
            #enter_server_context
            (|| #ret_type #body)()
        }) #error_cases

    };

    method_implementations.push(quote_spanned! { method.span() =>
        #(#attrs)*
        #vis
        #defaultness
        #sig {
            #new_body
        }
    });
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

    let new_body = quote_spanned! { ident.span() =>
        self.orpc_internal.#oqueue_field_ident
        .#ident
        .clone()
    };

    method_implementations.push(quote_spanned! { method.span() =>
        #(#attrs)*
        #vis
        #defaultness
        #sig {
            #new_body
        }
    });
}
