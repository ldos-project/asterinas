// SPDX-License-Identifier: MPL-2.0

//! The framework for starting threads associated with a server.
//!
//! You should consider if using a thread is correct, or if you should use the message handler
//! framework. The message handler framework is more flexible and declarative.

use log::error;
use spin::Once;

use crate::{
    orpc::framework::{CurrentServer, Server, errors},
    prelude::{Arc, Box},
    task::TaskOptions,
};

/// The body of a ORPC thread as a closure.
pub type ThreadMain = Box<dyn FnOnce() -> Result<(), Box<dyn core::error::Error>> + Send + 'static>;

/// The type of the function used to implement the `spawn_thread` function.
pub(crate) type SpawnThreadFn = fn(Arc<dyn Server + Send + Sync + 'static>, ThreadMain);

/// Injected function for spawning new threads. See [`inject_spawn_thread`].
pub(crate) static SPAWN_THREAD_FN: Once<SpawnThreadFn> = Once::new();

/// Start a new server thread. This should only be called while spawning a server.
pub fn spawn_thread<T: Server>(
    server: Arc<T>,
    body: impl (FnOnce() -> Result<(), Box<dyn core::error::Error>>) + Send + 'static,
) {
    if let Some(spawn_fn) = SPAWN_THREAD_FN.get() {
        spawn_fn(server, Box::new(body));
        return;
    }

    TaskOptions::new(wrap_server_thread_body(server, Box::new(body)))
        .spawn()
        .unwrap();
}

/// Return a closure wrapping the body of a server thread with the machinery to setup the execution
/// context and catch and handle errors.
///
/// This should only be used by spawn_thread implementations which are injected using
/// [`inject_spawn_thread`].
pub fn wrap_server_thread_body(
    server: Arc<dyn Server + Send + Sync + 'static>,
    body: ThreadMain,
) -> impl FnOnce() {
    move || {
        if let Result::Err(payload) = crate::panic::catch_unwind({
            let server = server.clone();
            move || {
                Server::orpc_server_base(server.as_ref()).attach_task();
                let _server_context =
                    CurrentServer::enter_server_context(server.orpc_server_base());
                if let Result::Err(e) = body() {
                    Server::orpc_server_base(server.as_ref()).abort(&e);
                }
            }
        }) {
            let err = errors::RPCError::from_panic(payload);
            error!("Server thread panicked: {}", err);
            Server::orpc_server_base(server.as_ref()).abort(&err);
        }
    }
}

/// Set a custom function for spawning threads. This allows overriding the default thread spawning
/// behavior. This is required in kernels, like Asterinas, that do not run raw OSTD [`Task`]s
/// correctly.
pub fn inject_spawn_thread(func: SpawnThreadFn) {
    SPAWN_THREAD_FN.call_once(|| func);
}
