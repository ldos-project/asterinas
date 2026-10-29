// SPDX-License-Identifier: MPL-2.0

use std::iter;

use heck::{ToSnakeCase, ToUpperCamelCase};
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::{
    Attribute, Block, FnArg, Ident, ImplItem, Item, ItemImpl, ItemTrait, Lifetime, Pat, Path,
    ReturnType, Signature, Token, TraitItem, Type, TypePath, Visibility,
    fold::{self, Fold},
    parse::Parse,
    spanned::Spanned,
};

/// Get an appropriate module name based on a type name.
fn module_name(type_name: &Ident) -> Ident {
    let name = type_name.to_string().to_snake_case();
    syn::parse_str(&name).unwrap_or_else(|_| Ident::new_raw(&name, type_name.span()))
}

/// The options of a `trace_methods` attribute on an impl.
#[derive(Default)]
pub struct ImplOptions {
    /// Visibility of generated event types.
    visibility: Option<Visibility>,
    /// Module containing the implemented trait.
    trait_module: Option<Path>,
}

impl Parse for ImplOptions {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut options = Self::default();
        while !input.is_empty() {
            let name: Ident = input.parse()?;
            if name == "visibility" {
                if options.visibility.is_some() {
                    return Err(syn::Error::new(name.span(), "duplicate `visibility`"));
                }
                input.parse::<Token![=]>()?;
                options.visibility = Some(input.parse()?);
            } else if name == "trait_module" {
                if options.trait_module.is_some() {
                    return Err(syn::Error::new(name.span(), "duplicate `trait_module`"));
                }
                input.parse::<Token![=]>()?;
                options.trait_module = Some(input.parse()?);
            } else {
                return Err(syn::Error::new(
                    name.span(),
                    "expected `visibility` or `trait_module`",
                ));
            }

            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(options)
    }
}

/// Removes tracing annotations from a trait or impl for baseline builds.
pub fn baseline_trace_methods(mut item: Item) -> syn::Result<TokenStream> {
    match &mut item {
        Item::Impl(impl_block) => {
            for item in &mut impl_block.items {
                if let ImplItem::Method(method) = item {
                    remove_trace_annotations(&mut method.attrs, &mut method.sig)?;
                }
            }
        }
        Item::Trait(trait_item) => {
            for item in &mut trait_item.items {
                if let TraitItem::Method(method) = item {
                    remove_trace_annotations(&mut method.attrs, &mut method.sig)?;
                }
            }
        }
        _ => {
            return Err(syn::Error::new_spanned(
                item,
                "trace_methods must be placed on a trait or impl",
            ));
        }
    }

    Ok(quote!(#item))
}

fn remove_trace_annotations(
    attrs: &mut Vec<Attribute>,
    signature: &mut Signature,
) -> syn::Result<()> {
    has_trace_attrs(attrs)?;
    for argument in &mut signature.inputs {
        match argument {
            FnArg::Receiver(receiver) => {
                take_include(&mut receiver.attrs)?;
            }
            FnArg::Typed(argument) => {
                take_include(&mut argument.attrs)?;
            }
        }
    }
    Ok(())
}

/// The flags for which "directions" should include a given argument.
struct Include {
    /// Include the argument in call events.
    call: bool,
    /// Include the argument in return events.
    return_: bool,
}

impl Default for Include {
    fn default() -> Self {
        Self {
            call: true,
            return_: true,
        }
    }
}

impl Parse for Include {
    /// Parse an `only = call` or `only = (call, return)` option.
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let parameter: Ident = input.parse()?;
        if parameter != "only" {
            return Err(syn::Error::new(parameter.span(), "expected `only`"));
        }
        input.parse::<Token![=]>()?;

        let mut include = Self {
            call: false,
            return_: false,
        };

        fn parse_choice(input: syn::parse::ParseStream, include: &mut Include) -> syn::Result<()> {
            if input.peek(Token![return]) {
                input.parse::<Token![return]>()?;
                include.return_ = true;
            } else {
                let target: Ident = input.parse()?;
                if target != "call" {
                    return Err(syn::Error::new(
                        target.span(),
                        "expected `call` or `return`",
                    ));
                }
                include.call = true;
            }
            Ok(())
        }

        if input.peek(syn::token::Paren) {
            let content;
            syn::parenthesized!(content in input);
            while !content.is_empty() {
                parse_choice(&content, &mut include)?;
                if !content.is_empty() {
                    content.parse::<Token![,]>()?;
                }
            }
        } else {
            parse_choice(input, &mut include)?;
        }
        if !input.is_empty() {
            return Err(input.error("unexpected trace argument"));
        }

        Ok(include)
    }
}

/// A method argument available to call or return events.
struct EventField {
    /// Source span of the argument.
    span: Span,
    /// Field name in the event type.
    name: Ident,
    /// Field type in the event type.
    ty: Type,
    /// Expression passed to the event constructor.
    value: TokenStream,
    /// Events containing this field.
    include: Include,
}

/// The call and return event types for a traced method.
struct MethodSchema {
    /// Traced method name.
    method_name: Ident,
    /// Call event type name.
    call_type: Ident,
    /// Return event type name.
    return_type: Ident,
    /// Fields derived from method arguments.
    fields: Vec<EventField>,
    /// Method return type.
    output_type: Type,
    /// Source span of the return type.
    output_span: Span,
}

/// The owner and event namespace of a traced impl.
struct ExpansionContext {
    /// Module containing the event types.
    event_module: TokenStream,
    /// Module path used in trace names.
    trace_module_path: TraceModulePath,
    /// Type or trait named in trace paths.
    owner_name: Ident,
    /// Type recorded for the receiver.
    receiver_type: Type,
    /// Concrete `Self` type in inherent methods.
    data_self_type: Option<Type>,
    /// Helper for converting trait receivers to trait objects.
    receiver_helper: Option<Ident>,
}

/// Parts of the module path used in trace names.
struct TraceModulePath {
    /// Base module path expression.
    prefix: TokenStream,
    /// Path segments appended to the base.
    segments: Vec<TokenStream>,
}

impl TraceModulePath {
    fn current_module() -> Self {
        Self {
            prefix: quote!(Some(module_path!())),
            segments: Vec::new(),
        }
    }

    fn trait_module(path: &Path) -> Self {
        let mut segments = path.segments.iter().map(|segment| &segment.ident);
        let first = segments.next().unwrap();
        let rest: Vec<_> = segments.collect();

        if first == "crate" {
            let mut segments = vec![quote!(::ostd::orpc::path::crate_name_from_module_path(
                module_path!()
            ))];
            segments.extend(rest.iter().map(|segment| quote!(stringify!(#segment))));
            Self {
                prefix: quote!(None),
                segments,
            }
        } else if first == "self" || first == "super" {
            let additional_parents = if first == "super" {
                rest.iter()
                    .take_while(|segment| **segment == "super")
                    .count()
            } else {
                0
            };
            let parents = usize::from(first == "super") + additional_parents;
            let mut module_path = quote!(module_path!());
            for _ in 0..parents {
                module_path = quote!(#module_path.rsplit_once("::").unwrap().0);
            }
            Self {
                prefix: quote!(Some(#module_path)),
                segments: rest
                    .into_iter()
                    .skip(additional_parents)
                    .map(|segment| quote!(stringify!(#segment)))
                    .collect(),
            }
        } else {
            Self {
                prefix: quote!(Some(
                    concat!(stringify!(#first) #(, "::", stringify!(#rest))*)
                )),
                segments: Vec::new(),
            }
        }
    }
}

/// Expand a traced impl with call and return events.
pub fn trace_methods_impl(
    options: ImplOptions,
    mut impl_block: ItemImpl,
) -> syn::Result<TokenStream> {
    let self_type = impl_type(&impl_block)?;
    let type_name = self_type.path.segments.last().unwrap().ident.clone();
    let trait_path = impl_block.trait_.as_ref().map(|(_, path, _)| path.clone());
    let trait_name = trait_path
        .as_ref()
        .map(|path| path.segments.last().unwrap().ident.clone());
    let concrete_self_type = Type::Path(self_type.clone());
    let receiver_type = trait_path.as_ref().map_or_else(
        || concrete_self_type.clone(),
        |path| syn::parse_quote!(dyn #path),
    );
    let data_self_type = trait_path.is_none().then_some(concrete_self_type);
    let module_name = module_name(trait_name.as_ref().unwrap_or(&type_name));
    if trait_name.is_none()
        && let Some(trait_module) = &options.trait_module
    {
        return Err(syn::Error::new_spanned(
            trait_module,
            "`trait_module` is only valid on a trait impl",
        ));
    }
    let trait_module = options.trait_module.or_else(|| {
        trait_path.as_ref().and_then(|path| {
            let mut module = path.clone();
            module.segments = path
                .segments
                .iter()
                .take(path.segments.len() - 1)
                .cloned()
                .collect();
            (!module.segments.is_empty()).then_some(module)
        })
    });
    let event_module = match &trait_module {
        Some(trait_module) => quote!(#trait_module::#module_name),
        None => quote!(#module_name),
    };
    let trace_module_path = trait_module.as_ref().map_or_else(
        TraceModulePath::current_module,
        TraceModulePath::trait_module,
    );
    let generates_types = trait_name.is_none();
    if !generates_types && options.visibility.is_some() {
        return Err(syn::Error::new_spanned(
            &impl_block.self_ty,
            "`visibility` is not valid on a trait impl",
        ));
    }
    let module_visibility = options.visibility.clone().unwrap_or(Visibility::Inherited);
    let item_visibility = options
        .visibility
        .unwrap_or_else(|| syn::parse_quote!(pub(super)));
    let context = ExpansionContext {
        event_module,
        trace_module_path,
        owner_name: trait_name.unwrap_or(type_name),
        receiver_type,
        data_self_type,
        receiver_helper: None,
    };

    let mut event_types = Vec::new();
    for item in &mut impl_block.items {
        let ImplItem::Method(method) = item else {
            continue;
        };
        if !has_trace_attrs(&mut method.attrs)? {
            continue;
        }

        let schema = instrument_method(&mut method.sig, &mut method.block, &context)?;
        if generates_types {
            event_types.push(generate_event_types(&schema, &item_visibility));
        }
    }

    let event_module = generates_types.then(|| {
        quote! {
            #[cfg(not(baseline_asterinas))]
            #module_visibility mod #module_name {
                use super::*;

                #(#event_types)*
            }
        }
    });

    Ok(quote! {
        #event_module
        #impl_block
    })
}

/// Expand a traced trait with call and return event types.
pub fn trace_methods_trait(
    options: ImplOptions,
    mut trait_item: ItemTrait,
) -> syn::Result<TokenStream> {
    if options.trait_module.is_some() {
        return Err(syn::Error::new_spanned(
            &trait_item.ident,
            "`trait_module` is only valid on a trait impl",
        ));
    }
    if !trait_item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &trait_item.generics,
            "trace_methods does not support generic traits",
        ));
    }

    let trait_name = trait_item.ident.clone();
    let module_name = module_name(&trait_name);
    let receiver_type: Type = syn::parse_quote!(dyn #trait_name);
    let receiver_helper = format_ident!("__{}TraceReceiver", trait_name);
    let module_visibility = options.visibility.clone().unwrap_or(Visibility::Inherited);
    let item_visibility = options
        .visibility
        .unwrap_or_else(|| syn::parse_quote!(pub(super)));
    let context = ExpansionContext {
        event_module: quote!(#module_name),
        trace_module_path: TraceModulePath::current_module(),
        owner_name: trait_name.clone(),
        receiver_type,
        data_self_type: None,
        receiver_helper: Some(receiver_helper.clone()),
    };
    let mut event_types = Vec::new();
    let mut has_traced_default = false;

    for item in &mut trait_item.items {
        let TraitItem::Method(method) = item else {
            continue;
        };
        if !has_trace_attrs(&mut method.attrs)? {
            continue;
        }

        let schema = if let Some(block) = &mut method.default {
            has_traced_default = true;
            instrument_method(&mut method.sig, block, &context)?
        } else {
            analyze_signature(&mut method.sig, &context.receiver_type, None, None)?
        };
        event_types.push(generate_event_types(&schema, &item_visibility));
    }

    let helper = has_traced_default.then(|| {
        trait_item.colon_token.get_or_insert_with(Default::default);
        trait_item
            .supertraits
            .push(syn::parse_quote!(#receiver_helper));
        let visibility = &trait_item.vis;
        quote! {
            /// A strange trait to allow getting a dyn reference from default methods.
            #[doc(hidden)]
            #visibility trait #receiver_helper {
                fn __as_dyn(&self) -> &dyn #trait_name;
            }

            impl<T: #trait_name + Sized> #receiver_helper for T {
                fn __as_dyn(&self) -> &dyn #trait_name {
                    self
                }
            }
        }
    });

    Ok(quote! {
        #[cfg(not(baseline_asterinas))]
        #module_visibility mod #module_name {
            use super::*;

            #(#event_types)*
        }

        #helper
        #trait_item
    })
}

/// Get the concrete type named by a supported impl.
fn impl_type(impl_block: &ItemImpl) -> syn::Result<TypePath> {
    if !impl_block.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &impl_block.generics,
            "trace_methods does not support generic impls",
        ));
    }

    let Type::Path(self_type) = impl_block.self_ty.as_ref() else {
        return Err(syn::Error::new_spanned(
            &impl_block.self_ty,
            "trace_methods requires a concrete type",
        ));
    };
    if self_type.qself.is_some() {
        return Err(syn::Error::new_spanned(
            self_type,
            "trace_methods does not support qualified self types",
        ));
    }

    Ok(self_type.clone())
}

/// Add call and return tracing to a method.
fn instrument_method(
    signature: &mut Signature,
    block: &mut Block,
    context: &ExpansionContext,
) -> syn::Result<MethodSchema> {
    let schema = analyze_signature(
        signature,
        &context.receiver_type,
        context.data_self_type.as_ref(),
        context.receiver_helper.as_ref(),
    )?;
    let call_values = schema
        .fields
        .iter()
        .filter(|field| field.include.call)
        .map(|field| &field.value);
    let return_values = schema
        .fields
        .iter()
        .filter(|field| field.include.return_)
        .map(|field| &field.value);
    let original_block = block.clone();
    let call_value_type = event_type(&context.event_module, &schema.call_type);
    let return_value_type = event_type(&context.event_module, &schema.return_type);
    let return_value = quote_spanned!(schema.output_span=> &__return_value);
    let call_event = quote_spanned!(signature.span()=><#call_value_type>::new(#(#call_values),*));
    let return_event = quote_spanned!(signature.span()=><#return_value_type>::new(#(#return_values,)* #return_value));
    let call_trace = trace_expression(
        context,
        &schema.method_name,
        quote!(call),
        &call_event,
        &call_value_type,
    );
    let return_trace = trace_expression(
        context,
        &schema.method_name,
        quote!(return),
        &return_event,
        &return_value_type,
    );
    *block = syn::parse_quote!({
        #[cfg(not(baseline_asterinas))]
        {
            #call_trace
        }

        // Closure is required to correctly handle return statements in original_block.
        #[expect(clippy::borrow_deref_ref)]
        let __return_value = (|| #original_block)();

        #[cfg(not(baseline_asterinas))]
        {
            #return_trace
        }

        __return_value
    });
    Ok(schema)
}

/// Describe the events associated with a method signature.
fn analyze_signature(
    signature: &mut Signature,
    receiver_type: &Type,
    data_self_type: Option<&Type>,
    receiver_helper: Option<&Ident>,
) -> syn::Result<MethodSchema> {
    validate_method(signature)?;

    let mut normalizer = TypeNormalizer::default();
    let fields = method_fields(
        signature,
        receiver_type,
        data_self_type,
        receiver_helper,
        &mut normalizer,
    )?;
    let method_name = signature.ident.clone();
    let type_prefix = method_name.to_string().to_upper_camel_case();
    let call_type = format_ident!("{type_prefix}Call", span = method_name.span());
    let return_type = format_ident!("{type_prefix}Return", span = method_name.span());
    let (output_type, output_span) = match &signature.output {
        ReturnType::Default => (
            syn::parse_quote_spanned!(method_name.span()=> ()),
            method_name.span(),
        ),
        ReturnType::Type(_, ty) => (
            normalizer.normalize_data_type(ty.as_ref().clone(), data_self_type)?,
            ty.span(),
        ),
    };

    Ok(MethodSchema {
        method_name,
        call_type,
        return_type,
        fields,
        output_type,
        output_span,
    })
}

/// Define a method's call and return event types.
fn generate_event_types(schema: &MethodSchema, visibility: &Visibility) -> TokenStream {
    let call_fields: Vec<_> = schema
        .fields
        .iter()
        .filter(|field| field.include.call)
        .collect();
    let return_fields: Vec<_> = schema
        .fields
        .iter()
        .filter(|field| field.include.return_)
        .collect();
    let call_definition = event_definition(&schema.call_type, &call_fields, None, visibility);
    let return_definition = event_definition(
        &schema.return_type,
        &return_fields,
        Some(&schema.output_type),
        visibility,
    );

    quote! {
        #call_definition
        #return_definition
    }
}

/// Check whether a method supports tracing.
fn validate_method(signature: &Signature) -> syn::Result<()> {
    if signature.asyncness.is_some() {
        return Err(syn::Error::new_spanned(
            signature.asyncness,
            "trace does not support async methods",
        ));
    }
    Ok(())
}

/// Get the arguments recorded in a method's events.
fn method_fields(
    signature: &mut Signature,
    receiver_type: &Type,
    data_self_type: Option<&Type>,
    receiver_helper: Option<&Ident>,
    normalizer: &mut TypeNormalizer,
) -> syn::Result<Vec<EventField>> {
    let mut fields = Vec::new();

    for argument in &mut signature.inputs {
        match argument {
            FnArg::Receiver(receiver) => {
                let include = take_include(&mut receiver.attrs)?;
                let span = receiver.span();
                let value = if let Some(helper) = receiver_helper {
                    if receiver.reference.is_some() {
                        quote_spanned!(span=> #helper::__as_dyn(self))
                    } else {
                        quote_spanned!(span=> #helper::__as_dyn(&self))
                    }
                } else if receiver.reference.is_some() {
                    quote_spanned!(span=> self)
                } else {
                    quote_spanned!(span=> &self)
                };
                fields.push(EventField {
                    span,
                    name: format_ident!("self_", span = span),
                    ty: receiver_type.clone(),
                    value,
                    include,
                });
            }
            FnArg::Typed(argument) => {
                let Pat::Ident(pattern) = argument.pat.as_ref() else {
                    return Err(syn::Error::new_spanned(
                        &argument.pat,
                        "trace requires identifier argument patterns",
                    ));
                };
                let include = take_include(&mut argument.attrs)?;
                let span = argument.span();
                let is_self = pattern.ident == "self";
                let is_trait_receiver = is_self && matches!(receiver_type, Type::TraitObject(_));
                let name = event_field_name(&pattern.ident);
                let value = if is_self {
                    if let Some(helper) = receiver_helper {
                        quote_spanned!(span=> #helper::__as_dyn(&*self))
                    } else if is_trait_receiver {
                        quote_spanned!(span=> &*self)
                    } else {
                        quote_spanned!(span=> &self)
                    }
                } else {
                    let ident = &pattern.ident;

                    // Generate a reborrow based on the depth of the refs in the argument type. This
                    // is required, because we strip the `mut`s from the ref types, to avoid
                    // lifetime problems caused by lifetime parameter invariance of the mutable
                    // references.
                    fn count_refs(t: &Type) -> usize {
                        match t {
                            Type::Reference(type_reference) => count_refs(&type_reference.elem) + 1,
                            _ => 0,
                        }
                    }
                    let n_refs = count_refs(&argument.ty);
                    let refs = iter::repeat_n(quote! {&}, n_refs);
                    let derefs = iter::repeat_n(quote! {*}, n_refs);

                    quote_spanned!(span=> &#(#refs)*#(#derefs)*#ident)
                };
                let ty = if is_trait_receiver {
                    receiver_type.clone()
                } else if is_self {
                    normalizer.normalize_receiver_type(argument.ty.as_ref().clone(), receiver_type)
                } else {
                    normalizer.normalize_data_type(argument.ty.as_ref().clone(), data_self_type)?
                };
                fields.push(EventField {
                    span,
                    name,
                    ty,
                    value,
                    include,
                });
            }
        }
    }

    Ok(fields)
}

/// Remove leading underscores from an argument's event field name.
fn event_field_name(argument_name: &Ident) -> Ident {
    let name = argument_name.to_string();
    let stripped = name.trim_start_matches('_');
    if stripped.is_empty() {
        return argument_name.clone();
    }

    let mut field_name =
        syn::parse_str::<Ident>(stripped).unwrap_or_else(|_| format_ident!("{stripped}_"));
    field_name.set_span(argument_name.span());
    field_name
}

/// Get the events in which an argument appears.
fn take_include(attrs: &mut Vec<Attribute>) -> syn::Result<Include> {
    let (mut trace_attrs, other_attrs): (Vec<_>, _) = std::mem::take(attrs)
        .into_iter()
        .partition(|attr| attr.path.is_ident("trace"));
    *attrs = other_attrs;
    if trace_attrs.len() > 1 {
        return Err(syn::Error::new_spanned(
            trace_attrs.pop().unwrap(),
            "duplicate `trace` attribute",
        ));
    }
    match trace_attrs.pop() {
        Some(attr) => attr.parse_args(),
        None => Ok(Include::default()),
    }
}

/// Remove a method-level `trace` attribute, rejecting duplicates and arguments.
fn has_trace_attrs(attrs: &mut Vec<Attribute>) -> syn::Result<bool> {
    let (mut trace_attrs, other_attrs) = std::mem::take(attrs)
        .into_iter()
        .partition(|attr| attr.path.is_ident("trace"));
    *attrs = other_attrs;
    if trace_attrs.len() > 1 {
        return Err(syn::Error::new_spanned(
            trace_attrs.pop().unwrap(),
            "duplicate `trace` attribute",
        ));
    }
    if let Some(attr) = trace_attrs.pop() {
        if !attr.tokens.is_empty() {
            return Err(syn::Error::new_spanned(
                attr,
                "`trace` does not take arguments",
            ));
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Define a call or return event type.
fn event_definition(
    type_name: &Ident,
    fields: &[&EventField],
    output_type: Option<&Type>,
    visibility: &Visibility,
) -> TokenStream {
    let field_definitions = fields.iter().map(|field| {
        let EventField { span, name, ty, .. } = field;
        quote_spanned!(*span=> #visibility #name: &'__trace #ty,)
    });
    let constructor_parameters = fields.iter().map(|field| {
        let EventField { span, name, ty, .. } = field;
        quote_spanned!(*span=> #name: &'__trace #ty,)
    });
    let constructor_fields = fields.iter().map(|field| &field.name);

    if fields.is_empty() && output_type.is_none() {
        return quote! {
            #[derive(::ostd::orpc::oqueue::Element)]
            #visibility struct #type_name;

            impl #type_name {
                #visibility fn new() -> Self {
                    Self
                }
            }
        };
    }

    let output_field =
        output_type.map(|ty| quote_spanned!(ty.span()=> #visibility return_value: &'__trace #ty,));
    let output_parameter =
        output_type.map(|ty| quote_spanned!(ty.span()=> return_value: &'__trace #ty,));
    let output_initializer = output_type.map(|_| quote!(return_value,));

    quote! {
        #[derive(::ostd::orpc::oqueue::Element)]
        #visibility struct #type_name<'__trace> {
            #(#field_definitions)*
            #output_field
        }

        impl<'__trace> #type_name<'__trace> {
            #visibility fn new(
                #(#constructor_parameters)*
                #output_parameter
            ) -> Self {
                Self {
                    #(#constructor_fields,)*
                    #output_initializer
                }
            }
        }
    }
}

/// Name an event type in its module.
fn event_type(event_module: &TokenStream, type_name: &Ident) -> TokenStream {
    quote!(#event_module::#type_name)
}

/// Emit an event
fn trace_expression(
    context: &ExpansionContext,
    method_name: &Ident,
    event: TokenStream,
    value: &TokenStream,
    value_type: &TokenStream,
) -> TokenStream {
    let prefix = &context.trace_module_path.prefix;
    let segments = &context.trace_module_path.segments;
    let type_name = &context.owner_name;
    let path = quote! {
        ::ostd::orpc::path::Path::traced_method_path(
            #prefix,
            &[#(#segments,)*
              stringify!(#type_name), stringify!(#method_name), stringify!(#event)]
        )
    };

    quote! {
        ::ostd::__trace_structured_data_at_path!(
            #path,
            #value,
            #value_type,
        );
    }
}

/// Normalizes method types for use in trace events.
#[derive(Default)]
struct TypeNormalizer {
    /// Type replacing `Self` in event fields.
    self_type: Option<Type>,
    /// Whether the current type contains `Self`.
    found_self: bool,
}

impl TypeNormalizer {
    /// Resolve a data type for a trace event.
    fn normalize_data_type(&mut self, ty: Type, self_type: Option<&Type>) -> syn::Result<Type> {
        self.self_type = self_type.cloned();
        self.found_self = false;
        let normalized = self.fold_type(ty.clone());
        if self.found_self && self.self_type.is_none() {
            return Err(syn::Error::new_spanned(
                ty,
                "`Self` is only supported in self receiver types",
            ));
        }
        Ok(normalized)
    }

    /// Resolve a receiver type for a trace event.
    fn normalize_receiver_type(&mut self, ty: Type, self_type: &Type) -> Type {
        self.self_type = Some(self_type.clone());
        self.found_self = false;
        self.fold_type(ty)
    }
}

impl Fold for TypeNormalizer {
    /// Resolve `Self` in a type.
    fn fold_type(&mut self, ty: Type) -> Type {
        if let Type::Path(type_path) = &ty
            && type_path.qself.is_none()
            && type_path.path.is_ident("Self")
        {
            self.found_self = true;
            return self.self_type.clone().unwrap_or(ty);
        }

        fold::fold_type(self, ty)
    }

    /// Resolve paths beginning with `Self`.
    fn fold_type_path(&mut self, type_path: TypePath) -> TypePath {
        if type_path
            .path
            .segments
            .first()
            .is_some_and(|segment| segment.ident == "Self")
        {
            self.found_self = true;
            if let Some(self_type) = self.self_type.clone() {
                let span = type_path.span();
                let tail = type_path.path.segments.iter().skip(1);
                let normalized = syn::parse2(quote_spanned!(span=> <#self_type>::#(#tail)::*))
                    .expect("normalized Self path must be a valid type path");
                return fold::fold_type_path(self, normalized);
            }
        }
        fold::fold_type_path(self, type_path)
    }

    /// Bind non-static references to the event lifetime.
    fn fold_lifetime(&mut self, lifetime: Lifetime) -> Lifetime {
        if lifetime.ident == "static" {
            lifetime
        } else {
            Lifetime::new("'__trace", lifetime.span())
        }
    }

    /// Bind references without an explicit lifetime to the event lifetime and remove mut from
    /// references.
    fn fold_type_reference(&mut self, mut reference: syn::TypeReference) -> syn::TypeReference {
        if reference.lifetime.is_none() {
            reference.lifetime = Some(Lifetime::new("'__trace", reference.and_token.span));
        }
        reference.mutability = None;
        fold::fold_type_reference(self, reference)
    }
}

#[cfg(test)]
mod tests {
    use quote::quote;
    use syn::parse_quote;

    use super::*;

    fn expand_impl(options: ImplOptions, item: ItemImpl) -> String {
        trace_methods_impl(options, item).unwrap().to_string()
    }

    fn expand_trait(options: ImplOptions, item: ItemTrait) -> String {
        trace_methods_trait(options, item).unwrap().to_string()
    }

    fn expand_trait_impl(trait_path: &str) -> String {
        let item = syn::parse_str(&format!(
            "impl {trait_path} for Worker {{ #[trace] fn process(&self) {{}} }}"
        ))
        .unwrap();
        expand_impl(parse_quote!(), item)
    }

    // This does not test the placement of spans, because what is "correct" is unclear and not worth
    // testing.

    #[test]
    fn baseline_impl_keeps_method_bodies_without_trace_annotations() {
        let item = parse_quote! {
            impl Worker {
                #[trace]
                fn process(&self, #[trace(only = return)] value: u32) -> u32 {
                    value + 1
                }
            }
        };

        let expanded = baseline_trace_methods(item).unwrap();

        assert_eq!(
            expanded.to_string(),
            quote! {
                impl Worker {
                    fn process(&self, value: u32) -> u32 {
                        value + 1
                    }
                }
            }
            .to_string()
        );
    }

    #[test]
    fn parses_named_options() {
        assert!(syn::parse_str::<ImplOptions>("visibility = pub(crate)").is_ok());
        assert!(syn::parse_str::<ImplOptions>("trait_module = crate::api").is_ok());
        assert!(
            syn::parse_str::<ImplOptions>("visibility = pub(crate), trait_module = crate::api")
                .is_ok()
        );
    }

    #[test]
    fn parses_only_selection() {
        let include: Include = syn::parse_str("only = (call, return)").unwrap();
        assert!(include.call && include.return_);
        let include: Include = syn::parse_str("only = call").unwrap();
        assert!(include.call && !include.return_);
        let include: Include = syn::parse_str("only = return").unwrap();
        assert!(!include.call && include.return_);
    }

    #[test]
    fn rejects_method_trace_arguments() {
        for attr in [
            "#[trace()]",
            "#[trace(path = \"custom\")]",
            "#[trace = \"custom\"]",
        ] {
            let impl_block =
                syn::parse_str(&format!("impl Worker {{ {attr} fn process(&self) {{}} }}"))
                    .unwrap();

            let error = trace_methods_impl(parse_quote!(), impl_block)
                .unwrap_err()
                .to_string();

            assert_eq!(error, "`trace` does not take arguments", "{attr}");
        }
    }

    #[test]
    fn generates_public_types_and_default_paths() {
        let impl_block = parse_quote! {
            impl Worker {
                #[trace]
                fn process(&mut self, input: Input) -> Result<()> {
                    run(input)
                }
            }
        };

        let output = expand_impl(parse_quote!(visibility = pub(crate)), impl_block);

        assert!(output.contains("pub (crate) mod worker"));
        assert!(output.contains("pub (crate) struct ProcessCall"));
        assert!(output.contains("self_ : & '__trace Worker"));
        assert!(output.contains("input : & '__trace Input"));
        assert!(output.contains("traced_method_path (Some (module_path ! ())"));
    }

    #[test]
    fn strips_leading_underscores_from_event_fields() {
        let impl_block = parse_quote! {
            impl Worker {
                #[trace]
                fn process(&self, _input: Input, __context: Context, _type: Type, __: Marker) {}

                #[trace]
                fn named_self(_self: Other) {}
            }
        };

        let output = expand_impl(parse_quote!(), impl_block);

        assert!(output.contains("input : & '__trace Input"));
        assert!(output.contains("context : & '__trace Context"));
        assert!(output.contains("type_ : & '__trace Type"));
        assert!(output.contains("__ : & '__trace Marker"));
        assert!(output.contains("new (self , & _input , & __context , & _type , & __)"));
        assert!(output.contains("self_ : & '__trace Other"));
        assert!(output.contains("new (& _self)"));
    }

    #[test]
    fn applies_field_selection() {
        let impl_block = parse_quote! {
            impl Worker {
                #[trace]
                fn process(
                    #[trace(only = ())] self: Arc<Self>,
                    #[trace(only = call)] input: Input,
                    #[trace(only = return)] context: Context,
                ) -> Result<()> {
                    run(input, &context)
                }
            }
        };

        let output = expand_impl(parse_quote!(), impl_block);

        assert!(!output.contains("self_ :"));
        assert!(output.contains("new (& input)"), "{output}");
        assert!(
            output.contains("new (& context , & __return_value)"),
            "{output}"
        );
        assert!(!output.contains("trace (only"));
    }

    #[test]
    fn defaults_generated_items_to_parent_visibility() {
        let impl_block = parse_quote! {
            impl Worker {
                #[trace]
                fn poll(&self) {}
            }
        };

        let output = expand_impl(parse_quote!(), impl_block);

        assert!(output.contains("pub (super) struct PollCall"));
    }

    #[test]
    fn instruments_default_trait_methods() {
        let trait_item = parse_quote! {
            pub trait WorkerTrait {
                #[trace]
                fn process(&self, _input: Input) -> Result<()> {
                    unsupported()
                }
            }
        };

        let output = expand_trait(parse_quote!(visibility = pub(crate)), trait_item);

        assert!(output.contains("pub (crate) mod worker_trait"));
        assert!(output.contains("self_ : & '__trace dyn WorkerTrait"));
        assert!(output.contains("trait WorkerTrait : __WorkerTraitTraceReceiver"));
        assert!(output.contains("__WorkerTraitTraceReceiver :: __as_dyn (self)"));
        assert!(
            output.contains("impl < T : WorkerTrait + Sized > __WorkerTraitTraceReceiver for T")
        );
        assert!(output.contains("< worker_trait :: ProcessCall > :: new"));
        assert!(output.contains("< worker_trait :: ProcessReturn > :: new"));
        assert!(!output.contains("trace ]"));
    }

    #[test]
    fn required_trait_methods_do_not_add_a_receiver_helper() {
        let trait_item = parse_quote! {
            trait WorkerTrait {
                #[trace]
                fn process(&self, input: Input) -> Result<()>;
            }
        };

        let output = expand_trait(parse_quote!(), trait_item);

        assert!(!output.contains("__WorkerTraitTraceReceiver"));
    }

    #[test]
    fn traces_default_method_with_arc_receiver() {
        let trait_item = parse_quote! {
            trait WorkerTrait {
                #[trace]
                fn process(self: Arc<Self>) {}
            }
        };

        let output = expand_trait(parse_quote!(), trait_item);

        assert!(output.contains("self_ : & '__trace dyn WorkerTrait"));
        assert!(output.contains("__WorkerTraitTraceReceiver :: __as_dyn (& * self)"));
    }

    #[test]
    fn trait_impl_uses_explicit_trait_module() {
        let impl_block = parse_quote! {
            impl WorkerTrait for Worker {
                #[trace]
                fn process(&self, input: Input) -> Result<()> {
                    run(input)
                }
            }
        };

        let output = expand_impl(parse_quote!(trait_module = crate::api), impl_block);

        assert!(!output.contains("mod worker_trait"));
        assert!(output.contains("crate :: api :: worker_trait :: ProcessCall"));
        assert!(
            output.contains("traced_method_path (None , & [:: ostd :: orpc :: path :: crate_name_from_module_path (module_path ! ()) , stringify ! (api) , stringify ! (WorkerTrait)")
        );
        assert!(output.contains("new (self , & input)"));
    }

    #[test]
    fn trait_impl_keeps_trait_and_event_modules_separate() {
        let impl_block = parse_quote! {
            impl Socket for StreamSocket {
                #[trace]
                fn connect(&self) {}
            }
        };

        let output = expand_impl(parse_quote!(trait_module = crate::net::socket), impl_block);

        assert!(output.contains("crate :: net :: socket :: socket :: ConnectCall"));
        assert!(output.contains("stringify ! (net) , stringify ! (socket) , stringify ! (Socket)"));
    }

    #[test]
    fn trait_impl_resolves_event_and_trace_paths() {
        for (trait_path, event_type, trace_module) in [
            (
                "WorkerTrait",
                "worker_trait :: ProcessCall",
                "Some (module_path ! ())",
            ),
            (
                "aster_core::api::WorkerTrait",
                "aster_core :: api :: worker_trait :: ProcessCall",
                "concat ! (stringify ! (aster_core) , \"::\" , stringify ! (api))",
            ),
            (
                "crate::api::WorkerTrait",
                "crate :: api :: worker_trait :: ProcessCall",
                "crate_name_from_module_path (module_path ! ()) , stringify ! (api)",
            ),
            (
                "crate::WorkerTrait",
                "crate :: worker_trait :: ProcessCall",
                "crate_name_from_module_path (module_path ! ()) , stringify ! (WorkerTrait)",
            ),
            (
                "self::WorkerTrait",
                "self :: worker_trait :: ProcessCall",
                "Some (module_path ! ())",
            ),
            (
                "super::WorkerTrait",
                "super :: worker_trait :: ProcessCall",
                "Some (module_path ! () . rsplit_once (\"::\") . unwrap () . 0)",
            ),
            (
                "super::super::api::WorkerTrait",
                "super :: super :: api :: worker_trait :: ProcessCall",
                "stringify ! (api) , stringify ! (WorkerTrait)",
            ),
        ] {
            let output = expand_trait_impl(trait_path);
            assert!(output.contains(event_type), "{trait_path}: {output}");
            assert!(output.contains(trace_module), "{trait_path}: {output}");
        }
    }

    #[test]
    fn resolves_self_in_inherent_method_types() {
        let impl_block = parse_quote! {
            impl Worker {
                #[trace]
                fn create(peer: Arc<Self>) -> Option<Self::Output> {
                    todo!()
                }
            }
        };

        let output = expand_impl(parse_quote!(), impl_block);

        assert!(output.contains("peer : & '__trace Arc < Worker >"));
        assert!(output.contains("value : & '__trace Option < < Worker > :: Output >"));
    }

    #[test]
    fn rejects_associated_self_types_in_trait_messages() {
        let trait_item = parse_quote! {
            trait WorkerTrait {
                #[trace]
                fn process(&self, input: Self::Input);
            }
        };

        let error = trace_methods_trait(parse_quote!(), trait_item).unwrap_err();

        assert_eq!(
            error.to_string(),
            "`Self` is only supported in self receiver types"
        );
    }
}
