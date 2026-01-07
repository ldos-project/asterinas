// SPDX-License-Identifier: MPL-2.0

//! Simple prefetcher policies
//!
//! This module contains simple prefetcher policies for for testing and as examples. These are not
//! designed to be particularly useful and are definitely not well thought out.

// TODO(arthurp, https://github.com/ldos-project/asterinas/issues/118): Replace these policies with
// real heuristics.

use alloc::{boxed::Box, sync::Arc};
use core::ops::Range;

use ostd::orpc::{
    framework::{
        errors::RPCError,
        shutdown::{self, ShutdownState},
        spawn_thread,
    },
    legacy_oqueue::{StrongObserver, WeakObserver},
    orpc_impl, orpc_server,
    statistics::{Outstanding, OutstandingCounter},
    sync::select,
};

use crate::{
    Result,
    fs::server_traits::{PageCache, PageIOObservable},
};

/// A prefetcher which implements the policy originally provided by Asterinas.
///
/// The policy is: If there are no outstanding reads and the most recent read was sequential
/// (exactly stride 1), read pages ahead. The number of pages read starts at some `min` and doubles
/// every time there is another prefetch (sequential read with no outstanding reads). Unlike the
/// original, this resets when there is a non-sequential read, and can restart when another
/// sequential sequence starts.
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
        max_window_size: usize,
        initial_window_size: usize,
    ) -> Result<Arc<Self>> {
        let server = Self::new_with(|orpc_internal, _| Self {
            orpc_internal,
            shutdown_state: Default::default(),
        });

        let underlying_page_store = cache.underlying_page_store()?;
        // TODO: Tie this to the lifetime of `server`.
        let outstanding_counter = OutstandingCounter::spawn(
            underlying_page_store.page_reads_oqueue(),
            underlying_page_store.page_reads_reply_oqueue(),
        )?;

        spawn_thread(server.clone(), {
            let read_observer = cache.page_reads_oqueue().attach_strong_observer()?;
            let read_weak_observer = cache.page_reads_oqueue().attach_weak_observer()?;
            let outstanding_count_observer = outstanding_counter
                .outstanding_oqueue()
                .attach_weak_observer()?;
            let shutdown_observer = server
                .shutdown_state
                .shutdown_oqueue
                .attach_strong_observer()?;
            let cache: Arc<dyn PageCache> = cache.clone();
            let server = server.clone();

            struct PrefetcherState {
                read_observer: Box<dyn StrongObserver<usize>>,
                read_weak_observer: Box<dyn WeakObserver<usize>>,
                outstanding_count_observer: Box<dyn WeakObserver<isize>>,
                shutdown_observer: Box<dyn StrongObserver<()>>,
                cache: Arc<dyn PageCache>,
                server: Arc<ReadaheadPrefetcher>,
                window: Option<Range<usize>>,
                max_window_size: usize,
                initial_window_size: usize,
            }

            let mut state = PrefetcherState {
                read_observer,
                read_weak_observer,
                outstanding_count_observer,
                shutdown_observer,
                cache,
                server,
                window: Default::default(),
                max_window_size,
                initial_window_size,
            };

            impl PrefetcherState {
                fn run(&mut self) -> Result<()> {
                    loop {
                        self.server.shutdown_state.check()?;
                        select! {
                            if let idx = self.read_observer.try_strong_observe() {
                                self.maybe_prefetch(idx)?;
                            },
                            if let () = self.shutdown_observer.try_strong_observe() {}
                        };
                    }
                }

                fn update_window(&mut self, idx: usize) {
                    if let Some(window) = &mut self.window {
                        let next_start = window.end;
                        let next_size = (window.len() * 2).min(self.max_window_size);
                        *window = next_start..(next_start + next_size);
                    } else {
                        self.window = Some(idx..(idx + self.initial_window_size));
                    }
                }

                fn maybe_prefetch(&mut self, idx: usize) -> Result<()> {
                    if let Some(outstanding) = self
                        .outstanding_count_observer
                        .weak_observe_recent(1)
                        .last()
                    {
                        let is_sequential = if let Some([a, b]) =
                            self.read_weak_observer.weak_observe_recent(2).last_chunk()
                        {
                            a + 1 == *b
                        } else {
                            false
                        };

                        let has_used_prefetched = if let Some(window) = &self.window {
                            idx == window.start || idx == window.end
                        } else {
                            true
                        };
                        if is_sequential && has_used_prefetched {
                            self.update_window(idx);
                            if *outstanding == 0 {
                                for i in self.window.as_ref().unwrap().clone() {
                                    self.cache.prefetch_oqueue().produce(i)?;
                                }
                            }
                        }
                        if !is_sequential {
                            self.window = None;
                        }
                    }
                    Ok(())
                }
            }

            move || state.run().map_err(|v| v.into())
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
