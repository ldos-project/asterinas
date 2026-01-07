use core::marker::PhantomData;

pub(crate) struct OQueueInner<T> {
    pub(crate) _phantom: PhantomData<[T]>
    // XXX: Add a real implementation
}
