// SPDX-License-Identifier: MPL-2.0

use alloc::sync::Arc;

use aster_logger::println;
use ostd::orpc::{
    framework::{errors::RPCError, shutdown, spawn_thread},
    oqueue::{OQueueAttachError, OQueueRef},
    orpc_impl, orpc_server,
    sync::select,
};

use crate::fs::server_traits::PageCacheReadInfo;

/// A server to monitor and log the cache activity of a [`crate::fs::server_traits::PageCache`].
#[orpc_server(shutdown::Shutdown)]
pub struct PageCacheLogger {
    shutdown_state: shutdown::ShutdownState,
}

#[orpc_impl]
impl shutdown::Shutdown for PageCacheLogger {
    fn shutdown(&self) -> Result<(), RPCError> {
        self.shutdown_state.shutdown();
        Ok(())
    }
}

impl PageCacheLogger {
    pub fn spawn(
        page_cache_read_info_oqueue: OQueueRef<PageCacheReadInfo>,
    ) -> Result<Arc<Self>, OQueueAttachError> {
        let server = Self::new_with(|orpc_internal, _| Self {
            orpc_internal,
            shutdown_state: Default::default(),
        });

        spawn_thread(server.clone(), {
            let read_obs = page_cache_read_info_oqueue.attach_strong_observer()?;
            let shutdown_obs = server
                .shutdown_state
                .shutdown_oqueue
                .attach_strong_observer()?;
            let server = server.clone();

            move || {
                loop {
                    server.shutdown_state.check()?;
                    select!(
                        if let info = read_obs.try_strong_observe() {
                            println!("{:?}", info);
                        },
                        if let () = shutdown_obs.try_strong_observe() {}
                    );
                }
            }
        });

        Ok(server)
    }
}
