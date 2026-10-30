// SPDX-License-Identifier: MPL-2.0

//! Stub implementation for orpc which is used to simplify writing code that supports both ORPC and
//! non-ORPC modes. This is exported as `crate::orpc` when `cfg(baseline_asterinas)` is set.

pub use orpc_macros::{noop as orpc_server, noop as orpc_impl, noop as orpc_trait};

pub use crate::{new_server, orpc_common::errors};

/// Baseline exports for OQueue derives used by shared types.
pub mod oqueue {
    pub use orpc_macros::ElementNoop as Element;
}

#[cfg(ktest)]
mod test {
    use crate::{DefaultProjection, Projectable, orpc::oqueue::Element, prelude::*};

    #[derive(DefaultProjection)]
    struct Projected(#[projection(opaque)] usize);

    #[derive(Projectable)]
    struct Trivial(usize);

    #[derive(Element)]
    struct Message(Trivial);

    #[ktest]
    fn baseline_derives_accept_ordinary_types() {
        let message = Message(Trivial(7));
        let projected = Projected(3);
        assert_eq!(message.0.0 + projected.0, 10);
    }
}
