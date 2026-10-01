// SPDX-License-Identifier: MPL-2.0

use aster_block::bio::BlockDeviceCompletionStats;
use ostd::{
    orpc::{
        oqueue::{GenericOQueueRef, OQueueRef},
        orpc_trait,
    },
    path,
};

#[orpc_trait]
pub trait BlockIOObservable {
    /// The OQueue containing every write request. This includes both sync and async writes and any
    /// other write operations on other traits
    fn bio_completion_oqueue(&self) -> OQueueRef<BlockDeviceCompletionStats> {
        GenericOQueueRef::new(4096, path!(io.block_io.bio_completion[unique]))
    }
}

/// A unique identifier for tracking I/O requests.
pub type IoRequestId = u64;
