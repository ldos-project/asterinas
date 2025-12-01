use alloc::{borrow::ToOwned, boxed::Box, sync::Arc};
use core::marker::PhantomData;

use crate::{
    orpc::{
        oqueue::{
            Consumer, OQueue, OQueueAttachError, OQueueRef, Producer, StrongObserver, WeakObserver,
        },
        sync::Blocker,
    },
    sync::SpinLock,
};

/// A wrapper struct around an underlying OQueue that transforms produced values.
pub struct TransformingQueue<T, U, F>
where
    F: Fn(&U) -> T,
{
    inner: OQueueRef<T>,
    transform: SpinLock<(F, PhantomData<U>)>,
}

impl<T, U, F> TransformingQueue<T, U, F>
where
    F: Fn(&U) -> T,
{
    /// Create a new transforming queue.
    pub fn new(queue: OQueueRef<T>, transform: F) -> Arc<Self> {
        Arc::new(Self {
            inner: queue,
            transform: SpinLock::new((transform, PhantomData)),
        })
    }
}

impl<T, U, F> OQueue<U> for TransformingQueue<T, U, F>
where
    F: Fn(&U) -> T + Clone + Send + 'static,
    T: Send + 'static,
    U: Send + 'static,
{
    fn attach_producer(&self) -> Result<Box<dyn Producer<U>>, OQueueAttachError> {
        let producer = self.inner.attach_producer()?;
        Ok(Box::new(TransformingProducer {
            inner: producer,
            transform: self.transform.lock().0.clone(),
            phantom: PhantomData,
        }))
    }

    fn attach_consumer(&self) -> Result<Box<dyn Consumer<U>>, OQueueAttachError> {
        // Return an error for consumer attachment
        Err(OQueueAttachError::Unsupported {
            table_type: "TransformingQueue".to_owned(),
        })
    }

    fn attach_strong_observer(&self) -> Result<Box<dyn StrongObserver<U>>, OQueueAttachError> {
        // Return an error for strong observer attachment
        Err(OQueueAttachError::Unsupported {
            table_type: "TransformingQueue".to_owned(),
        })
    }

    fn attach_weak_observer(&self) -> Result<Box<dyn WeakObserver<U>>, OQueueAttachError> {
        // Return an error for weak observer attachment
        Err(OQueueAttachError::Unsupported {
            table_type: "TransformingQueue".to_owned(),
        })
    }

    fn attach_child_queue(&self, subqueue: OQueueRef<U>) -> Result<(), OQueueAttachError> {
        Err(OQueueAttachError::Unsupported {
            table_type: "TransformingQueue".to_owned(),
        })
    }
}

struct TransformingProducer<T, U, F>
where
    F: Fn(&U) -> T,
{
    inner: Box<dyn Producer<T>>,
    transform: F,
    phantom: PhantomData<U>,
}

impl<T, U, F> Blocker for TransformingProducer<T, U, F>
where
    F: Fn(&U) -> T,
{
    fn should_try(&self) -> bool {
        self.inner.should_try()
    }

    fn prepare_to_wait(&self, waker: &alloc::sync::Arc<crate::sync::Waker>) {
        self.inner.prepare_to_wait(waker);
    }
}

impl<T, U, F> Producer<U> for TransformingProducer<T, U, F>
where
    F: Fn(&U) -> T + Clone + Send + 'static,
    T: Send + 'static,
    U: Send + 'static,
{
    fn produce(&self, data: U) {
        let transformed = (self.transform)(&data);
        self.inner.produce(transformed);
    }

    fn try_produce(&self, data: U) -> Option<U> {
        let transformed = (self.transform)(&data);
        self.inner.try_produce(transformed).map(|_| data)
    }
}

/// A wrapper struct around an underlying OQueue that transforms consumed values.
pub struct TransformingQueueConsumer<T, U, F>
where
    F: Fn(&T) -> U,
{
    inner: OQueueRef<T>,
    transform: SpinLock<(F, PhantomData<U>)>,
}

impl<T, U, F> TransformingQueueConsumer<T, U, F>
where
    F: Fn(&T) -> U,
{
    /// Create a new transforming queue for consumption.
    pub fn new(queue: OQueueRef<T>, transform: F) -> Arc<Self> {
        Arc::new(Self {
            inner: queue,
            transform: SpinLock::new((transform, PhantomData)),
        })
    }
}

impl<T, U, F> OQueue<U> for TransformingQueueConsumer<T, U, F>
where
    F: Fn(&T) -> U + Clone + Send + Sync + 'static,
    T: Send + 'static,
    U: Send + 'static,
{
    fn attach_producer(&self) -> Result<Box<dyn Producer<U>>, OQueueAttachError> {
        // Return an error for producer attachment
        Err(OQueueAttachError::Unsupported {
            table_type: "TransformingQueueConsumer".to_owned(),
        })
    }

    fn attach_consumer(&self) -> Result<Box<dyn Consumer<U>>, OQueueAttachError> {
        let consumer = self.inner.attach_consumer()?;
        Ok(Box::new(TransformingConsumer {
            inner: consumer,
            transform: self.transform.lock().0.clone(),
            phantom: PhantomData,
        }))
    }

    fn attach_strong_observer(&self) -> Result<Box<dyn StrongObserver<U>>, OQueueAttachError> {
        let observer = self.inner.attach_strong_observer()?;
        Ok(Box::new(TransformingStrongObserver {
            inner: observer,
            transform: self.transform.lock().0.clone(),
            phantom: PhantomData,
        }))
    }

    fn attach_weak_observer(&self) -> Result<Box<dyn WeakObserver<U>>, OQueueAttachError> {
        Err(OQueueAttachError::Unsupported {
            table_type: "TransformingQueueConsumer".to_owned(),
        })
    }
    fn attach_child_queue(&self, _subqueue: OQueueRef<U>) -> Result<(), OQueueAttachError> {
        Err(OQueueAttachError::Unsupported {
            table_type: "TransformingQueueConsumer".to_owned(),
        })
    }
}

struct TransformingConsumer<T, U, F>
where
    F: Fn(&T) -> U,
{
    inner: Box<dyn Consumer<T>>,
    transform: F,
    phantom: PhantomData<U>,
}

impl<T, U, F> Blocker for TransformingConsumer<T, U, F>
where
    F: Fn(&T) -> U,
{
    fn should_try(&self) -> bool {
        self.inner.should_try()
    }

    fn prepare_to_wait(&self, waker: &alloc::sync::Arc<crate::sync::Waker>) {
        self.inner.prepare_to_wait(waker);
    }
}

impl<T, U, F> Consumer<U> for TransformingConsumer<T, U, F>
where
    F: Fn(&T) -> U + Clone + Send + 'static,
    T: Send + 'static,
    U: Send + 'static,
{
    fn consume(&self) -> U {
        let value = self.inner.consume();
        (self.transform)(&value)
    }

    fn try_consume(&self) -> Option<U> {
        self.inner
            .try_consume()
            .map(|value| (self.transform)(&value))
    }
}

struct TransformingStrongObserver<T, U, F>
where
    F: Fn(&T) -> U,
{
    inner: Box<dyn StrongObserver<T>>,
    transform: F,
    phantom: PhantomData<U>,
}

impl<T, U, F> Blocker for TransformingStrongObserver<T, U, F>
where
    F: Fn(&T) -> U,
{
    fn should_try(&self) -> bool {
        self.inner.should_try()
    }

    fn prepare_to_wait(&self, waker: &alloc::sync::Arc<crate::sync::Waker>) {
        self.inner.prepare_to_wait(waker);
    }
}

impl<T, U, F> StrongObserver<U> for TransformingStrongObserver<T, U, F>
where
    F: Fn(&T) -> U + Clone + Send + Sync + 'static,
    T: Send + 'static,
    U: Send + 'static,
{
    fn strong_observe(&self) -> U {
        let value = self.inner.strong_observe();
        (self.transform)(&value)
    }

    fn try_strong_observe(&self) -> Option<U> {
        self.inner
            .try_strong_observe()
            .map(|value| (self.transform)(&value))
    }

    fn handle_fast(
        &mut self,
        handler: Box<dyn Fn(U) + Sync + Send>,
    ) -> Result<(), OQueueAttachError> {
        let transform = self.transform.clone();
        let inner_handler = Box::new(move |value: T| {
            let transformed = transform(&value);
            handler(transformed);
        });
        self.inner.handle_fast(inner_handler)
    }
}

#[cfg(ktest)]
mod test {
    use super::*;
    use crate::{
        orpc::oqueue::locking::{LockingQueue, ObservableLockingQueue},
        prelude::*,
    };
    #[ktest]
    fn test_transforming_queue_produce_consume() {
        let inner_queue: OQueueRef<i32> = LockingQueue::new(2);
        let transforming_queue = TransformingQueue::new(inner_queue.clone(), |x: &i32| *x * 2);

        let producer = transforming_queue.attach_producer().unwrap();
        let consumer = inner_queue.attach_consumer().unwrap();

        producer.produce(42);
        let value = consumer.consume();
        assert_eq!(value, 84);
    }

    #[ktest]
    fn test_transforming_queue_consumer_consume() {
        let inner_queue: OQueueRef<i32> = LockingQueue::new(2);
        let transforming_queue =
            TransformingQueueConsumer::new(inner_queue.clone(), |x: &i32| *x * 2);

        let producer = inner_queue.attach_producer().unwrap();
        let consumer = transforming_queue.attach_consumer().unwrap();

        producer.produce(42);
        let value = consumer.consume();
        assert_eq!(value, 84);
    }

    #[ktest]
    fn test_transforming_queue_consumer_try_consume() {
        let inner_queue: OQueueRef<i32> = LockingQueue::new(2);
        let transforming_queue =
            TransformingQueueConsumer::new(inner_queue.clone(), |x: &i32| *x * 2);

        let producer = inner_queue.attach_producer().unwrap();
        let consumer = transforming_queue.attach_consumer().unwrap();

        producer.produce(42);
        let result = consumer.try_consume();
        assert_eq!(result, Some(84));
    }

    #[ktest]
    fn test_transforming_queue_consumer_observe() {
        let inner_queue: OQueueRef<i32> = ObservableLockingQueue::new(2, 2);
        let transforming_queue =
            TransformingQueueConsumer::new(inner_queue.clone(), |x: &i32| *x * 2);

        let producer = inner_queue.attach_producer().unwrap();
        let observer = transforming_queue.attach_strong_observer().unwrap();

        producer.produce(42);
        let value = observer.strong_observe();
        assert_eq!(value, 84);
    }

    #[ktest]
    fn test_transforming_queue_consumer_attach_producer_error() {
        let inner_queue: OQueueRef<i32> = LockingQueue::new(2);
        let transforming_queue =
            TransformingQueueConsumer::new(inner_queue.clone(), |x: &i32| *x * 2);

        let result = transforming_queue.attach_producer();
        assert!(result.is_err());
    }
}
