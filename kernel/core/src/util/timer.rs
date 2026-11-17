// SPDX-License-Identifier: MPL-2.0
#![cfg(not(baseline_asterinas))]

use alloc::{boxed::Box, sync::Arc};
use core::time::Duration;

use ostd::{
    orpc::oqueue::{GenericOQueueRef, OQueue, OQueueError, OQueueRef},
    sync::WaitQueue,
};

use crate::prelude::WaitTimeout;

/// TimerServer periodically produces a notification with a configurable frequency.
pub(crate) struct TimerServer {
    freq: Duration,
    notification_oqueue: OQueueRef<()>,
}

impl TimerServer {
    fn notify(&self) -> Result<(), OQueueError> {
        self.notification_oqueue.produce_ref(&())
    }

    pub(crate) fn notification_oqueue(&self) -> OQueueRef<()> {
        self.notification_oqueue.clone()
    }

    /// Create and spawn a new TimerServer.
    pub(crate) fn spawn(freq: Duration) -> Arc<Self> {
        let notify_server = Self::new(freq);
        ostd::task::spawn_thread({
            let notify_server = notify_server.clone();
            move || notify_server.main()
        });
        notify_server
    }

    /// Create a TimerServer with the specified frequency.
    pub(crate) fn new(freq: Duration) -> Arc<Self> {
        Arc::new(Self {
            freq,
            notification_oqueue: GenericOQueueRef::new_anonymous(1),
        })
    }

    /// Start the server and periodically notify.
    pub(crate) fn main(&self) -> Result<(), Box<dyn core::error::Error>> {
        let sleep_queue = WaitQueue::new();
        loop {
            let _ = sleep_queue.wait_until_or_timeout(|| -> Option<()> { None }, &self.freq);
            self.notify()?;
        }
    }
}
