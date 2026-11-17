// SPDX-License-Identifier: MPL-2.0

use alloc::sync::Arc;
use core::fmt::Debug;

use aster_block::{BlockDevice, bio::SubmittedBio};
use ostd::Error;

pub struct BioCandidates<'a> {
    /// The request being routed.
    pub bio: &'a mut SubmittedBio,
    /// The admitted member indices to choose from (never empty).
    pub candidates: &'a [usize],
}

pub trait SelectionPolicy: Debug + Send + Sync {
    /// Chooses the member device to read from among the admitted `candidates`.
    ///
    /// The policy must return one of the devices whose index appears in
    /// `selection.candidates`. If the policy cannot decide, for whatever reason,
    /// this should return an error; the caller will use some fallback.
    fn select_block_device(&self, selection: BioCandidates) -> Result<Arc<dyn BlockDevice>, Error>;
}
