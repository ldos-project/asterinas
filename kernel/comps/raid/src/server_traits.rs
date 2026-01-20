// SPDX-License-Identifier: MPL-2.0

use alloc::sync::Arc;
use core::fmt::Debug;

use aster_block::BlockDevice;
pub use aster_virtio::device::block::server_traits::BlockIOObservable;
use ostd::{Error, orpc::orpc_trait};

/// A combined trait for block devices that support I/O performance tracing.
///
/// This trait combines `BlockDevice` (basic block I/O operations) with
/// `BlockIOObservable` (I/O performance trace queues), enabling policies
/// like `LinnOSPolicy` to observe completion traces for intelligent device selection.
pub trait ObservableBlockDevice: BlockDevice + BlockIOObservable + Debug {}

/// Blanket implementation: any type implementing both traits automatically
/// implements `ObservableBlockDevice`.
impl<T: BlockDevice + BlockIOObservable + Debug> ObservableBlockDevice for T {}

#[orpc_trait]
pub trait SelectionPolicy: Debug {
    /// Get the block device to read from. The policy cannot decide, for whatever reason, this should
    /// return an error. The caller will use some fallback. If the returned block device does not
    /// exist, then the caller will also fallback.
    fn select_block_device(&self) -> Result<Arc<dyn BlockDevice>, Error>;
}
