use quote::{ToTokens, format_ident, quote, quote_spanned};
use syn::{FnArg, ItemTrait, LitStr, Receiver, spanned::Spanned};

use crate::parsing_utils::ORPCMethodKind;

/// The implementation of the `orpc_trait` attr macro.
pub fn orpc_trait_macro_impl(
    _attr: proc_macro::TokenStream,
    input: syn::ItemTrait,
) -> proc_macro2::TokenStream {
    // The declarations of the fields in the OQueue struct
    let mut oqueue_declarations = Vec::new();
    // The field initializers for the above fields. This is used the in the implementation of `Default`.
    let mut oqueue_initializers = Vec::new();
    // Errors generated whereever. Each elements should be a `compile_error!` invocation.
    let mut errors = Vec::new();
    // The trait method declarations.
    let mut method_decls = Vec::new();

    let vis = &input.vis;

    for item in &input.items {
        match item {
            syn::TraitItem::Method(trait_item_method) => {
                match ORPCMethodKind::of(&trait_item_method.sig) {
                    Some(ORPCMethodKind::ORPC { return_type: _ }) => {
                        process_orpc_method(&mut method_decls, &mut errors, &trait_item_method);
                    }
                    Some(ORPCMethodKind::OQueue { return_type: typ }) => {
                        process_oqueue_method(
                            &mut oqueue_declarations,
                            &mut oqueue_initializers,
                            &mut method_decls,
                            &mut errors,
                            &trait_item_method,
                            typ,
                            vis,
                        );
                    }
                    None => {
                        errors.push(quote_spanned! { trait_item_method.sig.output.span() =>
                            compile_error!("ORPC methods must return either a Result or an OQueueRef")
                        });
                    }
                }
            }

            _ => errors.push(quote_spanned! { item.span() =>
                compile_error!("only methods are allowed")
            }),
        }
    }

    // Reconstruct the trait declaration with: Server added as a supertrait, updated method declarations.
    let trait_decl = {
        let ItemTrait {
            attrs,
            vis,
            unsafety,
            auto_token,
            trait_token,
            ident,
            generics,
            colon_token,
            supertraits,
            brace_token: _,
            items: _,
        } = &input;
        let colon_token = colon_token.unwrap_or_default();
        let plus_token = if supertraits.is_empty() {
            quote! {}
        } else {
            quote! { + }
        };
        quote! {
            #(#attrs)*
            #vis
            #unsafety
            #auto_token
            #trait_token
            #ident
            #generics
            #colon_token
            #supertraits #plus_token ::orpc::Server
            {
                #(#method_decls)*
            }
        }
    };

    // The type of the OQueue struct that holds references to the OQueues associated with this trait.
    let oqueues_struct_ident = format_ident!("{}OQueues", input.ident, span = input.ident.span());

    let oqueue_struct_docs = LitStr::new(
        &format!(
            "All the OQueue references associated with {}. This is used to build the ORPC internal data structures for server.",
            input.ident.to_string()
        ),
        input.span(),
    );

    let output = quote! {
        #trait_decl

        #[doc(hidden)]
        #[doc = #oqueue_struct_docs]
        #vis struct #oqueues_struct_ident {
            #(#oqueue_declarations),*
        }

        impl ::core::default::Default for #oqueues_struct_ident {
            fn default() -> Self {
                Self {
                    #(#oqueue_initializers),*
                }
            }
        }

        #(#errors;)*
    };

    output
}

/// Take an ORPC method declaration and process it for the trait declaration.
///
/// We don't actually generate anything for these. This method just checks and generates errors if the method match the
/// correct form.
fn process_orpc_method(
    method_decls: &mut Vec<proc_macro2::TokenStream>,
    errors: &mut Vec<proc_macro2::TokenStream>,
    trait_item_method: &syn::TraitItemMethod,
) {
    let mut bad_arg = false;
    if 1 > trait_item_method.sig.inputs.len() || trait_item_method.sig.inputs.len() > 2 {
        bad_arg = true;
    }
    match trait_item_method.sig.receiver() {
        // The only correct form: `&self`
        Some(FnArg::Receiver(Receiver {
            reference: Some(_),
            mutability: None,
            ..
        })) => {}
        _ => bad_arg = true,
    }
    if bad_arg {
        errors.push(quote_spanned! { trait_item_method.sig.inputs.span() =>
            compile_error!("ORPC trait methods must have the self argument and at most one other argument")
        });
    }
    method_decls.push(trait_item_method.to_token_stream());
}

/// Take an OQueue declaration on an ORPC trait and generate code to implement it.
///
/// This generates:
///
/// 1. The declaration of the oqueue ref field in the OQueue struct.
/// 2. The initializer for that field in `Default`.
/// 3. The updated method declaration in the trait.
fn process_oqueue_method(
    oqueue_declarations: &mut Vec<proc_macro2::TokenStream>,
    oqueue_initializers: &mut Vec<proc_macro2::TokenStream>,
    method_decls: &mut Vec<proc_macro2::TokenStream>,
    errors: &mut Vec<proc_macro2::TokenStream>,
    trait_item_method: &syn::TraitItemMethod,
    ret_type: &syn::Type,
    vis: &syn::Visibility,
) {
    // Check for the correct method signature.
    if trait_item_method.sig.inputs.len() != 1 {
        errors.push(quote_spanned! { trait_item_method.sig.inputs.span() =>
            compile_error!("OQueue method must have only a receiver")
        });
    }
    match trait_item_method.sig.receiver() {
        // The only correct form: `&self`
        Some(FnArg::Receiver(Receiver {
            reference: Some(_),
            mutability: None,
            ..
        })) => {}
        _ => {
            errors.push(quote_spanned! { trait_item_method.sig.inputs.span() =>
                compile_error!("OQueue method must have a simple receiver")
            });
        }
    }

    let ident = &trait_item_method.sig.ident;
    
    // Extract the docs to place on the field.
    let attrs: Vec<_> = trait_item_method
        .attrs
        .iter()
        .filter(|a| {
            a.path
                .get_ident()
                .is_some_and(|i| i.to_string() == "doc".to_string())
        })
        .collect();
    // Create the initializer for the field. In the error case, just use `todo!` and generate an error separately.
    if let Some(constr) = &trait_item_method.default {
        oqueue_initializers.push(quote! {
                #ident: #constr
        });
    } else {
        errors.push(quote_spanned! { trait_item_method.span() =>
            compile_error!("method body providing constructor is missing")
        });
        oqueue_initializers.push(quote! {
                #ident: todo!()
        });
    }

    oqueue_declarations.push(quote! {
        #(#attrs)*
        #vis #ident: #ret_type
    });

    // Copy over the method declaration while removing the "default" which would just be the initializer for the OQueue.
    let mut trait_item_method = trait_item_method.clone();
    trait_item_method.default = None;
    method_decls.push(trait_item_method.to_token_stream());
}
