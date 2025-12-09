use alloc::string::ToString;

use orpc_macros::orpc_trait;

use crate::orpc::{
    framework::errors::RPCError,
    oqueue::{OQueueRef, ringbuffer::MPMCOQueue},
};

/// ORPC trait for a simple notifier
#[orpc_trait]
pub trait Notifier {
    fn notify(&self) -> Result<(), RPCError>;

    fn notification_oqueue(&self) -> OQueueRef<()> {
        MPMCOQueue::<()>::new(1, 1)
    }
}
