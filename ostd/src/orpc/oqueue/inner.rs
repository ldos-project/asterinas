use core::marker::PhantomData;

use crate::orpc::oqueue::{CommunicationOQueue, OQueue, ObservationOQueue};

pub(crate) struct OQueueInner<T> {
    pub(crate) _phantom: PhantomData<T>
    // XXX: Add a real implementation
}

impl<T: Send> CommunicationOQueue<T> for OQueueInner<T> {
    fn attach_communication_producer(
        &self,
    ) -> Result<super::CommunicationProducer<T>, super::AttachmentError> {
        todo!()
    }

    fn attach_consumer(&self) -> Result<super::Consumer<T>, super::AttachmentError> {
        todo!()
    }
}

impl<T> ObservationOQueue<T> for OQueueInner<T> {
    fn attach_observation_producer(&self) -> Result<super::ObservationProducer<T>, super::AttachmentError> {
        todo!()
    }
}

impl<T> OQueue<T> for OQueueInner<T> {
    fn attach_strong_observer<U>(
        &self,
        query: super::ObservationQuery<T, U>,
    ) -> Result<super::StrongObserver<U>, super::AttachmentError>
    where
        U: Copy + Send {
        todo!()
    }

    fn attach_weak_observer<U>(
        &self,
        history_len: usize,
        query: super::ObservationQuery<T, U>,
    ) -> Result<super::WeakObserver<U>, super::AttachmentError>
    where
        U: Copy + Send {
        todo!()
    }

    fn as_any_oqueue(&self) -> super::AnyOQueueRef<T> {
        todo!()
    }
}

