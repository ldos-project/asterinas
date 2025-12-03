// SPDX-License-Identifier: MPL-2.0

//! Simple prefetcher policies
//!
//! This module contains simple prefetcher policies for for testing and as examples. These are not
//! designed to be particularly useful and are definitely not well thought out.

// TODO(arthurp, https://github.com/ldos-project/asterinas/issues/118): Replace these policies with
// real heuristics.

use alloc::sync::Arc;

use ostd::orpc::{
    framework::{
        errors::RPCError,
        shutdown::{self, ShutdownState},
        spawn_thread,
    },
    orpc_impl, orpc_server,
    sync::select,
};

use crate::{
    Result,
    fs::server_traits::{PageCache, PageIOObservable},
};

/// A test prefetcher that always prefetches page `idx + n` when page `idx` is read. `n` is the
/// number of pages the prefetcher should stay ahead of the reader. `n` is fixed at construction
/// time.
///
/// NOTE: This is not a good or realistic prefetcher. It is designed as an example.
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
    pub fn spawn(
        cache: Arc<impl PageCache + PageIOObservable>,
        n_steps_ahead: usize,
    ) -> Result<Arc<Self>> {
        let server = Self::new_with(|orpc_internal, _| Self {
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
                loop {
                    server.shutdown_state.check()?;
                    select!(
                        if let idx = read_observer.try_strong_observe() {
                            cache.prefetch(idx + n_steps_ahead)?;
                        },
                        if let () = shutdown_observer.try_strong_observe() {}
                    );
                }
            }
        });

        Ok(server)
    }
}

/// A test prefetcher that prefetches page `idx + stride*n` when page `idx` is read. `stride` is the
/// distance between the previous two reads. `n` is the number of steps the prefetcher should stay
/// ahead of the reader. `n` is fixed at construction time.
///
/// NOTE: This is not a good or realistic prefetcher. It is designed as an example.
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
    pub fn spawn(
        cache: Arc<impl PageCache + PageIOObservable>,
        n_steps_ahead: usize,
    ) -> Result<Arc<Self>> {
        let server = Self::new_with(|orpc_internal, _| Self {
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
                loop {
                    server.shutdown_state.check()?;
                    select!(
                        if let idx = read_observer.try_strong_observe() {
                            let recent = read_weak_observer.recent_cursor();
                            let history = read_weak_observer.weak_observe_range(recent - 1, recent);
                            if history.len() >= 2 {
                                let stride = history[1] - history[0];
                                cache.prefetch(idx + stride * n_steps_ahead)?;
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
