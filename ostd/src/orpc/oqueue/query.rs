// SPDX-License-Identifier: MPL-2.0

//! Queries for extracting information from an OQueue in the form needed by observers.

use alloc::boxed::Box;

/// A function to extract an observable value from a message. This function must be callable from
/// anywhere.
type ExtractionFunction<T, U> = dyn Fn(&T) -> Option<U> + Sync + Send + 'static;

/// A query to run on a message of type `T`, returning a value of type `U` (or `None`). This is used
/// for observers to extract the values they need from messages in an OQueue.
pub struct ObservationQuery<T: ?Sized, U> {
    /// The extractor function to call to extract the observed value from the message.
    extractor: Box<ExtractionFunction<T, U>>,
    // TODO(arthurp): This could be optimized for non-capturing cases by storing `fn(&T) ->
    // Option<U>` instead of `dyn Fn(&T) -> Option<U>`. This would avoid a dereference and an
    // allocation.

    // TODO(arthurp): This could carry an ID based on the code provided (for a macro) or the
    // `fn`-pointer. That ID could be used to avoid constructing a new ring buffer if the same query
    // is already present in an OQueue.
}

impl<T: ?Sized, U> ObservationQuery<T, U> {
    /// Create a query which extracts a value from the message.
    ///
    /// This function **must** be fast and side-effect free. It is called on the publication path
    /// for any OQueue it is used to observer.
    pub fn new(extractor: impl Fn(&T) -> U + Sync + Send + 'static) -> Self {
        Self {
            extractor: Box::new(move |v| Some(extractor(v))),
        }
    }

    /// Create a query which extracts a value from the message or discards it.
    ///
    /// This function **must** be fast and side-effect free. It is called on the publication path
    /// for any OQueue it is used to observer.
    pub fn new_filter(extractor: impl (Fn(&T) -> Option<U>) + Sync + Send + 'static) -> Self {
        Self {
            extractor: Box::new(extractor),
        }
    }

    /// Execute the query on a given value.
    pub(super) fn call(&self, msg: &T) -> Option<U> {
        (self.extractor)(msg)
    }
}

#[cfg(ktest)]
mod test {
    use super::*;
    use crate::prelude::*;

    #[ktest]
    fn test_new_extractor() {
        let query = ObservationQuery::new(|v| *v + 1);
        assert_eq!(query.call(&41), Some(42));
    }

    #[ktest]
    fn test_new_filter() {
        let query = ObservationQuery::new_filter(|v| if *v > 0 { Some(*v + 1) } else { None });
        assert_eq!(query.call(&5), Some(6));
        assert_eq!(query.call(&-1), None);
    }
}
