// SPDX-License-Identifier: MPL-2.0

//! An ORPC server which performs data capture into the format described in [the crate
//! documentation](`crate`) using an extraction function and a set of OQueues to observe. Data from
//! different OQueues are not distinguished in the output.
//!
//! This uses the following terms:
//! * "Device" refers to a block storage device.
//! * "File" refers to a range within a device used to store data of a specific type from a set of
//!   registered OQueues.
//! * "Chunk" refers to a chunk of data written to the block device together. Chucks are the size of
//!   a block device block, but the term was just too confusing here.
//!
//! For each block device that will be used for capture, the user should construct a
//! [`DataCaptureDevice`]. This can then be used to construct [`DataCaptureFile`] servers (via a
//! builder due to the generic type).
//!
//! [`DataCaptureFile`] has a thread which will observe all OQueues attach via
//! [`DataCaptureFile::register_observer`].

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{any::Any, error::Error, sync::atomic::AtomicBool};

use aster_time::read_monotonic_time;

use aster_block::{BLOCK_SIZE, BlockDevice, id::Bid};
use binary_serde::BinarySerde;
use ostd::{
    new_server,
    orpc::{
        errors::RPCError,
        framework::{Server, spawn_thread},
        oqueue::{
            ConsumableOQueue as _, ConsumableOQueueRef, Consumer, StrongObserver, ValueProducer,
        },
        orpc_impl, orpc_server, orpc_trait,
        path::Path,
        sync::{BlockOnMany, Blocker},
    },
    path,
};

use crate::data_buffering::ChunkingWriteWrapper;

/// Registration information for an OQueue observer
pub struct ObserverRegistration<T: Copy + Send + BinarySerde> {
    /// The path of the OQueue this is observing
    pub path: Path,
    /// An observer attachment at the appropriate type.
    pub observer: StrongObserver<T>,
}

impl<T: Copy + Send + BinarySerde> core::fmt::Debug for ObserverRegistration<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OQueueAttachment")
            .field("path", &self.path)
            .finish()
    }
}

#[orpc_trait]
pub trait DataCaptureFile<T: Copy + Send + BinarySerde>: Any {
    /// Attach a new OQueue to the output. If output has already started, then the path will not
    /// appear in the block header.
    fn register_observer(&self, attachment: ObserverRegistration<T>) -> Result<(), RPCError>;
    /// Flush any data remaining in the output buffers to disk.
    fn flush(&self) -> Result<(), RPCError>;
    /// Flush All data in the output buffer to disk.
    fn flush_all(&self) -> Result<(), RPCError>;
    /// Flush if data has been observed but not flushed for at least 10 seconds.
    fn timed_flush(&self) -> Result<(), RPCError>;
    /// Sync writes to disk.
    fn sync(&self) -> Result<(), RPCError>;
    /// Enable capturing to this file.
    fn start(&self) -> Result<(), RPCError>;
    /// Stop the server
    fn stop(&self) -> Result<(), RPCError>;
}

/// Command enum for DataCaptureFile operations
enum DataCaptureFileCommand<T: Copy + Send + BinarySerde + 'static> {
    RegisterObserver(ObserverRegistration<T>),
    Flush,
    FlushAll,
    /// Flush only if data has been observed but not yet flushed for at least 10 seconds.
    TimedFlush,
    Sync,
    Stop,
}

impl<T: Copy + Send + BinarySerde + 'static> core::fmt::Debug for DataCaptureFileCommand<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RegisterObserver(arg0) => f.debug_tuple("AttachOqueue").field(arg0).finish(),
            Self::Flush => write!(f, "Flush"),
            Self::FlushAll => write!(f, "FlushAll"),
            Self::TimedFlush => write!(f, "TimedFlush"),
            Self::Sync => write!(f, "Sync"),
            Self::Stop => write!(f, "Stop"),
        }
    }
}

#[orpc_server(DataCaptureFile<T>)]
struct DataCaptureFileServer<T: Copy + Send + BinarySerde + 'static> {
    command_oqueue: ConsumableOQueueRef<DataCaptureFileCommand<T>>,
    command_producer: ValueProducer<DataCaptureFileCommand<T>>,
    started: AtomicBool,
    stopped: AtomicBool,
}

pub struct DataCaptureFileServerThread<T: Copy + Send + BinarySerde + 'static> {
    command_consumer: Consumer<DataCaptureFileCommand<T>>,
    block_device: Arc<dyn aster_block::BlockDevice>,
    start_bid: Bid,
    end_bid: Bid,
    server: Arc<DataCaptureFileServer<T>>,
}

impl<T: Copy + Send + BinarySerde + 'static> DataCaptureFileServerThread<T> {
    fn run(&self) -> Result<(), Box<dyn Error>> {
        let mut data_buf_handler =
            ChunkingWriteWrapper::new(BLOCK_SIZE * 65536, self.block_device.clone(), self.start_bid);
        let mut observers: Vec<StrongObserver<T>> = Default::default();
        // The paths of the attached OQueues. Once the header is written this is set to None and
        // paths are no longer collected even if more OQueues are attached.
        let mut paths = Some(Vec::default());
        let mut block_handler = BlockOnMany::new();
        // Tracks whether unflushed data exists and when the most recent value was observed.
        let mut need_flush = false;
        let mut latest_data_observed_us: Option<u64> = None;

        loop {
            let blockers = [(&self.command_consumer) as &dyn Blocker]
                .into_iter()
                .chain(observers.iter().map(|o| o as &dyn Blocker));
            block_handler.block_on(blockers);

            // Handle commands from method calls
            while let Some(cmd) = self.command_consumer.try_consume() {
                match cmd {
                    DataCaptureFileCommand::RegisterObserver(ObserverRegistration {
                        path,
                        observer,
                    }) => {
                        observers.push(observer);
                        if let Some(paths) = &mut paths {
                            paths.push(path);
                        }
                    }
                    DataCaptureFileCommand::Flush => {
                        data_buf_handler.flush()?;
                    }
                    DataCaptureFileCommand::Sync => {
                        data_buf_handler.sync()?;
                    }
                    DataCaptureFileCommand::FlushAll => {
                        data_buf_handler.flush_all()?;
                    }
                    DataCaptureFileCommand::TimedFlush => {
                        if need_flush {
                            if let Some(last_us) = latest_data_observed_us {
                                let now_us = read_monotonic_time().as_micros() as u64;
                                if now_us.saturating_sub(last_us) > 5000000 {
                                    log::info!("[capture] Timed flush triggered after {} seconds of inactivity", (now_us - last_us) as f64 / 1_000_000.0);
                                    data_buf_handler.flush_all()?;
                                    need_flush = false;
                                    log::info!("[capture] Timed flush completed");
                                }
                            }
                        }
                    }
                    DataCaptureFileCommand::Stop => {
                        self.server
                            .stopped
                            .store(true, core::sync::atomic::Ordering::SeqCst);
                        return Ok(());
                    }
                }
            }

            let started = self
                .server
                .started
                .load(core::sync::atomic::Ordering::SeqCst);
            // Observe and serialize
            for o in &observers {
                // We can't skip the try_strong_observe calls when not `capturing` because that
                // would leave the values in the OQueues and block them.
                let mut drain_count = 0usize;
                while let Ok(Some(v)) = {
                    // Disable IRQs while holding the OQueue's SpinLock inside
                    // try_strong_observe to prevent deadlock with the IRQ handler
                    // that produces to the same OQueue (bio completion stats).
                    let _irq_guard = ostd::trap::irq::disable_local();
                    o.try_strong_observe()
                } {
                    if started {
                        if paths.is_some() {
                            data_buf_handler.write_header::<T>(paths.as_ref().unwrap())?;
                            paths = None;
                        }

                        data_buf_handler.write_value(&v);
                        latest_data_observed_us = Some(read_monotonic_time().as_micros() as u64);
                        need_flush = true;
                        // data_buf_handler.flush_if_needed()?;
                        if data_buf_handler.data_buf.len() % (32 * 1024) == 0 {  // 32 * 1024
                            log::info!("Captured Data from OQueue to Capture Buffer, size of buffer: {}, capacity: {}",
                                data_buf_handler.data_buf.len(),
                                data_buf_handler.data_buf.data.capacity());
                        }
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
impl<T: Copy + Send + BinarySerde> DataCaptureFile<T> for DataCaptureFileServer<T> {
    fn register_observer(&self, attachment: ObserverRegistration<T>) -> Result<(), RPCError> {
        self.command_producer
            .produce(DataCaptureFileCommand::RegisterObserver(attachment));
        Ok(())
    }

    fn flush(&self) -> Result<(), RPCError> {
        self.command_producer.produce(DataCaptureFileCommand::Flush);
        Ok(())
    }

    fn flush_all(&self) -> Result<(), RPCError> {
        self.command_producer.produce(DataCaptureFileCommand::FlushAll);
        Ok(())
    }

    fn timed_flush(&self) -> Result<(), RPCError> {
        self.command_producer.produce(DataCaptureFileCommand::TimedFlush);
        Ok(())
    }

    fn sync(&self) -> Result<(), RPCError> {
        self.command_producer.produce(DataCaptureFileCommand::Sync);
        Ok(())
    }

    fn stop(&self) -> Result<(), RPCError> {
        self.command_producer.produce(DataCaptureFileCommand::Stop);
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

/// A builder for a [`DataCaptureFile`] provided by a [`DataCaptureDevice`]. This is required
/// because ORPC methods cannot take type parameters, so this serves to hold all the information
/// needed to construct the [`DataCaptureFile`] and then take the type parameter that is also
/// needed.
pub struct DataCaptureFileBuilder {
    pub(crate) block_device: Arc<dyn BlockDevice>,
    pub(crate) path: Path,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) server: Arc<dyn Server>,
}

impl DataCaptureFileBuilder {
    /// Construct the [`DataCaptureFile`] for a specific type of data.
    pub fn build<T>(self) -> Arc<dyn DataCaptureFile<T>>
    where
        T: Copy + Send + BinarySerde + 'static,
    {
        // We manually context switch into the server here. This is not something we should
        // generally do, but this build function is required and can't be on a server (due to the
        // need to take a type parameter) and it's body should run in the context of the manager
        // server.
        Server::orpc_server_base(self.server.as_ref())
            .call_in_context(move || -> Result<Arc<DataCaptureFileServer<T>>, RPCError> {
                let command_oqueue =
                    ConsumableOQueueRef::new(8, self.path.append(&path!(commands)));
                let server = new_server!(|_| DataCaptureFileServer {
                    command_producer: command_oqueue
                        .attach_value_producer()
                        .expect("single purpose OQueue failed."),
                    command_oqueue,
                    started: AtomicBool::new(false),
                    stopped: AtomicBool::new(false),
                });

                spawn_thread(server.clone(), {
                    let thread = DataCaptureFileServerThread {
                        command_consumer: server
                            .command_oqueue
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
