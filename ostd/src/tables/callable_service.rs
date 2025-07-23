//! Utilities for using tables as an RPC system.
//! 


// TODO: This really needs the more advanced "erased information" support in tables. It's totally impossible to make
// this most general case work with Copy messages.

use alloc::{borrow::ToOwned, boxed::Box, string::String, sync::Arc};
use core::{borrow::Borrow, error::Error, fmt::Display};

use crate::{
    sync::WaitByYield,
    tables::{spsc::SpscTable, Consumer, Producer, Table, TableAttachError},
};


#[derive(Clone, Copy)]
struct CallMessage<A: Copy, R: Copy> {
    data: A,
    return_endpoint: ReturnProducer<R>,
}

#[derive(Clone, Copy)]
struct ReturnMessage<R: Copy> {
    data: Result<R, Box<dyn Error>>,
    return_endpoint: ReturnProducer<R>,
}

type ReturnProducer<R> = Box<dyn Producer<ReturnMessage<R>>>;

/// A callable service that guarantees
pub struct CallableService<A: Copy, R: Copy> {
    call_producer: Box<dyn Producer<CallMessage<A, R>>>,
    return_producer: Option<Box<dyn Producer<ReturnMessage<R>>>>,
    return_consumer: Box<dyn Consumer<ReturnMessage<R>>>,
}

#[derive(Debug)]
pub enum CallError {
    /// An error during the communication with the service (as opposed to in the execution in the other service.)
    CommunicationError { message: String },

    /// Error in the callee.
    CalleeError { source: Box<dyn Error> },
}

impl Display for CallError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CallError::CommunicationError { message } => {
                write!(f, "error communicating: {message}")
            }
            CallError::CalleeError { source } => write!(f, "error in callee: {source}"),
        }
    }
}

impl Error for CallError {}

impl<A: Copy + Send + 'static, R: Copy + Send + 'static> CallableService<A, R> {
    pub fn new_from_endpoints(
        call_producer: Box<dyn Producer<CallMessage<A, R>>>,
        return_producer: Box<dyn Producer<ReturnMessage<R>>>,
        return_consumer: Box<dyn Consumer<ReturnMessage<R>>>,
    ) -> Self {
        Self {
            call_producer,
            return_producer: Some(return_producer),
            return_consumer,
        }
    }

    pub fn new_from_tables(
        call_table: &(dyn Table<CallMessage<A, R>>),
        return_table: &(dyn Table<ReturnMessage<R>>),
    ) -> Result<Self, TableAttachError> {
        Ok(Self::new_from_endpoints(
            call_table.attach_producer()?,
            return_table.attach_producer()?,
            return_table.attach_consumer()?,
        ))
    }

    pub fn new(
        max_strong_observers: usize,
    ) -> Result<(Arc<dyn Table<CallMessage<A, R>>>, CallableService<A, R>), TableAttachError> {
        let call_table = SpscTable::new_with_waiters(
            2,
            max_strong_observers,
            40,
            WaitByYield::default(),
            WaitByYield::default(),
        );
        let service =
            CallableService::new_from_call_table(call_table.as_ref(), max_strong_observers)?;
        Ok((call_table, service))
    }

    pub fn new_from_call_table(
        call_table: &dyn Table<CallMessage<A, R>>,
        return_max_strong_observers: usize,
    ) -> Result<CallableService<A, R>, TableAttachError> {
        let return_table = SpscTable::new_with_waiters(
            2,
            return_max_strong_observers,
            40,
            WaitByYield::default(),
            WaitByYield::default(),
        );
        Ok(CallableService::new_from_tables(call_table, return_table.as_ref())?)
    }

    pub fn call(&mut self, arg: A) -> Result<R, CallError> {
        self.call_producer.put(CallMessage {
            data: arg,
            return_endpoint: self.return_producer.take().ok_or_else(|| {
                CallError::CommunicationError {
                    message: "call already in progress".to_owned(),
                }
            })?,
        });
        let ReturnMessage {
            data,
            return_endpoint,
        } = self.return_consumer.take();
        if self.return_producer.is_some() {
            return Err(CallError::CommunicationError {
                message: "return producer already refilled".to_owned(),
            });
        }
        self.return_producer = Some(return_endpoint);
        data.map_err(|e| CallError::CalleeError { source: e })
    }
}
