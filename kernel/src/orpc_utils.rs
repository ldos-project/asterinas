use alloc::{boxed::Box, sync::Arc};

use log::{error, info};
use ostd::orpc::framework::{CurrentServer, Server, errors};

use crate::thread::kernel_thread::ThreadOptions;

/// Start a new server thread. This should only be called while spawning a server.
pub fn spawn_thread<T: Server + Send + 'static>(
    server: Arc<T>,
    body: impl (FnOnce() -> Result<(), Box<dyn core::error::Error>>) + Send + 'static,
) {
    let _ = ThreadOptions::new(move || {
        if let Result::Err(payload) = ostd::panic::catch_unwind({
            let server = server.clone();
            move || {
                Server::orpc_server_base(server.as_ref()).attach_task();
                let _server_context =
                    CurrentServer::enter_server_context(server.orpc_server_base());
                if let Result::Err(e) = body() {
                    info!("Server thread ({}) exited: {}", server.orpc_server_base(), e);
                    Server::orpc_server_base(server.as_ref()).abort(&e);
                }
            }
        }) {
            let err = errors::RPCError::from_panic(payload);
            error!("Server thread ({}) panicked: {}", server.orpc_server_base(), err);
            Server::orpc_server_base(server.as_ref()).abort(&err);
        }
    })
    .spawn();
}
