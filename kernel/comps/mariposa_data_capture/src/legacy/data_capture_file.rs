// SPDX-License-Identifier: MPL-2.0

//! TEMPORARY: A version of [`crate::data_capture_file`] that operates on legacy OQueues.
//!
//! This is otherwise identical in structure and purpose to the non-legacy version. See
//! [`crate::data_capture_file`] for full documentation.

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{any::Any, error::Error, sync::atomic::AtomicBool};

use aster_block::{BLOCK_SIZE, BlockDevice, id::Bid};
use binary_serde::BinarySerde;
use ostd::{
    new_server,
    orpc::{
        errors::RPCError,
        framework::{Server, spawn_thread},
        legacy_oqueue::{Consumer, OQueue, StrongObserver, locking::LockingQueue},
        orpc_impl, orpc_server, orpc_trait,
        sync::{BlockOnMany, Blocker},
    },
};

use crate::data_buffering::ChunkingWriteWrapper;

/// TEMPORARY: Registration information for a legacy OQueue observer.
pub struct ObserverRegistration<T: Copy + Send + BinarySerde> {
    /// TEMPORARY: A strong observer attachment on the legacy OQueue.
    pub observer: Box<dyn StrongObserver<T>>,
}

impl<T: Copy + Send + BinarySerde> core::fmt::Debug for ObserverRegistration<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ObserverRegistration").finish()
    }
}

#[orpc_trait]
pub trait DataCaptureFile<T: Copy + Send + BinarySerde>: Any {
    /// TEMPORARY: Attach a new legacy OQueue to the output. If output has already started, then the
    /// path will not appear in the block header.
    fn register_observer(&self, attachment: ObserverRegistration<T>) -> Result<(), RPCError>;
    /// TEMPORARY: Flush any data remaining in the output buffers to disk.
    fn flush(&self) -> Result<(), RPCError>;
    /// TEMPORARY: Enable or disable capturing to this file.
    fn set_capturing(&self, capturing: bool) -> Result<(), RPCError>;
}

/// TEMPORARY: Command enum for [`DataCaptureFile`] operations.
enum DataCaptureFileCommand<T: Copy + Send + BinarySerde + 'static> {
    RegisterObserver(ObserverRegistration<T>),
    Flush,
}

impl<T: Copy + Send + BinarySerde + 'static> core::fmt::Debug for DataCaptureFileCommand<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RegisterObserver(reg) => f.debug_tuple("RegisterObserver").field(reg).finish(),
            Self::Flush => write!(f, "Flush"),
        }
    }
}

#[orpc_server(DataCaptureFile<T>)]
struct DataCaptureFileServer<T: Copy + Send + BinarySerde + 'static> {
    command_queue: Arc<LockingQueue<DataCaptureFileCommand<T>>>,
    capturing: AtomicBool,
}

struct DataCaptureFileServerThread<T: Copy + Send + BinarySerde + 'static> {
    command_consumer: Box<dyn Consumer<DataCaptureFileCommand<T>>>,
    block_device: Arc<dyn BlockDevice>,
    start_bid: Bid,
    end_bid: Bid,
    server: Arc<DataCaptureFileServer<T>>,
}

impl<T: Copy + Send + BinarySerde + 'static> DataCaptureFileServerThread<T> {
    fn run(&self) -> Result<(), Box<dyn Error>> {
        let mut data_buf_handler =
            ChunkingWriteWrapper::new(BLOCK_SIZE * 2, self.block_device.clone(), self.start_bid);
        let mut observers: Vec<Box<dyn StrongObserver<T>>> = Default::default();
        let mut headers_written = false;
        let mut block_handler = BlockOnMany::new();

        loop {
            let blockers = [(&*self.command_consumer) as &dyn Blocker]
                .into_iter()
                .chain(observers.iter().map(|o| o.as_ref() as &dyn Blocker));
            block_handler.block_on(blockers);

            // Handle commands from method calls.
            if let Some(cmd) = self.command_consumer.try_consume() {
                match cmd {
                    DataCaptureFileCommand::RegisterObserver(ObserverRegistration { observer }) => {
                        observers.push(observer);
                    }
                    DataCaptureFileCommand::Flush => {
                        data_buf_handler.flush()?;
                    }
                }
            }

            let capturing = self
                .server
                .capturing
                .load(core::sync::atomic::Ordering::SeqCst);
            // Observe and serialize.
            for o in &observers {
                // We can't skip the try_strong_observe calls when not `capturing` because that
                // would leave the values in the OQueues and block them.
                if let Some(v) = o.try_strong_observe()
                    && capturing
                {
                    if !headers_written {
                        data_buf_handler.write_header::<T>(&[])?;
                        headers_written = true;
                    }

                    data_buf_handler.write_value(&v);
                    data_buf_handler.flush_if_needed()?;
                    if data_buf_handler.current_bid == self.end_bid {
                        log::warn!("Data capture ran out of space.");
                    }
                }
            }
        }
    }
}

#[orpc_impl]
impl<T: Copy + Send + BinarySerde> DataCaptureFile<T> for DataCaptureFileServer<T> {
    fn register_observer(&self, attachment: ObserverRegistration<T>) -> Result<(), RPCError> {
        self.command_queue
            .produce(DataCaptureFileCommand::RegisterObserver(attachment))
            .unwrap();
        Ok(())
    }

    fn flush(&self) -> Result<(), RPCError> {
        self.command_queue
            .produce(DataCaptureFileCommand::Flush)
            .unwrap();
        Ok(())
    }

    fn set_capturing(&self, capturing: bool) -> Result<(), RPCError> {
        self.capturing
            .store(capturing, core::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

/// TEMPORARY: A builder for a [`DataCaptureFile`] provided by a [`DataCaptureDevice`].
pub struct DataCaptureFileBuilder {
    pub(crate) block_device: Arc<dyn BlockDevice>,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) server: Arc<dyn Server>,
}

impl DataCaptureFileBuilder {
    /// TEMPORARY: Construct the [`DataCaptureFile`] for a specific type of data.
    pub fn build<T>(self) -> Arc<dyn DataCaptureFile<T>>
    where
        T: Copy + Send + BinarySerde + 'static,
    {
        Server::orpc_server_base(self.server.as_ref())
            .call_in_context(move || -> Result<Arc<DataCaptureFileServer<T>>, RPCError> {
                let command_queue = LockingQueue::new(8);
                let server = new_server!(|_| DataCaptureFileServer {
                    command_queue: command_queue.clone(),
                    capturing: AtomicBool::new(false)
                });

                spawn_thread(server.clone(), {
                    let thread = DataCaptureFileServerThread {
                        command_consumer: command_queue
                            .attach_consumer()
                            .expect("single purpose OQueue failed."),
                        block_device: self.block_device,
                        start_bid: Bid::new(self.start as u64),
                        end_bid: Bid::new(self.end as u64),
                        server: server.clone(),
                    };
                    move || thread.run()
                });

                Ok(server)
            })
            .expect("Closure cannot fail")
    }
}
