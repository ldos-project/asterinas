// SPDX-License-Identifier: MPL-2.0

use core::fmt::Display;

use crate::mm::page_table::PageTableError;

/// The error type which is returned from the APIs of this crate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    /// Invalid arguments provided.
    InvalidArgs,
    /// Insufficient memory available.
    NoMemory,
    /// Page fault occurred.
    PageFault,
    /// Access to a resource is denied.
    AccessDenied,
    /// Input/output error.
    IoError,
    /// Insufficient system resources.
    NotEnoughResources,
    /// Arithmetic Overflow occurred.
    Overflow,
    /// Memory mapping already exists for the given virtual address.
    MapAlreadyMappedVaddr,
    /// Error when allocating kernel virtual memory.
    KVirtAreaAllocError,
}

impl Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidArgs => write!(f, "invalid arguments provided"),
            Self::NoMemory => write!(f, "insufficient memory available"),
            Self::PageFault => write!(f, "page fault occurred"),
            Self::AccessDenied => write!(f, "access to a resource is denied"),
            Self::IoError => write!(f, "input/output error"),
            Self::NotEnoughResources => write!(f, "insufficient system resources"),
            Self::Overflow => write!(f, "arithmetic overflow occurred"),
            Self::MapAlreadyMappedVaddr => write!(
                f,
                "memory mapping already exists for the given virtual address"
            ),
            Self::KVirtAreaAllocError => write!(f, "error when allocating kernel virtual memory"),
        }
    }
}

impl core::error::Error for Error {}

impl From<PageTableError> for Error {
    fn from(_err: PageTableError) -> Error {
        Error::AccessDenied
    }
}
