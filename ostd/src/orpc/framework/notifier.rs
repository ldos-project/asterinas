// SPDX-License-Identifier: MPL-2.0

//! Arbitrary notifications via ORPC
use orpc_macros::orpc_trait;

use crate::orpc::{errors::RPCError, oqueue::OQueueRef};

/// ORPC trait for a simple notifier
#[orpc_trait]
pub trait Notifier {
    /// Notifies the server.
    fn notify(&self) -> Result<(), RPCError>;

    /// OQueue for listening to notifications.
    fn notification_oqueue(&self) -> OQueueRef<()> {
        OQueueRef::new(1, oqueue_path)
    }
}
