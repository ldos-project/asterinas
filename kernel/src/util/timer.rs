// SPDX-License-Identifier: MPL-2.0
use alloc::{boxed::Box, string::ToString, sync::Arc};
use core::time::Duration;

use ostd::{
    orpc::{
        framework::{
            errors::RPCError,
            notifier::{Notifier, NotifierOQueues},
            spawn_thread,
        },
        oqueue::OQueueRef,
        orpc_impl, orpc_server,
    },
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
        if self.notification_oqueue().produce(()).is_err() {
            panic!("Could not produce into notification_oqueue")
        } else {
            Ok(())
        }
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
        let server = Self::new_with(|orpc_internal, _| Self {
            orpc_internal,
            freq,
        });
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
