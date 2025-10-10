use syn::Path;

use quote::{format_ident, quote};
use syn::Type;

/// The kind of a method in an ORPC trait.
pub(crate) enum ORPCMethodKind<'a> {
    /// An normal RPC method. This returns some `Result<R, E>`.
    #[allow(unused)]
    ORPC { return_type: &'a Type },
    /// An accessor method for an OQueue. This returns some `OQueueRef<T>`.
    OQueue { return_type: &'a Type },
}

impl<'a> ORPCMethodKind<'a> {
    /// Extract all the required information from a signature.
    pub(crate) fn of(sig: &syn::Signature) -> Option<ORPCMethodKind> {
        let ret = &sig.output;
        match ret {
            syn::ReturnType::Type(_, typ) => match typ.as_ref() {
                syn::Type::Path(syn::TypePath { qself: None, path }) => {
                    match &path.segments.last() {
                        Some(syn::PathSegment {
                            ident,
                            arguments: _,
                        }) => {
                            let name = ident.to_string();
                            match name.as_str() {
                                "Result" => Some(ORPCMethodKind::ORPC { return_type: typ }),
                                "OQueueRef" => Some(ORPCMethodKind::OQueue { return_type: typ }),
                                _ => None,
                            }
                        }
                        None => None,
                    }
                }
                _ => None,
            },
            _ => None,
        }
    }
}

/// Construct the name of the field holding the OQueues associated with a trait. This field will be in the ORPCInternal
/// struct for a server.
/// 
/// NOTE: These fields break the snail_case requirement of Rust fields, so uses of this need to be marked to prevent the
/// warning. This is preferable to rewriting the name to match the style introduce more complexity. User code should
/// NEVER include this name.
pub(crate) fn make_oqueues_field_name(
    errors: &mut Vec<proc_macro2::TokenStream>,
    trait_ident: &Path,
) -> syn::Ident {
    if let Some(trait_ident) = trait_ident.segments.last() {
        let mut i = format_ident!("{}_oqueues", trait_ident.ident);
        i.set_span(trait_ident.ident.span());
        i
    } else {
        errors.push(quote! {
            compile_error!("INTERNAL ERROR: Missing trait name")
        });
        format_ident!("_fake")
    }
}
