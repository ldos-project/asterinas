// SPDX-License-Identifier: MPL-2.0

//! Arbitrary notifications via ORPC
use alloc::string::ToString;

use orpc_macros::orpc_trait;

use crate::orpc::{
    framework::errors::RPCError,
    oqueue::{OQueueRef, ringbuffer::MPMCOQueue},
};

/// ORPC trait for a simple notifier
#[orpc_trait]
pub trait Notifier {
    /// Notifies the server.
    fn notify(&self) -> Result<(), RPCError>;

    /// OQueue for listening to notifications.
    fn notification_oqueue(&self) -> OQueueRef<()> {
        MPMCOQueue::<()>::new(1, 1)
    }
}
