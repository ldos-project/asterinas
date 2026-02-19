// SPDX-License-Identifier: MPL-2.0

//! Small ORPC server utilities for observing OQueues.

extern crate alloc;

use alloc::sync::Arc;

use orpc_macros::{orpc_impl, orpc_server, orpc_trait};

use super::{
    errors::RPCError,
    framework::{
        shutdown::{self, ShutdownState},
        spawn_thread,
    },
    sync::select,
};
use crate::{
    new_server, orpc::{
        legacy_oqueue::OQueueAttachError,
        oqueue::{OQueue as _, OQueueBase as _, OQueueError, OQueueRef, ObservationQuery, StrongObserver},
        path::Path,
    }, path
};

/// An ORPC trait exposing an OQueue of outstanding request counts.
#[orpc_trait]
pub trait Outstanding {
    /// The OQueue that publishes the number of outstanding requests (requests - replies).
    fn outstanding_oqueue(&self) -> OQueueRef<isize> {
        OQueueRef::new(8, path!(outstanding[unique]))
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
        path: Path,
        request_observer: StrongObserver<()>,
        reply_observer: StrongObserver<()>,
    ) -> Result<Arc<Self>, OQueueError> {
        let server = new_server!(path.clone(), |_| Self {
            shutdown_state: ShutdownState::new(path),
        });

        spawn_thread(server.clone(), {
            let shutdown_observer = server
                .shutdown_state
                .shutdown_oqueue
                .attach_strong_observer(ObservationQuery::identity())?;
            let outstanding_oqueue_producer = server.outstanding_oqueue().attach_ref_producer()?;
            let server = server.clone();

            move || {
                let mut outstanding: isize = 0;
                loop {
                    server.shutdown_state.check()?;
                    select!(
                        if let _ = request_observer.try_strong_observe() {
                            outstanding += 1;
                            outstanding_oqueue_producer.produce_ref(&outstanding);
                        },
                        if let _ = reply_observer.try_strong_observe() {
                            outstanding -= 1;
                            outstanding_oqueue_producer.produce_ref(&outstanding);
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
