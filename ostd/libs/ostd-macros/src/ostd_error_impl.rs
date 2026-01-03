// SPDX-License-Identifier: MPL-2.0

use proc_macro::TokenStream;
use quote::{quote, quote_spanned};
use syn::{
    Attribute, Fields, Ident, Item, ItemEnum, Variant, parenthesized, parse::Parse,
    spanned::Spanned as _,
};

pub(crate) fn ostd_error_impl(input: Item) -> TokenStream {
    let ostd_error_trait_impl = match generate_ostd_error_trait_impl(&input) {
        Ok(v) => v,
        Err(e) => e.into_compile_error(),
    };

    let expanded = match input {
        Item::Enum(ItemEnum {
            attrs,
            vis,
            enum_token,
            ident,
            generics,
            variants,
            ..
        }) => {
            let new_variants = variants
                .into_iter()
                .map(
                    |Variant {
                         attrs,
                         ident,
                         fields,
                         discriminant,
                     }|
                     -> Result<proc_macro2::TokenStream, syn::Error> {
                        let (context_handling, attrs) = process_attrs(attrs)?;
                        let new_fields = ostd_error_process_fields(&fields, context_handling);
                        let discriminant = discriminant.as_ref().map(|(eq, v)| {
                            quote! { #eq #v }
                        });
                        Ok(quote! {
                                #(#attrs)*
                                #ident #new_fields #discriminant
                        })
                    },
                )
                .collect::<Result<Vec<_>, _>>();
            match new_variants {
                Ok(new_variants) => {
                    quote! {
                        #(#attrs)*
                        #vis
                        #enum_token #ident #generics {
                            #(#new_variants),*
                        }
                    }
                }
                Err(error) => error.into_compile_error(),
            }
        }
        Item::Struct(syn::ItemStruct {
            attrs,
            vis,
            struct_token,
            ident,
            generics,
            fields,
            semi_token,
        }) => match process_attrs(attrs) {
            Ok((context_handling, attrs)) => {
                let new_fields = ostd_error_process_fields(&fields, context_handling);
                quote! {
                        #(#attrs)*
                        #vis
                        #struct_token
                        #ident #generics #new_fields #semi_token
                }
            }
            Err(e) => e.into_compile_error(),
        },
        _ => quote! { compile_error!("ostd_error can only be applied to enums or structs") },
    };

    quote! {
        #expanded
        #ostd_error_trait_impl
    }
    .into()
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum VariantContextHandling {
    Inline,
    Boxed,
    Source,
    None,
}

/// A syn-parsable simple argument to a attr.
///
/// ```ignore
/// ident(ident)
/// ```
pub(crate) struct SimpleAttrArgument {
    pub prefix: Ident,
    #[expect(unused)]
    pub paren_token: syn::token::Paren,
    pub argument: Ident,
}

impl Parse for SimpleAttrArgument {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let content;
        Ok(SimpleAttrArgument {
            prefix: input.parse()?,
            paren_token: parenthesized!(content in input),
            argument: content.parse()?,
        })
    }
}

pub(crate) fn process_attrs(
    attrs: Vec<Attribute>,
) -> Result<(VariantContextHandling, Vec<Attribute>), syn::Error> {
    let mut ret_attrs = Vec::new();
    let mut ret = VariantContextHandling::Boxed;
    for attr in attrs {
        if attr.path().is_ident("ostd") {
            let arg = attr.parse_args::<SimpleAttrArgument>()?;
            match (
                arg.prefix.to_string().as_str(),
                arg.argument.to_string().as_str(),
            ) {
                ("context", "source") => {
                    ret = VariantContextHandling::Source;
                }
                ("context", "boxed") => {
                    ret = VariantContextHandling::Boxed;
                }
                ("context", "inline") => {
                    ret = VariantContextHandling::Inline;
                }
                ("context", "none") => {
                    ret = VariantContextHandling::None;
                }
                _ => {
                    return Err(syn::Error::new(
                        attr.span(),
                        "invalid argument to ostd(..)".to_owned(),
                    ));
                }
            }
        } else {
            ret_attrs.push(attr);
        }
    }
    Ok((ret, ret_attrs))
}

pub(crate) fn ostd_error_process_fields(
    input: &Fields,
    context_handling: VariantContextHandling,
) -> proc_macro2::TokenStream {
    let new_context_field = match context_handling {
        VariantContextHandling::None | VariantContextHandling::Source => quote! {},
        _ => {
            let ty = match context_handling {
                VariantContextHandling::Inline => quote! { ::ostd::stack_info::StackInfo },
                VariantContextHandling::Boxed => {
                    quote! { ::alloc::boxed::Box<::ostd::stack_info::StackInfo> }
                }
                VariantContextHandling::None | VariantContextHandling::Source => unreachable!(),
            };
            quote! {
                /// Information about the context captured when the error occurred. This is captured automatically by Snafu.
                #[snafu(implicit)]
                context: #ty
            }
        }
    };
    match input {
        Fields::Named(syn::FieldsNamed {
            brace_token: _,
            named,
        }) => {
            let new_named = named.iter().collect::<Vec<_>>();
            quote! {
                {
                    #(#new_named,)*
                    #new_context_field
                }
            }
        }
        Fields::Unnamed(_fields_unnamed) => {
            quote_spanned! { input.span() => compile_error!("ostd_error only supports unit and named structs") }
        }
        Fields::Unit => {
            quote_spanned! { input.span() =>
                {
                    #new_context_field
                }
            }
        }
    }
}

pub(crate) fn generate_ostd_error_trait_impl(
    input: &Item,
) -> Result<proc_macro2::TokenStream, syn::Error> {
    match input {
        Item::Enum(ItemEnum {
            ident,
            variants,
            generics,
            ..
        }) => {
            let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
            let match_arms = variants
                .iter()
                .map(stack_info_extractor_arm_for_variant)
                .collect::<Result<Vec<_>, syn::Error>>()?;

            Ok(quote! {
                impl #impl_generics ::ostd::OstdError for #ident #type_generics #where_clause {
                    fn stack_info(&self) -> Option<&::ostd::stack_info::StackInfo> {
                        match self {
                            #(#match_arms),*
                        }
                    }
                }
            })
        }
        Item::Struct(syn::ItemStruct {
            attrs,
            ident,
            generics,
            ..
        }) => {
            let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
            let (context_handling, _attrs) = process_attrs(attrs.clone())?;
            let stack_info_body = match context_handling {
                VariantContextHandling::Inline | VariantContextHandling::Boxed => {
                    quote! { Some(&self.context) }
                }
                VariantContextHandling::Source => {
                    quote! { ::ostd::OstdError::stack_info(&self.source) }
                }
                VariantContextHandling::None => quote! { None },
            };
            Ok(quote! {
                impl #impl_generics ::ostd::OstdError for #ident #type_generics #where_clause {
                    fn stack_info(&self) -> Option<&::ostd::stack_info::StackInfo> {
                        #stack_info_body
                    }
                }
            })
        }
        _ => Err(syn::Error::new(
            input.span(),
            "ostd_error can only be applied to enums or structs".to_owned(),
        )),
    }
}

fn stack_info_extractor_arm_for_variant(
    variant: &Variant,
) -> Result<proc_macro2::TokenStream, syn::Error> {
    let variant_ident = &variant.ident;
    let (context_handling, _attrs) = process_attrs(variant.attrs.clone())?;
    match &variant.fields {
        Fields::Unnamed(_) => Ok(quote! {
            compile_error!("Unnamed fields not supported")
        }),
        _ => match context_handling {
            VariantContextHandling::Inline | VariantContextHandling::Boxed => {
                let pat = quote_spanned! { variant.fields.span() => { context, .. } };
                let body = quote_spanned! { variant.fields.span() => context };
                Ok(quote! {
                    Self::#variant_ident #pat => Some(#body)
                })
            }
            VariantContextHandling::Source => {
                let pat = quote_spanned! { variant.fields.span() => { source, .. } };
                let body = quote_spanned! { variant.fields.span() => ::ostd::error::OstdError::stack_info(source) };
                Ok(quote! {
                    Self::#variant_ident #pat => #body
                })
            }
            VariantContextHandling::None => {
                let pat = quote_spanned! { variant.fields.span() => { .. } };
                Ok(quote! {
                    Self::#variant_ident #pat => None
                })
            }
        },
    }
}
