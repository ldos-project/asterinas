//! Non-constructable implementation of table traits for use as in unreachable branches of Result and similar.

use crate::tables::{Consumer, Cursor, Producer, StrongObserver, WeakObserver};


/// Non-constructable struct for Producer trait implementation.
pub struct NonConstructableProducer;

impl<T> Producer<T> for NonConstructableProducer {
    fn put(&self, _data: T) {
        unreachable!()
    }

    fn try_put(&self, _data: T) -> Option<T> {
        unreachable!()
    }
}

/// Non-constructable struct for Consumer trait implementation.
pub struct NonConstructableConsumer;

impl<T> Consumer<T> for NonConstructableConsumer {
    fn take(&self) -> T {
        unreachable!()
    }

    fn try_take(&self) -> Option<T> {
        unreachable!()
    }
}

/// Non-constructable struct for StrongObserver trait implementation.
pub struct NonConstructableStrongObserver;

impl<T> StrongObserver<T> for NonConstructableStrongObserver {
    fn strong_observe(&self) -> T {
        unreachable!()
    }

    fn try_strong_observe(&self) -> Option<T> {
        unreachable!()
    }
}

/// Non-constructable struct for WeakObserver trait implementation.
pub struct NonConstructableWeakObserver;

impl<T> WeakObserver<T> for NonConstructableWeakObserver {
    fn weak_observe(&self, _index: Cursor) -> Option<T> {
        unreachable!()
    }

    fn recent_cursor(&self) -> Cursor {
        unreachable!()
    }
    
    fn oldest_cursor(&self) -> Cursor {
        unreachable!()
    }
}
