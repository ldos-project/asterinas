// SPDX-License-Identifier: MPL-2.0

use alloc::sync::Arc;
use core::fmt::Debug;

use aster_block::{BlockDevice, bio::SubmittedBio};
#[cfg(not(baseline_asterinas))]
pub use aster_virtio::device::block::server_traits::BlockIOObservable;
use ostd::{Error, orpc::orpc_trait};

/// A combined trait for block devices that support I/O performance tracing.
///
/// This trait combines `BlockDevice` (basic block I/O operations) with
/// `BlockIOObservable` (I/O performance trace queues), enabling policies
/// like `LinnOSPolicy` to observe completion traces for intelligent device selection.
#[cfg(not(baseline_asterinas))]
pub trait ObservableBlockDevice: BlockDevice + BlockIOObservable + Debug {}

/// Blanket implementation: any type implementing both traits automatically
/// implements `ObservableBlockDevice`.
#[cfg(not(baseline_asterinas))]
impl<T: BlockDevice + BlockIOObservable + Debug> ObservableBlockDevice for T {}

pub struct BioCandidates<'a> {
    /// The request being routed.
    pub bio: &'a mut SubmittedBio,
    /// The admitted member indices to choose from (never empty).
    pub candidates: &'a [usize],
}

#[orpc_trait]
pub trait SelectionPolicy: Debug {
    /// Chooses the member device to read from among the admitted `candidates`.
    ///
    /// The policy must return one of the devices whose index appears in
    /// `selection.candidates`. If the policy cannot decide, for whatever reason,
    /// this should return an error; the caller will use some fallback.
    fn select_block_device(&self, selection: BioCandidates) -> Result<Arc<dyn BlockDevice>, Error>;
}
