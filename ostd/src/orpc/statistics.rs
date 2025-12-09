// SPDX-License-Identifier: MPL-2.0

//! Small ORPC server utilities for observing OQueues.

extern crate alloc;

use alloc::sync::Arc;

use orpc_macros::{orpc_impl, orpc_server, orpc_trait};

use super::{
    framework::{
        errors::RPCError,
        shutdown::{self, ShutdownState},
        spawn_thread,
    },
    oqueue::{OQueueRef, locking::ObservableLockingQueue},
    sync::select,
};
use crate::orpc::oqueue::OQueueAttachError;

/// An ORPC trait exposing an OQueue of outstanding request counts.
#[orpc_trait]
pub trait Outstanding {
    /// The OQueue that publishes the number of outstanding requests (requests - replies).
    fn outstanding_oqueue(&self) -> OQueueRef<isize> {
        ObservableLockingQueue::new(4, 8)
    }
}

/// Observes a request and a reply OQueue and publishes the outstanding count.
#[orpc_server(Outstanding, shutdown::Shutdown)]
pub struct OutstandingCounter {
    shutdown_state: ShutdownState,
}

#[orpc_impl]
impl shutdown::Shutdown for OutstandingCounter {
    fn shutdown(&self) -> core::result::Result<(), RPCError> {
        self.shutdown_state.shutdown();
        Ok(())
    }
}

impl OutstandingCounter {
    /// Spawn a new `OutstandingCounter` server which observes `request_oqueue` and
    /// `reply_oqueue`.
    pub fn spawn<T: 'static, U: 'static>(
        request_oqueue: OQueueRef<T>,
        reply_oqueue: OQueueRef<U>,
    ) -> Result<Arc<Self>, OQueueAttachError> {
        let server = Self::new_with(|orpc_internal, _| Self {
            orpc_internal,
            shutdown_state: Default::default(),
        });

        spawn_thread(server.clone(), {
            let request_observer = request_oqueue.attach_strong_observer()?;
            let reply_observer = reply_oqueue.attach_strong_observer()?;
            let shutdown_observer = server
                .shutdown_state
                .shutdown_oqueue
                .attach_strong_observer()?;
            let outstanding_oqueue_producer = server.outstanding_oqueue().attach_producer()?;
            let server = server.clone();

            move || {
                let mut outstanding: isize = 0;
                loop {
                    server.shutdown_state.check()?;
                    select!(
                        if let _ = request_observer.try_strong_observe() {
                            outstanding += 1;
                            outstanding_oqueue_producer.produce(outstanding);
                        },
                        if let _ = reply_observer.try_strong_observe() {
                            outstanding -= 1;
                            outstanding_oqueue_producer.produce(outstanding);
                        },
                        if let () = shutdown_observer.try_strong_observe() {}
                    );
                }
            }
        });

        Ok(server)
    }
}

#[orpc_impl]
impl Outstanding for OutstandingCounter {
    fn outstanding_oqueue(&self) -> OQueueRef<isize>;
}
