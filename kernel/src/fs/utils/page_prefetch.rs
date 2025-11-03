use alloc::{format, sync::{Arc, Weak}};

use aster_logger::println;
use ostd::orpc::{
    framework::{
        Server,
        errors::RPCError,
        shutdown::{self, ShutdownState},
    },
    oqueue::StrongObserver,
    orpc_impl, orpc_server,
    sync::select,
};

use crate::{
    Result,
    fs::server_traits::{PageCache, PageIOObservable},
    orpc_utils::spawn_thread,
};

#[orpc_server(shutdown::Shutdown)]
pub struct ReadaheadPrefetcher {
    shutdown_state: ShutdownState,
}

#[orpc_impl]
impl shutdown::Shutdown for ReadaheadPrefetcher {
    fn shutdown(&self) -> core::result::Result<(), RPCError> {
        self.shutdown_state.shutdown();
        Ok(())
    }
}

impl ReadaheadPrefetcher {
    pub fn spawn(cache: Arc<impl PageCache + PageIOObservable>) -> Result<Arc<Self>> {
        let server = Self::new_with(format!("{}.cache", cache.path()), |orpc_internal, _| Self {
            orpc_internal,
            shutdown_state: Default::default(),
        });

        spawn_thread(server.clone(), {
            let read_observer = cache.page_reads_oqueue().attach_strong_observer()?;
            let shutdown_observer = server
                .shutdown_state
                .shutdown_oqueue
                .attach_strong_observer()?;
            let cache: Arc<dyn PageCache> = cache.clone();
            let server = server.clone();

            move || {
                println!(
                    "readahead thread {} (for cache: {})",
                    server.path(),
                    cache.orpc_server_base()
                );
                loop {
                    server.shutdown_state.check()?;
                    select!(
                        if let idx = read_observer.try_strong_observe() {
                            println!("Prefetching based on {}", idx);
                            cache.prefetch(idx + 4)?;
                        },
                        if let () = shutdown_observer.try_strong_observe() {}
                    );
                }
            }
        });

        Ok(server)
    }
}

#[orpc_server(shutdown::Shutdown)]
pub struct StridedPrefetcher {
    shutdown_state: ShutdownState,
}

#[orpc_impl]
impl shutdown::Shutdown for StridedPrefetcher {
    fn shutdown(&self) -> core::result::Result<(), RPCError> {
        self.shutdown_state.shutdown();
        Ok(())
    }
}

impl StridedPrefetcher {
    pub fn spawn(cache: Arc<impl PageCache + PageIOObservable>) -> Result<Arc<Self>> {
        let server = Self::new_with(format!("{}.cache", cache.path()), |orpc_internal, _| Self {
            orpc_internal,
            shutdown_state: Default::default(),
        });

        spawn_thread(server.clone(), {
            let read_observer = cache.page_reads_oqueue().attach_strong_observer()?;
            let read_weak_observer = cache.page_reads_oqueue().attach_weak_observer()?;
            let shutdown_observer = server
                .shutdown_state
                .shutdown_oqueue
                .attach_strong_observer()?;
            let cache: Arc<dyn PageCache> = cache.clone();
            let server = server.clone();

            move || {
                println!(
                    "Strided thread {} (for cache: {})",
                    server.path(),
                    cache.orpc_server_base()
                );
                loop {
                    server.shutdown_state.check()?;
                    select!(
                        if let idx = read_observer.try_strong_observe() {
                            let recent = read_weak_observer.recent_cursor();
                            let history = read_weak_observer.weak_observe_range(recent - 2, recent);
                            if history.len() >= 2 {
                                let stride = history[1] - history[0];
                                println!("Prefetching based on {} and stride {}", idx, stride);
                                cache.prefetch(idx + stride*4)?;
                            }
                        },
                        if let () = shutdown_observer.try_strong_observe() {}
                    );
                }
            }
        });

        Ok(server)
    }
}
