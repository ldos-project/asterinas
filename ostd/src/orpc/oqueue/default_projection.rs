// SPDX-License-Identifier: MPL-2.0

//! Default projections of OQueue messages for export.

use ostd_pod::Pod;

/// Supplies the value exported for an OQueue message when using [`register_with`](super::registry::register_with).
///
/// The `DefaultProjection` derive recursively projects fields that implement this trait. POD
/// values project to themselves; mark fields that should be opaque identifiers with
/// `#[projection(opaque)]`.
pub trait DefaultProjection {
    /// The projected value type written to the export stream.
    type Projection;

    /// Returns the default projection of this message.
    fn default_projection(&self) -> Self::Projection;
}

impl<T> DefaultProjection for T
where
    T: Pod,
{
    type Projection = Self;

    fn default_projection(&self) -> Self::Projection {
        *self
    }
}

#[cfg(ktest)]
mod test {
    use alloc::vec::Vec;

    use serde::Serialize;

    use super::DefaultProjection;
    use crate::prelude::*;

    #[derive(crate::DefaultProjection)]
    struct ProjectedLeaf {
        value: u32,
    }

    #[derive(Clone, Copy, serde::Serialize)]
    struct SpecialProjectionOutput(u32);

    struct SpecialProjection(u32);

    impl DefaultProjection for SpecialProjection {
        type Projection = SpecialProjectionOutput;

        fn default_projection(&self) -> Self::Projection {
            SpecialProjectionOutput(self.0 + 1)
        }
    }

    #[derive(crate::DefaultProjection)]
    struct ProjectedMessage {
        leaf: ProjectedLeaf,
        #[projection(opaque)]
        ref_type: &'static str,
        special: SpecialProjection,
    }

    #[ktest]
    fn default_projection_serializes_nested_custom_and_opaque_fields() {
        let message = ProjectedMessage {
            leaf: ProjectedLeaf { value: 0x1234_5678 },
            ref_type: "opaque",
            special: SpecialProjection(41),
        };
        let projected = message.default_projection();

        let mut actual = Vec::new();
        Serialize::serialize(
            &projected,
            &mut minicbor_serde::Serializer::new(&mut actual),
        )
        .unwrap();

        let mut decoder = minicbor::Decoder::new(&actual);
        assert_eq!(decoder.map().unwrap(), Some(3));
        assert_eq!(decoder.str().unwrap(), "leaf");
        assert_eq!(decoder.map().unwrap(), Some(1));
        assert_eq!(decoder.str().unwrap(), "value");
        assert_eq!(decoder.u32().unwrap(), message.leaf.value);
        assert_eq!(decoder.str().unwrap(), "ref_type");
        decoder.u64().unwrap();
        assert_eq!(decoder.str().unwrap(), "special");
        assert_eq!(decoder.u32().unwrap(), 42);
        assert_eq!(decoder.position(), actual.len());
    }
}
