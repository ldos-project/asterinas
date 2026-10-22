// SPDX-License-Identifier: MPL-2.0

//! Default projections of OQueue messages for export.

use alloc::{boxed::Box, sync::Arc};

use serde::Serialize;
pub use zerocopy::{Immutable, IntoBytes, KnownLayout};

pub use crate::DefaultProjection;

/// Supplies the value exported for an OQueue message when using [`register_with`](super::registry::register_with).
///
/// The `DefaultProjection` derive recursively projects fields that implement this trait. POD
/// values project to themselves; mark fields that should be opaque identifiers with
/// `#[projection(opaque)]`, which projects them through [`OpaqueId`].
pub trait DefaultProjection {
    /// The projected value type written to the export stream.
    type Projection: Copy + Send + Serialize + 'static;

    /// Returns the default projection of this message.
    fn default_projection(&self) -> Self::Projection;
}

impl<T> DefaultProjection for T
where
    T: IntoBytes + KnownLayout + Immutable + Copy + Send + Serialize + 'static,
{
    type Projection = Self;

    fn default_projection(&self) -> Self::Projection {
        *self
    }
}

/// Supplies an opaque identifier for a value projected with `#[projection(opaque)]`.
///
/// By default, the identifier is the address of the value itself. Smart pointers identify the
/// and references identify the value they point to instead, so that copies of the same pointer
/// share an identifier.
pub trait OpaqueId {
    /// Returns the opaque identifier of this value.
    fn opaque_id(&self) -> usize;
}

impl<T: ?Sized> OpaqueId for T {
    default fn opaque_id(&self) -> usize {
        core::ptr::from_ref(self) as *const () as usize
    }
}

impl<T: ?Sized> OpaqueId for &T {
    fn opaque_id(&self) -> usize {
        core::ptr::from_ref(*self) as *const () as usize
    }
}

impl<T: ?Sized> OpaqueId for Arc<T> {
    fn opaque_id(&self) -> usize {
        Arc::as_ptr(self) as *const () as usize
    }
}

impl<T: ?Sized> OpaqueId for Box<T> {
    fn opaque_id(&self) -> usize {
        let ptr: *const T = &**self;
        ptr as *const () as usize
    }
}

#[cfg(ktest)]
mod test {
    use serde::Serialize;

    use super::DefaultProjection;
    use crate::prelude::*;

    // If bool is DefaultProjection this should compile.
    // This is worth testing as bool is treated slightly special and only implements IntoBytes and
    // not FromBytes, which caused problems when we had a blanked impl on the trait Pod.
    #[expect(dead_code)]
    #[derive(DefaultProjection)]
    struct TestProjectBool(bool);

    #[derive(DefaultProjection)]
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

    #[derive(DefaultProjection)]
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

    #[derive(DefaultProjection)]
    struct ProjectedPointers<'a> {
        #[projection(opaque)]
        arc: Arc<u32>,
        #[projection(opaque)]
        boxed: Box<u32>,
        #[projection(opaque)]
        reference: &'a u32,
    }

    #[ktest]
    fn default_projection_get_inner_value_pointer_from_smart_ptr() {
        let data: u32 = 3;

        let arc = Arc::new(1);
        let message = ProjectedPointers {
            arc: arc.clone(),
            boxed: Box::new(2),
            reference: &data,
        };
        let projected = message.default_projection();

        assert_eq!(projected.arc, Arc::as_ptr(&arc) as usize);
        assert_eq!(
            projected.boxed,
            core::ptr::from_ref(&*message.boxed) as usize
        );
        assert_eq!(projected.reference, core::ptr::from_ref(&data) as usize);
    }
}
