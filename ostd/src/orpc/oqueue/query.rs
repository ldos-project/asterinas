// SPDX-License-Identifier: MPL-2.0

//! Queries for extracting information from an OQueue in the form needed by observers.

use alloc::boxed::Box;

use crate::orpc::oqueue::{ElementDescriptor, LifetimelessElementDescriptor};

/// A function to extract an observable value from a message. This function must be callable from
/// anywhere.
#[expect(type_alias_bounds)]
type ExtractionFunction<D: ElementDescriptor, U> =
    dyn for<'a> Fn(&D::Element<'a>) -> Option<U> + Sync + Send + 'static;

/// A query to run on a message of type `T`, returning a value of type `U` (or `None`). This is used
/// for observers to extract the values they need from messages in an OQueue.
pub struct ObservationQuery<D: ElementDescriptor, U> {
    /// The extractor function to call to extract the observed value from the message.
    extractor: Box<ExtractionFunction<D, U>>,
    // TODO(arthurp): This could be optimized for non-capturing cases by storing `fn(&T) ->
    // Option<U>` instead of `dyn Fn(&T) -> Option<U>`. This would avoid a dereference and an
    // allocation.

    // TODO(arthurp): This could carry an ID based on the code provided (for a macro) or the
    // `fn`-pointer. That ID could be used to avoid constructing a new ring buffer if the same query
    // is already present in an OQueue.
}

impl<D: ElementDescriptor, U> ObservationQuery<D, U> {
    /// Create a query which extracts a value from the message.
    ///
    /// This function **must** be fast and side-effect free. It is called on the publication path
    /// for any OQueue it is used to observer.
    pub fn new(extractor: impl for<'a> Fn(&D::Element<'a>) -> U + Sync + Send + 'static) -> Self {
        Self {
            extractor: Box::new(move |v| Some(extractor(v))),
        }
    }

    /// Create a query which extracts a value from the message or discards it.
    ///
    /// This function **must** be fast and side-effect free. It is called on the publication path
    /// for any OQueue it is used to observer.
    pub fn new_filter(
        extractor: impl for<'a> Fn(&D::Element<'a>) -> Option<U> + Sync + Send + 'static,
    ) -> Self {
        Self {
            extractor: Box::new(extractor),
        }
    }

    /// Execute the query on a given value.
    pub(super) fn call<'a, 'b>(&'a self, msg: &'a D::Element<'b>) -> Option<U> {
        (self.extractor)(msg)
    }
}

impl<T: Copy + 'static> ObservationQuery<LifetimelessElementDescriptor<T>, T> {
    /// A query which observes the entire message.
    ///
    /// This is equivalent to `ObservationQuery::new(|x| *x)`, but may be optimized.
    pub fn identity() -> Self {
        Self {
            extractor: Box::new(|x: &T| -> Option<T> { Some(*x) }),
        }
    }
}

#[cfg(ktest)]
mod test {
    use super::*;
    use crate::prelude::*;

    #[ktest]
    fn new_extractor() {
        let query =
            ObservationQuery::<LifetimelessElementDescriptor<usize>, usize>::new(|v| *v + 1);
        assert_eq!(query.call(&41), Some(42));
    }

    #[ktest]
    fn new_filter() {
        let query =
            ObservationQuery::<LifetimelessElementDescriptor<isize>, isize>::new_filter(|v| {
                if *v > 0 { Some(*v + 1) } else { None }
            });
        assert_eq!(query.call(&5), Some(6));
        assert_eq!(query.call(&-1), None);
    }
}
