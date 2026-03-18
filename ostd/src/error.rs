// SPDX-License-Identifier: MPL-2.0

//! Errors for OSTD

use ostd_macros::ostd_error;
use snafu::Snafu;

#[cfg(not(baseline_asterinas))]
use crate::orpc::oqueue::OQueueError;
use crate::{mm::page_table::PageTableError, orpc::errors::RPCError, stack_info::StackInfo};

/// The trait of all errors carrying OSTD metadata.
///
/// This specifically provides access to an [`StackInfo`]. Implement this using the
/// [`ostd_error`] attribute macro.
pub trait OstdError: snafu::Error {
    /// Get the context captured with the error.
    fn stack_info(&self) -> Option<&StackInfo>;
}

/// The error type which is returned from the APIs of this crate.
#[ostd_error]
#[derive(Snafu, Debug)]
#[snafu(visibility(pub))]
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
    /// A ORPC error
    #[snafu(transparent)]
    #[ostd(context(source))]
    RPCError {
        /// Source ORPC error
        source: RPCError,
    },
    /// An OQueue error
    #[snafu(transparent)]
    #[ostd(context(source))]
    #[cfg(not(baseline_asterinas))]
    OQueueError {
        /// Source OQueue error
        source: OQueueError,
    },
}

impl From<PageTableError> for Error {
    // TODO(arthurp): This should be replaced with a proper wrapping of PageTableError as a source.
    fn from(_err: PageTableError) -> Error {
        AccessDeniedSnafu.build()
    }
}
