#![allow(unused)]
//! Several prefetching policies that operate on a [`BlockCache`].

use std::error::Error;

use orpc::{CurrentServer, ServerRef, oqueue::registry, orpc_impl, orpc_server, spawn_thread};
use snafu::{FromString, OptionExt, ResultExt, Whatever, whatever};

use crate::{
    block_device::BlockCache,
    shutdown::{Shutdown, ShutdownState},
};

/// A policy which always prefetches a page a fixed distance in the future.
#[orpc_server(crate::shutdown::Shutdown)]
pub struct NextPagePrefetcher {
    shutdown: ShutdownState,
}

#[orpc_impl]
impl Shutdown for NextPagePrefetcher {
    // An implementation of a method on the Shutdown trait
    fn shutdown(&self) -> Result<(), orpc_impl::errors::RPCError> {
        self.shutdown.shutdown()
    }
}

impl NextPagePrefetcher {
    pub fn spawn(cache: ServerRef<dyn BlockCache>) -> Result<ServerRef<Self>, Box<dyn Error>> {
        let server = Self::new_with(|orpc_internal| Self {
            orpc_internal,
            shutdown: Default::default(),
        });
        // Spawn a server thread
        spawn_thread(server.clone(), {
            // Setup OQueue connections.
            let read_observer = cache
                .read_request_observation_oqueue()
                .attach_strong_observer()?;
            let prefetch_sender = cache.prefetch_request_oqueue().attach_sender()?;
            let server = server.clone();
            // Actual Server Loop.
            move || {
                loop {
                    let block_id = read_observer.strong_observe();

                    prefetch_sender.send(block_id + 1);
                    CurrentServer::abort_point();
                    server.shutdown.check_for_shutdown()?;
                }
            }
        });
        Ok(server)
    }
}

/// A prefetcher which prefetches using a strong observer. It will always generate one prefetch for every read. It
/// selects the block to prefetch based on the stride between the previous two reads.
#[orpc_server(crate::shutdown::Shutdown)]
pub struct StridePrefetcher {
    shutdown: ShutdownState,
}

#[orpc_impl]
impl Shutdown for StridePrefetcher {
    // An implementation of a method on the Shutdown trait
    fn shutdown(&self) -> Result<(), orpc_impl::errors::RPCError> {
        self.shutdown.shutdown()
    }
}

impl StridePrefetcher {
    pub fn spawn(cache: ServerRef<dyn BlockCache>) -> Result<ServerRef<Self>, Box<dyn Error>> {
        let server = Self::new_with(|orpc_internal| Self {
            orpc_internal,
            shutdown: Default::default(),
        });
        spawn_thread(server.clone(), {
            // Setup OQueue Attachments
            let strong_observer = cache
                .read_request_observation_oqueue()
                .attach_strong_observer()?;
            let weak_observer = cache
                .read_request_observation_oqueue()
                .attach_weak_observer()?;
            let prefetch_sender = cache.prefetch_request_oqueue().attach_sender()?;
            let server = server.clone();
            // Server implementation
            move || {
                loop {
                    // Wait for the next read.
                    let block_id = strong_observer.strong_observe();

                    // Get the last two
                    let reads = weak_observer.weak_observe_recent(2);

                    // Prefetch algorithm
                    if reads.len() == 2 {
                        let stride = reads[1] - reads[0];
                        prefetch_sender.send(block_id + stride);
                    }

                    // Provide point for exiting the server if it crashed.
                    CurrentServer::abort_point();
                    server.shutdown.check_for_shutdown()?;
                }
            }
        });
        Ok(server)
    }
}

#[orpc_server(crate::shutdown::Shutdown)]
pub struct StrideLoadAwarePrefetcher {
    shutdown: ShutdownState,
}

#[orpc_impl]
impl Shutdown for StrideLoadAwarePrefetcher {
    // An implementation of a method on the Shutdown trait
    fn shutdown(&self) -> Result<(), orpc_impl::errors::RPCError> {
        self.shutdown.shutdown()
    }
}

impl StrideLoadAwarePrefetcher {
    pub fn spawn(cache: ServerRef<dyn BlockCache>) -> Result<ServerRef<Self>, Box<dyn Error>> {
        let server = Self::new_with(|orpc_internal| Self {
            orpc_internal,
            shutdown: Default::default(),
        });
        spawn_thread(server.clone(), {
            // Setup OQueue Attachments
            let strong_observer = cache
                .read_request_observation_oqueue()
                .attach_strong_observer()?;
            let weak_observer = cache
                .read_request_observation_oqueue()
                .attach_weak_observer()?;
            let prefetch_sender = cache.prefetch_request_oqueue().attach_sender()?;
            let system_load_observer = registry::lookup::<u8>("system_load")
                .unwrap_or_whatever()?
                .attach_weak_observer()?;
            let server = server.clone();
            // Server implementation
            move || {
                loop {
                    // Wait for the next read.
                    let block_id = strong_observer.strong_observe();

                    // Get the last two
                    let reads = weak_observer.weak_observe_recent(2);

                    // Prefetch algorithm
                    if reads.len() == 2 {
                        // Get the system load
                        let system_load = system_load_observer
                            .weak_observe_recent(1)
                            .pop()
                            .unwrap_or_default() as usize;
                        // Prefetch farther ahead when load is lower.
                        let steps = ((255 - system_load) / 64).max(1);
                        let stride = reads[1] - reads[0];
                        prefetch_sender.send(block_id + (stride * steps));
                    }

                    // Provide point for exiting the server if it crashed.
                    CurrentServer::abort_point();
                    server.shutdown.check_for_shutdown()?;
                }
            }
        });
        Ok(server)
    }
}

trait UnwrapOrWhateverExt<T> {
    fn unwrap_or_whatever(self) -> Result<T, Whatever>;
}

impl<T> UnwrapOrWhateverExt<T> for Option<T> {
    fn unwrap_or_whatever(self) -> Result<T, Whatever> {
        match self {
            Some(v) => Ok(v),
            None => Err(whatever!("Unwrapped None")),
        }
    }
}
