use alloc::boxed::Box;

/// A query for use
pub struct ObservationQuery<T: ?Sized, U> {
    /// The extractor function to call to extract the observed value from the message.
    extractor: Box<dyn Fn(&T) -> Option<U> + Sync + Send + 'static>,
    // TODO(arthurp): This could be optimized for non-capturing cases by storing `fn(&T) ->
    // Option<U>` instead of `dyn Fn(&T) -> Option<U>`. This would avoid a dereference and an
    // allocation.

    // TODO(arthurp): This could carry an ID based on the code provided (for a macro) or the
    // `fn`-pointer. That ID could be used to avoid constructing a new ring buffer if the same query
    // is already present in an OQueue.
}

impl<T: ?Sized, U> ObservationQuery<T, U> {
    /// Create a query which extracts a value from the message.
    pub fn new(extractor: impl Fn(&T) -> U + Sync + Send + 'static) -> Self {
        Self {
            extractor: Box::new(move |v| Some(extractor(v))),
        }
    }

    /// Create a query which extracts a value from the message or drops it.
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
