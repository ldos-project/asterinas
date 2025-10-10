/// A set of macros for use with ORPC. The most important are the ORPC attribute macros `orpc_trait`, `orpc_server`, and
/// `orpc_impl`. The `select` macro (for waiting on multiple OQueues) is also defined here.

mod orpc_impl;
mod orpc_server;
mod orpc_trait;
mod parsing_utils;
mod select;

use proc_macro::TokenStream;
use syn::{
    ItemImpl, ItemStruct, ItemTrait, Path, Token, parse_macro_input, punctuated::Punctuated,
};

/// Declare a trait as an ORPC trait that can be implemented by ORPC server.
///
/// Members must come in one of two forms:
///
/// `fn name(&self, arg: Arg) -> Result<Ret, SomeError>` defines an RPC method which is implemented using a normal
/// method body in the `impl` of this trait. The method takes at most one argument which must be an `Observable` type.
/// It returns a `Result<R, E>` with `R` being observable and `E` implementing `From<RPCError>`. (An easy way to do this
/// us to use `Snafu` and have a [transparent
/// variant](https://docs.rs/snafu/latest/snafu/derive.Snafu.html#delegating-to-the-underlying-error) for
/// `orpc::errors::RPCError`.)
///
/// `fn name(&self) -> OQueueRef<Msg>` defines an OQueue which is exported from the server to be directly used by
/// clients of the server. These methods must have a "default implementation" which returns a new OQueue of the correct
/// type. This will be run only when a server is constructed. This requirement will be relaxed once the OQueue
/// implementation matures to the point that there is a universal OQueue implementation.
///
/// ### Example
///
/// ```ignore
/// #[orpc_trait]
/// trait Counter {
///     fn atomic_incr(&self, additional: AdditionalAmount) -> Result<usize, RPCError>;
///     fn incr_oqueue(&self) -> OQueueRef<AdditionalAmount> {
///         LockingQueue::new(8)
///     }
/// }
/// ```
///
/// ### Implementation
///
/// This macro wil generate a struct `[trait name]OQueues` which contains all the OQueue references used by the trait.
/// This will include both explicitly exported OQueues and implicit OQueues associated with functions. This structure
/// should never be used directly.
#[proc_macro_attribute]
pub fn orpc_trait(attr: TokenStream, input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as ItemTrait);
    let output = orpc_trait::orpc_trait_macro_impl(attr, input);
    output.into()
}

/// Declare an ORPC server. This attribute is applied to the struct which holds the servers state private state. This
/// struct is the type used to represent the server and will implement all its ORPC traits.
///
/// This attribute takes an argument which is a list of all the ORPC traits that the server implements.
///
/// (NOTE: Ideally, traits could be implemented separately from the main struct, however this is not possible because
/// the servers internal state must include state associated with each trait.)
///
/// ### Example
///
/// ```ignore
/// #[orpc_server(Counter, IsReady)]
/// struct ServerAState {
///     increment: usize,
///     atomic_count: AtomicUsize,
/// }
/// ```
///
/// ### Implementation
///
/// This macro will generate an additional field in the struct called `orpc_internal` and its type,
/// `[struct name]ORPCInternal`. These contain all the internal information for the ORPC framework to implement the
/// calls and OQueue access. None of this should be accessed directly by the user.
#[proc_macro_attribute]
pub fn orpc_server(attr: TokenStream, input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as ItemStruct);
    let args = parse_macro_input!(attr with Punctuated::<Path, Token!(,)>::parse_terminated);
    let output = orpc_server::orpc_server_macro_impl(args, input);
    output.into()
}

/// Implement a specific ORPC trait for a server. This attribute is applies to an `impl` of an ORPC trait for a server
/// struct. RPC methods are implemented just as they would be for a non-ORPC trait. The methods which declare OQueues
/// should be included here without a body just as they are declared in the original trait.
///
/// NOTE: Due to this being a macro, you may run into development environment issue inside the `impl`. It can be useful
/// to temporarily comment out the attribute to perform refactors or view error messages more clearly.
///
/// ### Example
///
/// ```ignore
/// #[orpc_impl]
/// impl Counter for ServerAState {
///     fn atomic_incr(&self, additional: AdditionalAmount) -> Result<usize, RPCError> {
///         if additional.trigger_panic {
///             panic!("Asked to panic");
///         }
///
///         let addend = self.increment + additional.n;
///         // collect!(addend, addend);
///         let v = self.atomic_count.fetch_add(addend, Ordering::Relaxed);
///         Ok(v + addend)
///     }
///
///     fn incr_oqueue(&self) -> OQueueRef<AdditionalAmount>;
/// }
/// ```
///
/// ### Implementation
///
/// This macro generates the correct implementations including error handling for RPC methods and generates OQueue
/// accessors.
#[proc_macro_attribute]
pub fn orpc_impl(attr: TokenStream, input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as ItemImpl);
    let output = orpc_impl::orpc_impl_macro_impl(attr, input);
    output.into()
}

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
///     if let msg = receiver1.try_receive() {
///         assert_eq!(msg.x, receiver1_counter);
///         receiver1_counter += 1;
///     },
///     if let TestMessage { x } = receiver2.try_receive() {
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
