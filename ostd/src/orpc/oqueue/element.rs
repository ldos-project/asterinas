// SPDX-License-Identifier: MPL-2.0

//! Traits, types, and utilities to declare [`Element`]s which can be places in OQueues.

use core::marker::PhantomData;

pub use orpc_macros::Element;

/// A trait for types which encapsulate a element type [`Self::Element`]. This wrapper is required
/// for types with lifetime parameters. You should never have to implement this trait yourself. It
/// will be generated automatically if you derive [`Element`].
///
/// Most normal types can use [`LifetimelessElementDescriptor<T>`].
///
/// This type cannot be replaced with [`Self::Element`] itself, because that takes a lifetime
/// parameter and Rust does not support passing type constructors (generic types without their
/// argument bound) as type parameters.
pub trait ElementDescriptor: 'static {
    /// The element type, which may depend on a lifetime parameter.
    type Element<'a>: ?Sized;
}

/// An element which can be placed in an OQueue. This trait should always be handled by a generic
/// impl or derived.
pub trait Element {
    /// The descriptor for this element type.
    ///
    /// For many types this will be [`LifetimelessElementDescriptor<Self>`]. For types with lifetime
    /// parameters, this will be a special descriptor type which carries the universally quantified
    /// element type as [`Element`](`ElementDescriptor::Element`).
    type Descriptor: ElementDescriptor;
}

/// A [`ElementDescriptor`] for types without a lifetime parameter. This is by far the most common
/// descriptor.
pub struct LifetimelessElementDescriptor<T: ?Sized> {
    _phantom: PhantomData<T>,
}

impl<T: ?Sized + 'static> ElementDescriptor for LifetimelessElementDescriptor<T> {
    type Element<'a> = T;
}

// Impls of ElementDescriptor for standard types.

impl<T: Copy + 'static> Element for T {
    type Descriptor = LifetimelessElementDescriptor<Self>;
}

impl<T: Copy + 'static> Element for [T] {
    type Descriptor = LifetimelessElementDescriptor<Self>;
}

#[cfg(ktest)]
mod test {
    use ostd::prelude::*;
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::orpc::{
        oqueue::{
            ConsumableOQueue as _, ConsumableOQueueRef, ElementOQueueRef, OQueue as _,
            OQueueBase as _, ObservationQuery,
        },
        path::Path,
    };

    #[ktest]
    fn element_derive_no_lifetime() {
        #[derive(Element)]
        struct NoLifetime {
            value: u32,
        }

        assert_impl_all!(NoLifetime: Element);

        let queue = ConsumableOQueueRef::<NoLifetime>::new(4, Path::test());
        let producer = queue.attach_value_producer().unwrap();
        let consumer = queue.attach_consumer().unwrap();

        producer.produce(NoLifetime { value: 42 });
        let consumed = consumer.consume();
        assert_eq!(consumed.value, 42);
    }

    #[ktest]
    fn element_derive_one_lifetime() {
        #[derive(Element)]
        struct OneLifetime<'a> {
            value: &'a usize,
        }

        assert_impl_all!(OneLifetime<'static>: Element);
        assert_impl_all!(OneLifetimeDescriptor: ElementDescriptor);

        let queue = ElementOQueueRef::<OneLifetime>::new(4, Path::test());
        let producer = queue.attach_ref_producer().unwrap();
        let observer = queue
            .attach_strong_observer(ObservationQuery::new(|m: &OneLifetime| *m.value))
            .unwrap();

        let value = 123usize;
        producer.produce_ref(&OneLifetime { value: &value });
        let observed = observer.strong_observe().unwrap();
        assert_eq!(observed, 123);
    }

    #[ktest]
    fn element_derive_with_type_param_no_lifetime() {
        #[derive(Element)]
        struct WithTypeParamNoLifetime<T: 'static> {
            value: T,
        }

        assert_impl_all!(WithTypeParamNoLifetime<usize>: Element);

        let queue = ElementOQueueRef::<WithTypeParamNoLifetime<u32>>::new(4, Path::test());
        let producer = queue.attach_ref_producer().unwrap();
        let observer = queue
            .attach_strong_observer(ObservationQuery::new(|m: &WithTypeParamNoLifetime<u32>| {
                m.value
            }))
            .unwrap();

        let value = 999u32;
        producer.produce_ref(&WithTypeParamNoLifetime { value });
        let observed = observer.strong_observe().unwrap();
        assert_eq!(observed, 999);
    }

    #[ktest]
    fn element_derive_with_type_param() {
        #[derive(Element)]
        struct WithTypeParam<'a, T> {
            value: &'a T,
        }

        assert_impl_all!(WithTypeParamDescriptor<u32>: ElementDescriptor);

        let queue = ElementOQueueRef::<WithTypeParam<u32>>::new(4, Path::test());
        let producer = queue.attach_ref_producer().unwrap();
        let observer = queue
            .attach_strong_observer(ObservationQuery::new(|m: &WithTypeParam<u32>| *m.value))
            .unwrap();

        let value = 999u32;
        producer.produce_ref(&WithTypeParam { value: &value });
        let observed = observer.strong_observe().unwrap();
        assert_eq!(observed, 999);
    }

    #[ktest]
    fn element_derive_with_where_clause() {
        #[derive(Element)]
        struct WithWhereClause<'a, T>
        where
            T: Clone + 'static,
        {
            value: &'a T,
        }

        assert_impl_all!(WithWhereClauseDescriptor<u32>: ElementDescriptor);

        let queue = ElementOQueueRef::<WithWhereClause<u32>>::new(4, Path::test());
        let producer = queue.attach_ref_producer().unwrap();
        let observer = queue
            .attach_strong_observer(ObservationQuery::new(|m: &WithWhereClause<u32>| *m.value))
            .unwrap();

        let value = 777u32;
        producer.produce_ref(&WithWhereClause { value: &value });
        let observed = observer.strong_observe().unwrap();
        assert_eq!(observed, 777);
    }

    #[ktest]
    fn element_derive_multi_type_params() {
        #[derive(Element)]
        struct MultiTypeParams<'a, T, U> {
            value1: &'a T,
            value2: &'a U,
        }

        assert_impl_all!(MultiTypeParamsDescriptor<u32, usize>: ElementDescriptor);

        let queue = ElementOQueueRef::<MultiTypeParams<u32, usize>>::new(4, Path::test());
        let producer = queue.attach_ref_producer().unwrap();
        let observer = queue
            .attach_strong_observer(ObservationQuery::new(|m: &MultiTypeParams<u32, usize>| {
                (*m.value1, *m.value2)
            }))
            .unwrap();

        let val1 = 111u32;
        let val2 = 222usize;
        producer.produce_ref(&MultiTypeParams {
            value1: &val1,
            value2: &val2,
        });
        let observed = observer.strong_observe().unwrap();
        assert_eq!(observed, (111, 222));
    }
}
