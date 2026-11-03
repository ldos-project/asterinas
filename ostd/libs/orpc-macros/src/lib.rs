// SPDX-License-Identifier: MPL-2.0
//! Macros for OQueue elements, method tracing, and serialization.

mod element;
mod parsing_utils;
mod select;
mod trace_methods;
mod tuple_serialize;

use proc_macro::TokenStream;
use syn::{Item, parse_macro_input};

use crate::trace_methods::ImplOptions;

// TODO: The select syntax (and name) should be revisited. This provides a good starting point, but it also has some
// issues, such as rust-analyzer refactors not working and not having a clean way to match over different message forms.

/// Wait for one of multiple conditions to become true and execute the appropriate block.
///
/// The syntax is a series of if-let statements (separated by commas for technical reasons) where the RHS of each
/// binding is in the form `[blocker].fields_and_methods`. `select` will block waiting for any of the blockers and then
/// run all the if statements. The pattern must be irrefutable.
///
/// For example:
///
/// ```ignore
/// orpc_macros::select!(
///     if let msg = receiver1.try_produce() {
///         assert_eq!(msg.x, receiver1_counter);
///         receiver1_counter += 1;
///     },
///     if let TestMessage { x } = receiver2.try_produce() {
///         assert_eq!(x, receiver2_counter);
///         receiver2_counter += 1;
///     }
/// )
/// ```
///
/// NOTE: Keep the code inside the macro short and call into separate functions if possible. Inside macros IDE
/// assistance does not always work correctly and those tools are worth having for as much code as possible.
#[proc_macro]
pub fn select(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as select::SelectInput);
    let output = select::select_macro_impl(input);
    output.into()
}

/// Creates call and return OQueues for marked methods in a trait or impl.
///
/// ```ignore
/// #[trace_methods(visibility = pub(crate))]
/// trait Socket {
///     #[trace]
///     fn connect(&self, address: SocketAddr) -> Result<()>;
/// }
///
/// #[trace_methods(trait_module = crate::net)]
/// impl Socket for TcpSocket {
///     #[trace]
///     fn connect(&self, #[trace(only = call)] address: SocketAddr) -> Result<()> { /* ... */ }
/// }
/// ```
///
/// OQueues are named using: the crate, the module path, the type or trait name, the method, and
/// finally either `call` or `return`. For example, if the above declarations are in `net::socket`
/// in the `aster_core` crate (`kernel/core`), then the `connect` call OQueue will be
/// `aster_core.net.socket.Socket.connect.call`. For trait methods, the OQueues are named after the
/// *trait*, not the type implementing it (so all `impl`s share the same OQueues). For inherent
/// methods, their OQueues are named after the type.
///
/// The event types are in a `snake_case` module named after the trait or implementing type. They
/// are named in `CamelCase` based on the method name followed by `Call` or `Return`. For example,
/// the type are `net::socket::socket::ConnectCall` and `net::socket::socket::ConnectReturn` for
/// `connect`.
///
/// Fields are named based on argument names (from the `trait` declaration or the `impl` for
/// inherent methods). The names are the argument names without leading underscores (to make names
/// independent of if an argument is used), and `return_value` for the return value. Any field names
/// which conflict with Rust keywords have `_` appended (e.g., `self_`). For methods declared in a
/// trait, `self` has type `&dyn Trait`. For inherent methods, `self` has type `&Type`
///
/// Use `trait_module = ...` on a trait `impl` to give the path to the module containing the trait
/// when the trait path is unqualified.
///
/// Use `visibility = ...` to specify the visibility of the event types.
///
/// Arguments may be left out of either event with `#[trace(only = call)]` or
/// `#[trace(only = return)]`. An empty `#[trace(only = ())]` omits an argument from both events.
#[proc_macro_attribute]
pub fn trace_methods(attr: TokenStream, input: TokenStream) -> TokenStream {
    let options = parse_macro_input!(attr as ImplOptions);
    let item = parse_macro_input!(input as Item);
    let result = match item {
        Item::Impl(impl_block) => trace_methods::trace_methods_impl(options, impl_block),
        Item::Trait(trait_item) => trace_methods::trace_methods_trait(options, trait_item),
        item => Err(syn::Error::new_spanned(
            item,
            "trace_methods must be placed on a trait or impl",
        )),
    };
    result.unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Keeps traced methods unchanged in baseline mode, removing their tracing annotations.
#[proc_macro_attribute]
pub fn baseline_trace_methods(attr: TokenStream, input: TokenStream) -> TokenStream {
    let _options = parse_macro_input!(attr as ImplOptions);
    let item = parse_macro_input!(input as Item);
    trace_methods::baseline_trace_methods(item)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Implement `serde::Serialize` for a struct by serializing it as a tuple, instead of a map with
/// field names. This reduces the encoding overhead at the cost of the data no longer being fully
/// self describing. It has type information, but not field names.
///
/// All fields must implement `Serialize`, and serialization uses that impl. So, the serialized
/// value will not be a flat tuple, but instead of tuple of the other serializations which may be of
/// any type (tuple, record, etc).
///
/// # Example
///
/// ```ignore
/// #[derive(TupleSerialize)]
/// struct SimpleStruct {
///     field1: u64,
///     field2: u32,
/// }
/// ```
///
/// This will generate:
/// ```ignore
/// impl serde::Serialize for SimpleStruct {
///     fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
///         let mut tup = serializer.serialize_tuple(2)?;
///         tup.serialize_element(&self.field1)?;
///         tup.serialize_element(&self.field2)?;
///         tup.end()
///     }
/// }
/// ```
#[proc_macro_derive(TupleSerialize)]
pub fn tuple_serialize_derive(input: TokenStream) -> TokenStream {
    tuple_serialize::tuple_serialize_derive(input)
}

/// Derive macro for the `Element` trait.
///
/// This macro generates an `ElementDescriptor` for types with lifetime parameters,
/// or uses `LifetimelessElementDescriptor` for types without lifetime parameters.
#[proc_macro_derive(Element)]
pub fn element_derive(input: TokenStream) -> TokenStream {
    element::element_derive(input)
}
