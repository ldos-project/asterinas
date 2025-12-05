// SPDX-License-Identifier: MPL-2.0

use ostd::orpc::{
    orpc_trait,
};

use ostd::Error;
use aster_block::BlockDevice;
use alloc::sync::Arc;

#[orpc_trait]
pub trait BlockDeviceSelectPolicy {
    /// Get the block device to read from. The policy cannot decide, for whatever reason, this should
    /// return an error. The caller will use some fallback. If the returned block device does not
    /// exist, then the caller will also fallback.
    fn select_block_device(&self) -> Result<Arc<dyn BlockDevice>, Error>;
}

pub use aster_virtio::device::block::server_traits::{
    ORPCBio, ORPCBioOQueues,
    PageIOObservable, PageIOObservableOQueues,
    OQueueSubmit, OQueueSubmitOQueues,
};