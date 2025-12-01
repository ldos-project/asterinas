// SPDX-License-Identifier: MPL-2.0

//! Simple prefetcher policies
//!
//! This module contains simple prefetcher policies for for testing and as examples. These are not
//! designed to be particularly useful and are definitely not well thought out.

// TODO(arthurp, https://github.com/ldos-project/asterinas/issues/118): Replace these policies with
// real heuristics.

use alloc::sync::Arc;
use core::usize;

use aster_logger::println;
use ostd::orpc::{
    framework::{
        errors::RPCError,
        shutdown::{self, ShutdownState},
    },
    orpc_impl, orpc_server,
    sync::select,
};

use crate::{
    Result,
    fs::server_traits::{PageCache, PageIOObservable},
    orpc_utils::spawn_thread,
};

/// A prefetcher which implements the policy originally provided by Asterinas.
///
/// The policy is: If there are no outstanding reads and the most recent read was sequential
/// (exactly stride 1), read pages ahead. The number of pages read starts at some `min` and doubles
/// every time there is another prefetch (sequential read with no outstanding reads).
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
            let read_observer = cache.page_reads_oqueue().attach_strong_observer().unwrap();
            let underlying_read_observer = cache
                .underlying_page_store()
                .unwrap()
                .page_reads_oqueue()
                .attach_strong_observer()
                .unwrap();
            let underlying_read_reply_observer = cache
                .underlying_page_store()
                .unwrap()
                .page_reads_reply_oqueue()
                .attach_strong_observer()
                .unwrap();
            let shutdown_observer = server
                .shutdown_state
                .shutdown_oqueue
                .attach_strong_observer()?;
            let cache: Arc<dyn PageCache> = cache.clone();
            let server = server.clone();

            move || {
                let mut outstanding_reads = 0i64;
                loop {
                    server.shutdown_state.check()?;
                    select!(
                        if let idx = underlying_read_observer.try_strong_observe() {
                            outstanding_reads += 1;
                            // println!("+ {idx} outstanding reads = {}", outstanding_reads);
                        },
                        if let idx = underlying_read_reply_observer.try_strong_observe() {
                            outstanding_reads -= 1;
                            // println!("- {idx} outstanding reads = {}", outstanding_reads);
                        },
                        if let idx = read_observer.try_strong_observe() {
                            // println!("read");
                            if outstanding_reads < 2 {
                                let res = cache.prefetch_oqueue().produce(idx + n_steps_ahead);
                                // println!("issue prefetch {}: {}", idx + n_steps_ahead, res.is_ok_and(|v| v.is_none()));
                                // println!("issue prefetch {}", idx + n_steps_ahead);
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
                                cache
                                    .prefetch_oqueue()
                                    .produce(idx + stride * n_steps_ahead)?;
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
