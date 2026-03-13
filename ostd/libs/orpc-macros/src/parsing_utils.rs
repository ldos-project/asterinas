// SPDX-License-Identifier: MPL-2.0
use quote::{format_ident, quote};
use syn::{Generics, Path, Token, Type, parse_quote, punctuated::Punctuated};

/// The kind of a method in an ORPC trait.
pub(crate) enum ORPCMethodKind<'a> {
    /// An normal RPC method. This returns some `Result<R, E>`.
    #[allow(unused)]
    Orpc { return_type: &'a Type },
    /// An accessor method for an OQueue. This returns some `OQueueRef<T>`.
    OQueue { return_type: &'a Type },
}

impl ORPCMethodKind<'_> {
    /// Extract all the required information from a signature.
    pub(crate) fn of(sig: &syn::Signature) -> Option<ORPCMethodKind> {
        let ret = &sig.output;
        if let syn::ReturnType::Type(_, typ) = ret {
            if let syn::Type::Path(syn::TypePath { qself: None, path }) = typ.as_ref() {
                let path_segment = &path.segments.last()?;
                let name = path_segment.ident.to_string();
                return match name.as_str() {
                    "Result" => Some(ORPCMethodKind::Orpc { return_type: typ }),
                    "OQueueRef" => Some(ORPCMethodKind::OQueue { return_type: typ }),
                    _ => None,
                };
            }
        }
        None
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

/// Creates a [`PhantomData`] based on `Self`.
///
/// The `PhantomData` will be `Send` and `Sync`.
///
/// The resulting `Type` will look something like:
///
/// ```
/// # use std::marker::PhantomData;
/// # fn demo<'a, 'b, T, U>() {
/// # let _phantom:
/// PhantomData<(fn(&'a (), &'b ()) -> (T, U))>
/// # ;
/// # }
/// ```
///
/// [`PhantomData`]: core::marker::PhantomData
///
/// Copied from the [`to_phantom` crate](https://github.com/MrGVSV/to_phantom) under the MIT
/// license. It was copied here due to syn version conflicts.
pub(crate) fn generics_to_phantom(generics: &Generics) -> Type {
    let lifetimes = generics
        .lifetimes()
        .map(|param| &param.lifetime)
        .collect::<Vec<_>>();

    let types = generics
        .type_params()
        .map(|param| &param.ident)
        .collect::<Punctuated<_, Token![,]>>();

    parse_quote!(::core::marker::PhantomData<fn(#(&#lifetimes ()),*) -> (#types)>)
}
