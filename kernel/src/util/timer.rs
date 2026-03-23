// SPDX-License-Identifier: MPL-2.0
#![cfg(not(baseline_asterinas))]

use alloc::{boxed::Box, sync::Arc};
use core::time::Duration;

use ostd::{
    new_server,
    orpc::{
        errors::RPCError,
        framework::{
            notifier::{Notifier, NotifierOQueues},
            spawn_thread,
        },
        oqueue::{OQueue as _, OQueueRef},
        orpc_impl, orpc_server,
    },
    path,
    sync::WaitQueue,
};
use snafu::Whatever;

use crate::prelude::WaitTimeout;

/// TimerServer periodically produces a notification with a configurable frequency.
#[orpc_server(Notifier)]
pub struct TimerServer {
    freq: Duration,
}

#[orpc_impl]
impl Notifier for TimerServer {
    fn notify(&self) -> Result<(), RPCError> {
        self.notification_oqueue()
            .attach_ref_producer()
            .expect("Could not attach producer to notification_oqueue")
            .produce_ref(&());
        Ok(())
    }

    fn notification_oqueue(&self) -> OQueueRef<()>;
}

impl TimerServer {
    /// Create and spawn a new TimerServer.
    pub fn spawn(freq: Duration) -> Arc<Self> {
        let notify_server = Self::new(freq).unwrap();
        spawn_thread(notify_server.clone(), {
            let notify_server = notify_server.clone();
            move || notify_server.main()
        });
        notify_server
    }

    /// Create a TimerServer with the specified frequency.
    pub fn new(freq: Duration) -> Result<Arc<Self>, Whatever> {
        let server = new_server!(path!(timer[unique]), |_| Self { freq });
        Ok(server)
    }

    /// Start the server and periodically notify.
    pub fn main(&self) -> Result<(), Box<dyn core::error::Error>> {
        let sleep_queue = WaitQueue::new();
        loop {
            let _ = sleep_queue.wait_until_or_timeout(|| -> Option<()> { None }, &self.freq);
            self.notify()?;
        }
    }
}
