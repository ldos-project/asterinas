// SPDX-License-Identifier: MPL-2.0

//! Errors for OSTD

use ostd_macros::ostd_error;
use serde::Serialize;
use snafu::Snafu;

#[cfg(not(baseline_asterinas))]
use crate::orpc::oqueue::DefaultProjection;
#[cfg(not(baseline_asterinas))]
use crate::orpc::oqueue::OQueueError;
#[cfg(not(baseline_asterinas))]
use crate::stack_info::StackInfoDefaultProjection;
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
    #[snafu(display("Invalid arguments ({context})"))]
    InvalidArgs,
    /// Insufficient memory available.
    #[snafu(display("Insufficient memory ({context})"))]
    NoMemory,
    /// Page fault occurred.
    #[snafu(display("Page fault ({context})"))]
    PageFault,
    /// Access to a resource is denied.
    #[snafu(display("Access denied ({context})"))]
    AccessDenied,
    /// Input/output error.
    #[snafu(display("Input/output error ({context})"))]
    IoError,
    /// Insufficient system resources.
    #[snafu(display("Insufficient resources ({context})"))]
    NotEnoughResources,
    /// Arithmetic Overflow occurred.
    #[snafu(display("Arithmetic overflow ({context})"))]
    Overflow,
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

/// The error kind and captured context.
#[cfg(not(baseline_asterinas))]
#[derive(Clone, Copy, Serialize)]
pub struct ErrorDefaultProjection {
    kind: &'static str,
    context: Option<StackInfoDefaultProjection>,
}

#[cfg(not(baseline_asterinas))]
impl DefaultProjection for Error {
    type Projection = ErrorDefaultProjection;

    fn default_projection(&self) -> Self::Projection {
        let kind = match self {
            Self::InvalidArgs { .. } => "InvalidArgs",
            Self::NoMemory { .. } => "NoMemory",
            Self::PageFault { .. } => "PageFault",
            Self::AccessDenied { .. } => "AccessDenied",
            Self::IoError { .. } => "IoError",
            Self::NotEnoughResources { .. } => "NotEnoughResources",
            Self::Overflow { .. } => "Overflow",
            Self::RPCError { .. } => "RPCError",
            #[cfg(not(baseline_asterinas))]
            Self::OQueueError { .. } => "OQueueError",
        };

        ErrorDefaultProjection {
            kind,
            context: self.stack_info().map(DefaultProjection::default_projection),
        }
    }
}

impl From<PageTableError> for Error {
    // TODO(arthurp): This should be replaced with a proper wrapping of PageTableError as a source.
    fn from(_err: PageTableError) -> Error {
        AccessDeniedSnafu.build()
    }
}
