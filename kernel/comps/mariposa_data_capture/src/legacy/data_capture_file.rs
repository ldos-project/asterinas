// SPDX-License-Identifier: MPL-2.0

//! TEMPORARY: A version of [`crate::data_capture_file`] that operates on legacy OQueues.
//!
//! This is otherwise identical in structure and purpose to the non-legacy version. See
//! [`crate::data_capture_file`] for full documentation.

use alloc::{boxed::Box, string::ToString, sync::Arc, vec::Vec};
use core::{any::Any, sync::atomic::AtomicBool};

use aster_block::{BLOCK_SIZE, BlockDevice, id::Bid};
use ostd::{
    new_server,
    orpc::{
        errors::RPCError,
        framework::{Server, spawn_thread},
        legacy_oqueue::{Consumer, OQueue, StrongObserver, locking::LockingQueue},
        orpc_impl, orpc_server, orpc_trait,
        path::Path,
        sync::{BlockOnMany, Blocker},
    },
};
use serde::Serialize;

use crate::{DataCaptureError, data_buffering::ChunkingWriteWrapper};

/// TEMPORARY: Registration information for a legacy OQueue observer.
pub struct ObserverRegistration<T: Copy + Send + Serialize> {
    /// TEMPORARY: A strong observer attachment on the legacy OQueue.
    pub observer: Box<dyn StrongObserver<T>>,
}

impl<T: Copy + Send + Serialize> core::fmt::Debug for ObserverRegistration<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ObserverRegistration").finish()
    }
}

#[orpc_trait]
pub trait DataCaptureFile<T: Copy + Send + Serialize>: Any {
    /// TEMPORARY: Attach a new legacy OQueue to the output. If output has already started, then the
    /// path will not appear in the block header.
    fn register_observer(&self, attachment: ObserverRegistration<T>) -> Result<(), RPCError>;
    /// TEMPORARY: Sync writes to disk.
    fn sync(&self) -> Result<(), RPCError>;
    /// TEMPORARY: Enable capturing to this file.
    fn start(&self) -> Result<(), RPCError>;
    /// TEMPORARY: Stop the server
    fn stop(&self) -> Result<(), RPCError>;
}

/// TEMPORARY: Command enum for [`DataCaptureFile`] operations.
enum DataCaptureFileCommand<T: Copy + Send + Serialize + 'static> {
    RegisterObserver(ObserverRegistration<T>),
    Sync,
    Stop,
}

impl<T: Copy + Send + Serialize + 'static> core::fmt::Debug for DataCaptureFileCommand<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RegisterObserver(reg) => {
                f.debug_tuple("DataCaptureFileCommand").field(reg).finish()
            }
            Self::Sync => write!(f, "Sync"),
            Self::Stop => write!(f, "Stop"),
        }
    }
}

#[orpc_server(DataCaptureFile<T>)]
struct DataCaptureFileServer<T: Copy + Send + Serialize + 'static> {
    command_queue: Arc<LockingQueue<DataCaptureFileCommand<T>>>,
    started: AtomicBool,
    stopped: AtomicBool,
}

struct DataCaptureFileServerThread<T: Copy + Send + Serialize + 'static> {
    command_consumer: Box<dyn Consumer<DataCaptureFileCommand<T>>>,
    block_device: Arc<dyn BlockDevice>,
    path: Path,
    start_bid: Bid,
    end_bid: Bid,
    server: Arc<DataCaptureFileServer<T>>,
}

impl<T: Copy + Send + Serialize + 'static> DataCaptureFileServerThread<T> {
    fn run(&self) -> Result<(), DataCaptureError> {
        let mut data_buf_handler = ChunkingWriteWrapper::new(
            BLOCK_SIZE * 2,
            self.block_device.clone(),
            self.start_bid,
            self.end_bid,
        );
        let mut observers: Vec<Box<dyn StrongObserver<T>>> = Default::default();
        let mut headers_written = false;
        let mut block_handler = BlockOnMany::new();

        loop {
            let blockers = [(&*self.command_consumer) as &dyn Blocker]
                .into_iter()
                .chain(observers.iter().map(|o| o.as_ref() as &dyn Blocker));
            block_handler.block_on(blockers);

            // Handle commands from method calls.
            while let Some(cmd) = self.command_consumer.try_consume() {
                match cmd {
                    DataCaptureFileCommand::RegisterObserver(ObserverRegistration { observer }) => {
                        observers.push(observer);
                    }
                    DataCaptureFileCommand::Sync => {
                        data_buf_handler.sync()?;
                    }
                    DataCaptureFileCommand::Stop => {
                        data_buf_handler.sync()?;
                        self.server
                            .stopped
                            .store(true, core::sync::atomic::Ordering::SeqCst);
                        return Ok(());
                    }
                }
            }

            let capturing = self
                .server
                .started
                .load(core::sync::atomic::Ordering::SeqCst);
            // Observe and serialize.
            for o in &observers {
                // We can't skip the try_strong_observe calls when not `capturing` because that
                // would leave the values in the OQueues and block them.
                while let Some(v) = o.try_strong_observe() {
                    if capturing {
                        if !headers_written {
                            data_buf_handler.write_header::<T>(&self.path.to_string(), &[])?;
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
}

#[orpc_impl]
impl<T: Copy + Send + Serialize> DataCaptureFile<T> for DataCaptureFileServer<T> {
    fn register_observer(&self, attachment: ObserverRegistration<T>) -> Result<(), RPCError> {
        self.command_queue
            .produce(DataCaptureFileCommand::RegisterObserver(attachment))
            .unwrap();
        Ok(())
    }

    fn sync(&self) -> Result<(), RPCError> {
        self.command_queue
            .produce(DataCaptureFileCommand::Sync)
            .unwrap();
        Ok(())
    }

    fn stop(&self) -> Result<(), RPCError> {
        self.command_queue
            .produce(DataCaptureFileCommand::Stop)
            .unwrap();

        while !self.stopped.load(core::sync::atomic::Ordering::Relaxed) {
            ostd::task::Task::yield_now();
        }
        Ok(())
    }

    fn start(&self) -> Result<(), RPCError> {
        self.started
            .store(true, core::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

/// TEMPORARY: A builder for a [`DataCaptureFile`] provided by a [`DataCaptureDevice`].
pub struct DataCaptureFileBuilder {
    pub(crate) block_device: Arc<dyn BlockDevice>,
    pub(crate) path: Path,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) server: Arc<dyn Server>,
}

impl DataCaptureFileBuilder {
    /// TEMPORARY: Construct the [`DataCaptureFile`] for a specific type of data.
    pub fn build<T>(self) -> Arc<dyn DataCaptureFile<T>>
    where
        T: Copy + Send + Serialize + 'static,
    {
        Server::orpc_server_base(self.server.as_ref())
            .call_in_context(move || -> Result<Arc<DataCaptureFileServer<T>>, RPCError> {
                let command_queue = LockingQueue::new(8);
                let server = new_server!(|_| DataCaptureFileServer {
                    command_queue: command_queue.clone(),
                    started: AtomicBool::new(false),
                    stopped: AtomicBool::new(false),
                });

                spawn_thread(server.clone(), {
                    let thread = DataCaptureFileServerThread {
                        command_consumer: command_queue
                            .attach_consumer()
                            .expect("single purpose OQueue failed."),
                        block_device: self.block_device,
                        path: self.path,
                        start_bid: Bid::from_offset(self.start),
                        end_bid: Bid::from_offset(self.end),
                        server: server.clone(),
                    };
                    move || Ok(thread.run()?)
                });

                Ok(server)
            })
            .expect("Closure cannot fail")
    }
}
