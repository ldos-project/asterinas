// SPDX-License-Identifier: MPL-2.0

use int_to_c_enum::TryFromInt;
#[cfg(not(baseline_asterinas))]
use ostd::orpc::{errors::RPCError, oqueue::OQueueError};
use ostd::ostd_error;
use snafu::{ResultExt as _, Snafu};

use crate::{queue, transport::VirtioTransportError};

pub mod block;
pub mod console;
pub mod entropy;
pub mod input;
pub mod network;
pub mod socket;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, TryFromInt)]
pub(crate) enum VirtioDeviceType {
    Invalid = 0,
    Network = 1,
    Block = 2,
    Console = 3,
    Entropy = 4,
    TraditionalMemoryBalloon = 5,
    IoMemory = 6,
    Rpmsg = 7,
    ScsiHost = 8,
    Transport9P = 9,
    Mac80211Wlan = 10,
    RprocSerial = 11,
    VirtioCaif = 12,
    MemoryBalloon = 13,
    Gpu = 16,
    Timer = 17,
    Input = 18,
    Socket = 19,
    Crypto = 20,
    SignalDistribution = 21,
    Pstore = 22,
    Iommu = 23,
    Memory = 24,
}

#[ostd_error]
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum VirtioDeviceError {
    #[snafu(transparent)]
    Transport {
        source: VirtioTransportError,
    },
    #[ostd(context(source))]
    ResourceAlloc {
        source: ostd::Error,
    },
    InvalidQueueArgs,
    UnsupportedConfig,
    /// The ORPC Errors
    #[cfg(not(baseline_asterinas))]
    #[snafu(transparent)]
    #[ostd(context(source))]
    RPCError {
        /// Source ORPC error
        source: RPCError,
    },
    /// The OQueue errors
    #[cfg(not(baseline_asterinas))]
    #[snafu(transparent)]
    #[ostd(context(source))]
    OQueueError {
        /// Source OQueue error
        source: OQueueError,
    },
}

impl From<queue::CreationError> for VirtioDeviceError {
    fn from(value: queue::CreationError) -> Self {
        match value {
            queue::CreationError::InvalidArgs => InvalidQueueArgsSnafu.build(),
            queue::CreationError::ResourceAlloc(e) => {
                Err::<(), _>(e).context(ResourceAllocSnafu).unwrap_err()
            }
        }
    }
}
