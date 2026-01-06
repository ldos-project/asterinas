// SPDX-License-Identifier: MPL-2.0

//! Errors for OSTD

use ostd_macros::ostd_error;
use snafu::Snafu;

use crate::{mm::page_table::PageTableError, stack_info::StackInfo};

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
#[derive(Snafu, Clone, Debug)]
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
}

impl From<PageTableError> for Error {
    // TODO(arthurp): This should be replaced with a proper wrapping of PageTableError as a source.
    fn from(_err: PageTableError) -> Error {
        AccessDeniedSnafu.build()
    }
}
