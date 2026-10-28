// SPDX-License-Identifier: MPL-2.0

//! Default projections of OQueue messages for export.

use alloc::{boxed::Box, sync::Arc};
use core::any::type_name;

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

/// Tag to mark types as trivially projectable
pub trait Projectable {}

/// Implement Projectable for a set of types,
macro_rules! impl_projectable {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl Projectable for $ty {}
        )*
    };
}

impl_projectable!(bool);
impl_projectable!(i8, i16, i32, i64, isize);
impl_projectable!(u8, u16, u32, u64, usize);
impl_projectable!(f32, f64);
impl_projectable!(core::net::Ipv4Addr, core::net::Ipv6Addr);

impl<T> DefaultProjection for T
where
    T: Copy + Send + Serialize + Projectable + 'static,
{
    type Projection = Self;

    fn default_projection(&self) -> Self::Projection {
        *self
    }
}

/// An ID of an object. This include an opaque ID (often a pointer value), and the type name. The
/// type name is stored as a reference to a static string, so this struct is small and `Copy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct OID {
    /// An opaque ID of the object. This value has no meaning, but is unique for the lifetime of the
    /// object.
    ///
    /// NOTE: Ideally, this would be unique over the lifetime of the system. However, that isn't
    /// true yet.
    pub id: usize,

    /// The name of the type of the object. This comes from [`core::any::type_name`] and, like that,
    /// has an undefined format, so it cannot safely be parsed.
    pub type_name: &'static str,
}

impl OID {
    /// Create the OID for a value. `T` should be provided explicitly because doing so forces the
    /// compiler to generate the ID from something of type `T`. That prevents accidentally
    /// generating the ID from a pointer to a reference or a box-like type. The compiler auto-deref
    /// rules will generally unwrap the object as needed to get the correct reference.
    pub fn of_ref<T: ?Sized>(v: &T) -> OID {
        OID {
            id: core::ptr::from_ref(v) as *const () as usize,
            type_name: type_name::<T>(),
        }
    }
}

/// Supplies an opaque identifier for a value projected with `#[projection(opaque)]`.
///
/// By default, the identifier is the address of the value itself along with the name of the type.
/// Smart pointers and references identify the value they point to, so that copies of the same smart
/// pointer share an identifier.
pub trait OpaqueId {
    /// Returns the opaque identifier of this value.
    fn opaque_id(&self) -> OID;
}

impl<T: ?Sized> OpaqueId for &T {
    fn opaque_id(&self) -> OID {
        OID::of_ref::<T>(self)
    }
}

impl<T: ?Sized> OpaqueId for Arc<T> {
    fn opaque_id(&self) -> OID {
        OID::of_ref::<T>(self)
    }
}

impl<T: ?Sized> OpaqueId for Box<T> {
    fn opaque_id(&self) -> OID {
        OID::of_ref::<T>(self)
    }
}

#[cfg(ktest)]
mod test {
    use serde::Serialize;

    use super::DefaultProjection;
    use crate::{orpc::oqueue::default_projection::OID, prelude::*};

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
        assert_eq!(decoder.map().unwrap(), Some(2));
        assert_eq!(decoder.str().unwrap(), "id");
        decoder.u64().unwrap();
        assert_eq!(decoder.str().unwrap(), "type_name");
        assert!(decoder.str().unwrap().contains("str"));
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

        assert_eq!(projected.arc, OID::of_ref(arc.as_ref()));
        assert_eq!(projected.boxed, OID::of_ref(message.boxed.as_ref()));
        assert_eq!(projected.reference, OID::of_ref(&data));
    }
}
