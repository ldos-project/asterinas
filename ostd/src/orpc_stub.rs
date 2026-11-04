// SPDX-License-Identifier: MPL-2.0

//! Stub implementation for orpc which is used to simplify writing code that supports both ORPC and
//! non-ORPC modes. This is exported as `crate::orpc` when `cfg(baseline_asterinas)` is set.

pub use orpc_macros::{
    baseline_trace_methods as trace_methods, noop as orpc_impl, noop as orpc_server,
    noop as orpc_trait,
};

pub use crate::{new_server, orpc_common::errors};

/// Baseline exports for OQueue derives used by shared types.
pub mod oqueue {
    pub use orpc_macros::ElementNoop as Element;
}

#[cfg(ktest)]
mod test {
    use crate::{DefaultProjection, Projectable, orpc::oqueue::Element};

    #[expect(dead_code)]
    #[derive(DefaultProjection)]
    struct Projected(#[projection(opaque)] usize);

    #[expect(dead_code)]
    #[derive(Projectable)]
    struct Trivial(usize);

    #[expect(dead_code)]
    #[derive(Element)]
    struct Message(Trivial);
}
