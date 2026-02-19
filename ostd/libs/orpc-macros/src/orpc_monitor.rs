// SPDX-License-Identifier: MPL-2.0

use heck::ToUpperCamelCase;
use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::{Error, Ident, ItemImpl, Type, Visibility, parse_quote, spanned::Spanned as _};

/// The kind of attachment which a method should support.
#[derive(PartialEq)]
enum MethodAttachmentType {
    StrongObserver,
    Consumer,
    None,
}

impl MethodAttachmentType {
    fn to_attachment_type_symbol(&self) -> Option<TokenStream> {
        match self {
            MethodAttachmentType::StrongObserver => {
                Some(quote! { ::ostd::orpc::oqueue::StrongObserver })
            }
            MethodAttachmentType::Consumer => Some(quote! { ::ostd::orpc::oqueue::Consumer }),
            MethodAttachmentType::None => None,
        }
    }
}

/// The information about a method.
struct MethodDefinition {
    definition: syn::ImplItemMethod,
    attachment_type: MethodAttachmentType,
}

impl MethodDefinition {
    fn argument_type(&self) -> syn::Result<Option<Type>> {
        let self_argument = self.definition.sig.inputs.first();
        match self_argument {
            Some(syn::FnArg::Receiver(_)) => (),
            _ => {
                return Err(syn::Error::new(
                    self_argument.span(),
                    "Monitor methods must have a self argument.",
                ));
            }
        }
        let first_input = self.definition.sig.inputs.iter().nth(1);
        match first_input {
            Some(syn::FnArg::Typed(pat_type)) => Ok(Some(*pat_type.ty.clone())),
            None => Ok(None),
            _ => {
                // This is an error, but will be caught by the normal compiler.
                Ok(Some(parse_quote! { () }))
            }
        }
    }
}

pub fn orpc_monitor_impl(monitor_vis: Visibility, input_impl: ItemImpl) -> TokenStream {
    // A set of errors encountered.
    let mut errors = Vec::new();

    // Information about each method definition
    let mut method_definitions = Vec::new();

    for item in &input_impl.items {
        if let syn::ImplItem::Method(method) = item {
            // Collect information from the attributes.
            let mut is_strong_observer = false;
            let mut is_consumer = false;
            let mut filtered_attrs = Vec::new();

            for attr in &method.attrs {
                if attr.path.is_ident("strong_observer") {
                    is_strong_observer = true;
                } else if attr.path.is_ident("consumer") {
                    is_consumer = true;
                } else {
                    filtered_attrs.push(attr.clone());
                }
            }

            let kind = match (is_strong_observer, is_consumer) {
                (true, false) => MethodAttachmentType::StrongObserver,
                (false, true) => MethodAttachmentType::Consumer,
                (false, false) => MethodAttachmentType::None,
                (true, true) => {
                    errors.push(Error::new(
                        method.span(),
                        "Only a single attachment type can be used at a time. (This restriction may be relaxed in the future.)",
                    ));
                    MethodAttachmentType::None
                }
            };

            let definition = syn::ImplItemMethod {
                attrs: filtered_attrs,
                ..method.clone()
            };

            method_definitions.push(MethodDefinition {
                definition,
                attachment_type: kind,
            });
        } else {
            errors.push(Error::new(item.span(), "Only methods are allowed in #[orpc_monitor] impl. (You can use another impl on the same type.)"));
        }
    }

    // The fields of the *Attachment struct
    let mut attachment_fields = Vec::new();

    for method_def in &method_definitions {
        let method_name = &method_def.definition.sig.ident;

        match method_def.argument_type() {
            Ok(param_type) => {
                let param_type = param_type.clone().unwrap_or(parse_quote! {()});
                let field_name = format_ident!("{}_attachment", method_name);
                if let Some(attachment_type) =
                    method_def.attachment_type.to_attachment_type_symbol()
                {
                    attachment_fields.push(quote! {
                        #field_name: ::core::option::Option<#attachment_type<#param_type>>
                    });
                }
            }
            Err(e) => errors.push(e),
        }
    }

    // The name of the state type
    let state_name = match input_impl.self_ty.as_ref() {
        Type::Path(syn::TypePath { qself: None, path }) if path.get_ident().is_some() => {
            path.get_ident().unwrap().clone()
        }
        _ => {
            errors.push(Error::new(
                input_impl.self_ty.span(),
                "impl'd type must be referenced by a simple identifier",
            ));
            format_ident!("__ERROR__")
        }
    };

    // The name of monitor type
    let monitor_name = format_ident!("{}Monitor", state_name);
    // The name of the enum holding the commands
    let command_enum_name = format_ident!("{}MonitorCommand", state_name);
    // The name of the struct holding the attachments
    let attachment_struct_name = format_ident!("{}MonitorAttachments", state_name);

    // The commands in the command enum.
    let mut command_variants = Vec::new();
    // The match arms for `Debug` impl for the commands type. This cannot be derived, because it
    // needs to ignore the reply channels.
    let mut command_debug_match_arms = Vec::new();

    for method_def in &method_definitions {
        let return_type = match &method_def.definition.sig.output {
            syn::ReturnType::Default => quote! { () },
            syn::ReturnType::Type(_, ty) => quote! { #ty },
        };
        let method_name = &method_def.definition.sig.ident;
        let variant_name = format_ident!("{}", &method_name.to_string().to_upper_camel_case(),);

        if let Ok(param_type) = method_def.argument_type() {
            let param_type = param_type.clone().unwrap_or(parse_quote! {()});
            command_variants.push(quote! {
                #variant_name(#param_type, ::ostd::orpc::oqueue::ValueProducer<#return_type>),
            });

            if let Some(attachment_type) = method_def.attachment_type.to_attachment_type_symbol() {
                let attachment_message_name = format_ident!("Attach{variant_name}");
                command_variants.push(quote! {
                    #attachment_message_name (
                        #attachment_type<#param_type>,
                        ::ostd::orpc::oqueue::ValueProducer<::core::result::Result<(), ::ostd::orpc::oqueue::AttachmentError>>,
                    ),
                });
                command_debug_match_arms.push(quote! {
                    #command_enum_name::#attachment_message_name(_, _) =>
                        write!(f, "{}", stringify!(#attachment_message_name)),
                });
            }
            command_debug_match_arms.push(quote! {
                #command_enum_name::#variant_name(arg, _) =>
                    write!(f, "{}({:?})", stringify!(#variant_name), arg),
            });
        }
    }

    // The methods to include in the monitor `impl`
    let mut monitor_methods = Vec::new();

    for method_def in &method_definitions {
        let method_name = &method_def.definition.sig.ident;
        let variant_name = format_ident!("{}", method_name.to_string().to_upper_camel_case());

        let method_attrs = &method_def.definition.attrs;
        let method_vis = &method_def.definition.vis;

        if let Ok(param_type) = method_def.argument_type() {
            if let Some(attachment_type_base) =
                method_def.attachment_type.to_attachment_type_symbol()
            {
                let attachment_variant_name = format_ident!("Attach{}", variant_name);
                let attachment_method_name =
                    format_ident!("attach_{}", method_name, span = method_name.span());
                let param_type = param_type.clone().unwrap_or(parse_quote! {()});
                let attachment_type = quote! { #attachment_type_base<#param_type> };
                let span = method_def.definition.span();
                let attachment_docs = format!(
                    "
Configure the monitor to run `{method_name}` to handle values from `attachment`.

The documentation for [`Self::{method_name}`] is:\n\n",
                );
                monitor_methods.push(
                    quote_spanned! { span =>
                        #[doc = #attachment_docs]
                        #(#method_attrs)*
                        #[allow(clippy::allow_attributes)]
                        #[allow(unused)]
                        #method_vis fn #attachment_method_name(&self, attachment: #attachment_type)
                            -> ::core::result::Result<(), ::ostd::orpc::oqueue::AttachmentError>
                        {
                            ::ostd::orpc::framework::monitor::synchronous_request(
                                &self.command_producer,
                                |reply_producer| #command_enum_name::#attachment_variant_name(attachment, reply_producer)
                            )
                        }
                    }
                );
            }

            let return_type = match &method_def.definition.sig.output {
                syn::ReturnType::Default => quote! { () },
                syn::ReturnType::Type(_, ty) => quote! { #ty },
            };
            let (arg_decl, arg_use) = if let Some(param_type) = param_type {
                (quote! { arg: #param_type }, quote! { arg })
            } else {
                (quote! {}, quote! { () })
            };
            monitor_methods.push(quote_spanned! { method_def.definition.span() =>
                #(#method_attrs)*
                #[allow(clippy::allow_attributes)]
                #[allow(unused)]
                #method_vis fn #method_name(&self, #arg_decl) -> #return_type {
                    ::ostd::orpc::framework::monitor::synchronous_request(
                        &self.command_producer,
                        |reply_producer| #command_enum_name::#variant_name(#arg_use, reply_producer)
                    )
                }
            });
        }
    }

    monitor_methods.push(generate_start_fn(
        &method_definitions,
        state_name,
        &command_enum_name,
        &attachment_struct_name,
    ));

    let compiler_errors = {
        if let Some(mut collected_error) = errors.pop() {
            for e in errors {
                collected_error.combine(e);
            }
            collected_error.into_compile_error()
        } else {
            quote! {}
        }
    };

    let impl_without_our_attrs = {
        syn::ItemImpl {
            items: method_definitions
                .iter()
                .map(|d| d.definition.clone().into())
                .collect(),
            ..input_impl
        }
    };

    let expanded = quote! {
        #impl_without_our_attrs

        #[doc(hidden)]
        #[derive(Default)]
        struct #attachment_struct_name {
            #(#attachment_fields,)*
        }

        #[doc(hidden)]
        enum #command_enum_name {
            #(#command_variants)*
        }

        impl ::core::fmt::Debug for #command_enum_name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                match self {
                    #(#command_debug_match_arms)*
                }
            }
        }

        #monitor_vis struct #monitor_name {
            command_oqueue: ::ostd::orpc::oqueue::ConsumableOQueueRef<#command_enum_name>,
            command_producer: ::ostd::orpc::oqueue::ValueProducer<#command_enum_name>,
        }

        impl #monitor_name {
            #(#monitor_methods)*

            #monitor_vis fn new() -> Self {
                use ::ostd::orpc::oqueue::ConsumableOQueue;
                let command_oqueue = ::ostd::orpc::oqueue::ConsumableOQueueRef::new(2);
                Self {
                    command_producer: command_oqueue
                        .attach_value_producer()
                        .expect("single purpose OQueue failed."),
                    command_oqueue,
                }
            }
        }

        impl ::core::default::Default for #monitor_name {
            fn default() -> Self {
                Self::new()
            }
        }

        #compiler_errors
    };

    expanded
}

/// Generate a `start` method for the monitor which spawns a thread to process commands and messages
/// from the attached observers/consumers.
fn generate_start_fn(
    method_definitions: &Vec<MethodDefinition>,
    state_name: Ident,
    command_enum_name: &Ident,
    attachment_struct_name: &Ident,
) -> TokenStream {
    // The attachment blockers used for blocking the event loop.
    let mut attachment_blockers = Vec::new();
    // Arms for a match used to process every command
    let mut command_handling_arms = Vec::new();
    // Code blocks to poll and handle messages on attached OQueues.
    let mut observe_blocks = Vec::new();

    for method_def in method_definitions {
        let method_name = &method_def.definition.sig.ident;
        let field_name = format_ident!("{}_attachment", method_name);
        let variant_name = format_ident!("{}", method_name.to_string().to_upper_camel_case());
        let attachment_variant_name = format_ident!("Attach{}", variant_name);

        // If the message allows OQueue attachments,
        if method_def.attachment_type != MethodAttachmentType::None {
            // The blocker for this method's OQueue
            attachment_blockers.push(quote! {
                &attachments.#field_name
            });

            match method_def.attachment_type {
                MethodAttachmentType::StrongObserver => {
                    observe_blocks.push({
                        quote! {
                            match attachments
                                .#field_name
                                .as_ref()
                                .map(|a| a.try_strong_observe())
                            {
                                Some(Ok(Some(x))) => {
                                    state.#method_name(x)?;
                                }
                                Some(Ok(None)) => {}
                                Some(e @ Err(_)) => {
                                    ::ostd::ignore_err!(
                                        e,
                                        log::Level::Error,
                                        "Detaching from OQueue due to handler error"
                                    );
                                    attachments.#field_name = None;
                                }
                                None => {}
                            }
                        }
                    });
                }
                MethodAttachmentType::Consumer => {
                    observe_blocks.push({
                        quote! {
                            match attachments
                                .#field_name
                                .as_ref()
                                .map(|a| a.try_consume())
                            {
                                Some(Some(x)) => {
                                    state.#method_name(x)?;
                                }
                                _ => {}
                            }
                        }
                    });
                }
                MethodAttachmentType::None => {}
            }
        }

        // Handle method calls.
        let (arg_pat, call) = if method_def.definition.sig.inputs.len() > 1 {
            (quote! { arg }, quote! { state.#method_name(arg) })
        } else {
            (quote! { () }, quote! { state.#method_name() })
        };
        command_handling_arms.push(quote! {
            #command_enum_name::#variant_name(#arg_pat, value_producer) => {
                value_producer.produce(#call);
            }
        });

        // For method which can be attached, handle the attach method.
        if method_def.attachment_type != MethodAttachmentType::None {
            command_handling_arms.push(quote! {
                #command_enum_name::#attachment_variant_name(consumer, value_producer) => {
                    attachments.#field_name = Some(consumer.into());
                    value_producer.produce(Ok(()))
                }
            });
        }
    }

    // The expression holding all blockers for the event loop
    let all_blockers = if attachment_blockers.is_empty() {
        quote! { &[&command_consumer] }
    } else {
        quote! { &[&command_consumer, #(#attachment_blockers),*] }
    };

    quote! {
        fn start(&self, server: ::alloc::sync::Arc<dyn ::ostd::orpc::framework::Server>, state: #state_name) {
            ::ostd::orpc::framework::spawn_thread(server, {
                use ::ostd::orpc::oqueue::ConsumableOQueue;
                let command_consumer = self
                    .command_oqueue
                    .attach_consumer()
                    .expect("single purpose OQueue failed.");
                move || {
                    let mut state = state;
                    let mut attachments = #attachment_struct_name::default();
                    loop {
                        if let Some(c) = ::ostd::task::Task::current() {
                            c.block_on(#all_blockers);
                        } else {
                            ::ostd::task::Task::yield_now();
                        }
                        if let Some(cmd) = command_consumer.try_consume() {
                            match cmd {
                                #(#command_handling_arms)*
                            }
                        }
                        #(#observe_blocks)*
                    }
                }
            });
        }
    }
}
