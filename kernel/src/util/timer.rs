// SPDX-License-Identifier: MPL-2.0
use alloc::{boxed::Box, string::ToString, sync::Arc};
use core::time::Duration;

use ostd::{
    orpc::{
        framework::{
            errors::RPCError,
            notifier::{Notifier, NotifierOQueues},
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
            Err(RPCError::Panic {
                message: "Could not produce into notification_oqueue".to_string(),
            })
        } else {
            Ok(())
        }
    }

    fn notification_oqueue(&self) -> OQueueRef<()>;
}

impl TimerServer {
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
