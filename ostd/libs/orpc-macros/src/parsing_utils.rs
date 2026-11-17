// SPDX-License-Identifier: MPL-2.0
use syn::{Generics, Token, Type, parse_quote, punctuated::Punctuated};

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
