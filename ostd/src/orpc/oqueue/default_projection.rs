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
/// `#[projection(opaque)]`, which projects them through [`OpaqueId`], or omit fields with
/// `#[projection(omit)]`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement `DefaultProjection`",
    note = "derive `DefaultProjection` for a local type, or use `#[projection(opaque)]` on a field that should be represented by an ID;",
    note = "for a Copy, Send, and Serialize type that projects to itself, derive `Projectable`"
)]
pub trait DefaultProjection {
    /// The projected value type written to the export stream.
    type Projection: Copy + Send + Serialize + 'static;

    /// Returns the default projection of this message.
    fn default_projection(&self) -> Self::Projection;
}

/// Tag to mark types as trivially projectable
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement `Projectable`",
    note = "derive `Projectable` if this type can be copied, sent, and serialized as-is"
)]
pub trait Projectable {}

/// Implements `Projectable` for a set of types.
macro_rules! impl_projectable {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl Projectable for $ty {}
        )*
    };
}

impl_projectable!(());
impl_projectable!(bool);
impl_projectable!(char, f32, f64);
impl_projectable!(&'static str);
impl_projectable!(i8, i16, i32, i64, i128, isize);
impl_projectable!(u8, u16, u32, u64, u128, usize);
impl_projectable!(core::time::Duration);
impl_projectable!(
    core::net::IpAddr,
    core::net::Ipv4Addr,
    core::net::Ipv6Addr,
    core::net::SocketAddr,
    core::net::SocketAddrV4,
    core::net::SocketAddrV6,
);
impl_projectable!(
    core::num::NonZeroI8,
    core::num::NonZeroI16,
    core::num::NonZeroI32,
    core::num::NonZeroI64,
    core::num::NonZeroI128,
    core::num::NonZeroIsize,
    core::num::NonZeroU8,
    core::num::NonZeroU16,
    core::num::NonZeroU32,
    core::num::NonZeroU64,
    core::num::NonZeroU128,
    core::num::NonZeroUsize,
);

impl<T: Projectable, const N: usize> Projectable for [T; N] {}
impl<T: ?Sized> Projectable for core::marker::PhantomData<T> {}
impl<T: Projectable> Projectable for core::cmp::Reverse<T> {}
impl<T: Projectable> Projectable for core::num::Saturating<T> {}
impl<T: Projectable> Projectable for core::num::Wrapping<T> {}
impl<T: Projectable> Projectable for core::ops::Bound<T> {}

macro_rules! impl_tuple_default_projection {
    ($(($($ty:ident),+)),+ $(,)?) => {
        $(impl<$($ty: DefaultProjection),+> DefaultProjection for ($($ty,)+) {
            type Projection = ($($ty::Projection,)+);

            #[expect(non_snake_case)]
            fn default_projection(&self) -> Self::Projection {
                let ($($ty,)+) = self;
                ($($ty.default_projection(),)+)
            }
        })+
    };
}

impl_tuple_default_projection!(
    (A),
    (A, B),
    (A, B, C),
    (A, B, C, D),
    (A, B, C, D, E),
    (A, B, C, D, E, F),
    (A, B, C, D, E, F, G),
    (A, B, C, D, E, F, G, H),
    (A, B, C, D, E, F, G, H, I),
    (A, B, C, D, E, F, G, H, I, J),
    (A, B, C, D, E, F, G, H, I, J, K),
    (A, B, C, D, E, F, G, H, I, J, K, L),
    (A, B, C, D, E, F, G, H, I, J, K, L, M),
    (A, B, C, D, E, F, G, H, I, J, K, L, M, N),
    (A, B, C, D, E, F, G, H, I, J, K, L, M, N, O),
    (A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P),
);

impl<T> DefaultProjection for T
where
    T: Copy + Send + Serialize + Projectable + 'static,
{
    type Projection = Self;

    fn default_projection(&self) -> Self::Projection {
        *self
    }
}

impl<T: DefaultProjection> DefaultProjection for Option<T> {
    type Projection = Option<T::Projection>;

    fn default_projection(&self) -> Self::Projection {
        self.as_ref().map(DefaultProjection::default_projection)
    }
}

impl<T: DefaultProjection, E: DefaultProjection> DefaultProjection for Result<T, E> {
    type Projection = Result<T::Projection, E::Projection>;

    fn default_projection(&self) -> Self::Projection {
        match self {
            Ok(value) => Ok(value.default_projection()),
            Err(error) => Err(error.default_projection()),
        }
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
    use core::{
        cmp::Reverse,
        marker::PhantomData,
        net::SocketAddr,
        num::{NonZeroU128, Saturating, Wrapping},
        ops::Bound,
        time::Duration,
    };

    use serde::Serialize;

    use super::DefaultProjection;
    use crate::{orpc::oqueue::default_projection::OID, prelude::*};

    // If bool is DefaultProjection this should compile.
    // This is worth testing as bool is treated slightly special and only implements IntoBytes and
    // not FromBytes, which caused problems when we had a blanked impl on the trait Pod.
    #[expect(dead_code)]
    #[derive(DefaultProjection)]
    struct TestProjectBool(bool);

    #[expect(dead_code)]
    #[derive(DefaultProjection)]
    struct CoreTypeCoverage {
        character: char,
        signed: i128,
        unsigned: u128,
        nonzero: NonZeroU128,
        address: SocketAddr,
        duration: Duration,
        marker: PhantomData<u8>,
        bound: Bound<u32>,
        reverse: Reverse<u32>,
        saturating: Saturating<u32>,
        wrapping: Wrapping<u32>,
        array: [u8; 32],
        tuple: (u8, u16, u32),
        pair: (u8, crate::Error),
        option: Option<u32>,
        optional_error: Option<crate::Error>,
        result: Result<u32, crate::Error>,
        error: crate::Error,
    }

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

    #[expect(dead_code)]
    #[derive(DefaultProjection)]
    struct OmittedNamedField {
        included: u32,
        #[projection(omit)]
        omitted: Vec<u8>,
    }

    #[expect(dead_code)]
    #[derive(DefaultProjection)]
    struct OmittedTupleField(#[projection(omit)] Vec<u8>, u32);

    #[expect(dead_code)]
    #[derive(DefaultProjection)]
    enum OmittedEnumFields {
        Named {
            #[projection(omit)]
            omitted: Vec<u8>,
            included: u32,
        },
        Tuple(#[projection(omit)] Vec<u8>, u32),
    }

    #[ktest]
    fn default_projection_omits_named_and_tuple_fields() {
        let named = OmittedNamedField {
            included: 7,
            omitted: Vec::new(),
        }
        .default_projection();
        assert_eq!(named.included, 7);

        let mut encoded = Vec::new();
        Serialize::serialize(&named, &mut minicbor_serde::Serializer::new(&mut encoded)).unwrap();
        let mut decoder = minicbor::Decoder::new(&encoded);
        assert_eq!(decoder.map().unwrap(), Some(1));
        assert_eq!(decoder.str().unwrap(), "included");
        assert_eq!(decoder.u32().unwrap(), 7);
        assert_eq!(decoder.position(), encoded.len());

        let tuple = OmittedTupleField(Vec::new(), 9).default_projection();
        assert_eq!(tuple.0, 9);
    }

    #[ktest]
    fn default_projection_omits_enum_fields() {
        let named = OmittedEnumFields::Named {
            omitted: Vec::new(),
            included: 7,
        }
        .default_projection();
        assert!(matches!(
            named,
            OmittedEnumFieldsDefaultProjection::Named { included: 7 }
        ));

        let tuple = OmittedEnumFields::Tuple(Vec::new(), 9).default_projection();
        assert!(matches!(
            tuple,
            OmittedEnumFieldsDefaultProjection::Tuple(9)
        ));
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

    #[expect(dead_code)]
    #[derive(DefaultProjection)]
    struct BorrowedValue<'a> {
        #[projection(deref)]
        value: &'a bool,
    }

    #[derive(DefaultProjection)]
    struct BorrowedArc<'a> {
        #[projection(deref, opaque)]
        arc: &'a Arc<u32>,
    }

    #[ktest]
    fn deref_projection_identifies_arc_referent() {
        let arc = Arc::new(7);
        let projected = BorrowedArc { arc: &arc }.default_projection();
        assert_eq!(projected.arc, OID::of_ref(arc.as_ref()));
    }
}
