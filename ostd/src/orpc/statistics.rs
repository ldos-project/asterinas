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
use crate::{early_println, orpc::oqueue::OQueueAttachError};

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
                            early_println!("Request observed: {}", outstanding);                    
                            let _ = outstanding_oqueue_producer.produce(outstanding);
                        },
                        if let _ = reply_observer.try_strong_observe() {
                            outstanding -= 1;
                            early_println!("Reply observed: {}", outstanding);                    
                            let _ = outstanding_oqueue_producer.produce(outstanding);
                        },
                        if let () = shutdown_observer.try_strong_observe() {}
                    );
                }
            }
        });

        Ok(server)
    }
}

// Implement the ORPC OQueue accessor via the macro-generated body. The method
// must be declared without a body (semicolon only) so the `orpc_impl` macro
// generates the accessor that returns the server's internal OQueue reference.
#[orpc_impl]
impl Outstanding for OutstandingCounter {
    fn outstanding_oqueue(&self) -> OQueueRef<isize>;
}

#[cfg(ktest)]
mod test {
    use ostd_macros::ktest;

    use super::*;
    use crate::{
        assert_eq_eventually,
        orpc::oqueue::{OQueue, locking::ObservableLockingQueue},
    };

    #[ktest]
    fn outstanding_counter_counts() {
        let request_oqueue = ObservableLockingQueue::new(8, 2);
        let reply_oqueue = ObservableLockingQueue::new(8, 2);

        let server =
            OutstandingCounter::spawn(request_oqueue.clone(), reply_oqueue.clone()).unwrap();

        let outstanding_observer = server.outstanding_oqueue().attach_weak_observer().unwrap();

        let request_producer = request_oqueue.attach_producer().unwrap();
        request_producer.produce(0usize);
        // TODO: Use weak notification instead.
        assert_eq_eventually!(
            {
                let recent = outstanding_observer.weak_observe_recent(2);
                recent.last().copied().unwrap_or(100)
            },
            1
        );

        request_producer.produce(0usize);
        assert_eq_eventually!(
            {
                let recent = outstanding_observer.weak_observe_recent(2);
                recent.last().copied().unwrap_or(100)
            },
            2
        );

        let reply_producer = reply_oqueue.attach_producer().unwrap();
        reply_producer.produce(0usize);
        assert_eq_eventually!(
            {
                let recent = outstanding_observer.weak_observe_recent(2);
                recent.last().copied().unwrap_or(100)
            },
            1
        );
    }
}


