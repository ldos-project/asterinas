// SPDX-License-Identifier: MPL-2.0

//! A thread which performs data capture into the format described in [the crate
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
//! capture device. This can then be used to construct capture files (via a
//! builder due to the generic type).
//!
//! A capture file has a thread which will observe all OQueues attach via
//! `register_observer`.

use alloc::{boxed::Box, string::ToString as _, sync::Arc, vec::Vec};
use core::sync::atomic::AtomicBool;

use aster_block::{BLOCK_SIZE, BlockDevice, id::Bid};
use ostd::{
    orpc::{
        oqueue::{
            ConsumableOQueue as _, ConsumableOQueueRef, Consumer, StrongObserver, ValueProducer,
        },
        path::Path,
        sync::{BlockOnMany, Blocker},
    },
    path,
    sync::WaitQueue,
};
use serde::Serialize;

use crate::{DataCaptureError, data_buffering::ChunkingWriteWrapper};

/// Registration information for an OQueue observer
pub struct ObserverRegistration<T: Copy + Send + Serialize> {
    /// The path of the OQueue this is observing
    pub path: Path,
    /// An observer attachment at the appropriate type.
    pub observer: StrongObserver<T>,
}

impl<T: Copy + Send + Serialize> core::fmt::Debug for ObserverRegistration<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ObserverRegistration")
            .field("path", &self.path)
            .finish()
    }
}

/// Command enum for capture file operations
enum DataCaptureFileCommand<T: Copy + Send + Serialize + 'static> {
    RegisterObserver(ObserverRegistration<T>),
    WriteValues(Box<dyn Iterator<Item = T> + Send>),
    Sync,
    Stop,
}

impl<T: Copy + Send + Serialize + 'static> core::fmt::Debug for DataCaptureFileCommand<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RegisterObserver(arg0) => {
                f.debug_tuple("DataCaptureFileCommand").field(arg0).finish()
            }
            Self::WriteValues(_) => write!(f, "WriteValues"),
            Self::Sync => write!(f, "Sync"),
            Self::Stop => write!(f, "Stop"),
        }
    }
}

/// Writes the block header if it has not been written yet, consuming the collected OQueue paths so
/// later attachments no longer accumulate into it.
fn write_header_once<T>(
    writer: &mut ChunkingWriteWrapper,
    name: &Path,
    paths: &mut Option<Vec<Path>>,
) -> Result<(), DataCaptureError> {
    let Some(collected) = paths.take() else {
        return Ok(());
    };
    writer.write_header::<T>(&name.to_string(), &collected)
}

pub struct DataCaptureFile<T: Copy + Send + Serialize + 'static> {
    command_oqueue: ConsumableOQueueRef<DataCaptureFileCommand<T>>,
    command_producer: ValueProducer<DataCaptureFileCommand<T>>,
    started: AtomicBool,
    stopped: AtomicBool,
    stopped_wait_queue: WaitQueue,
}

pub(crate) struct DataCaptureFileThread<T: Copy + Send + Serialize + 'static> {
    path: Path,
    command_consumer: Consumer<DataCaptureFileCommand<T>>,
    block_device: Arc<dyn BlockDevice>,
    start_bid: Bid,
    end_bid: Bid,
    server: Arc<DataCaptureFile<T>>,
}

impl<T: Copy + Send + Serialize + 'static> DataCaptureFileThread<T> {
    fn run(&self) -> Result<(), DataCaptureError> {
        let mut data_buf_handler = ChunkingWriteWrapper::new(
            BLOCK_SIZE * 2,
            self.block_device.clone(),
            self.start_bid,
            self.end_bid,
        );
        let mut observers: Vec<StrongObserver<T>> = Default::default();
        // The paths of the attached OQueues. Once the header is written this is set to None and
        // paths are no longer collected even if more OQueues are attached.
        let mut paths = Some(Vec::default());
        let mut block_handler = BlockOnMany::new();

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
                    DataCaptureFileCommand::WriteValues(values) => {
                        write_header_once::<T>(&mut data_buf_handler, &self.path, &mut paths)?;
                        for value in values {
                            data_buf_handler.write_value(&value);
                            data_buf_handler.flush_if_needed()?;
                        }
                        data_buf_handler.sync()?;
                    }
                    DataCaptureFileCommand::Sync => {
                        data_buf_handler.sync()?;
                    }
                    DataCaptureFileCommand::Stop => {
                        data_buf_handler.sync()?;
                        self.server
                            .stopped
                            .store(true, core::sync::atomic::Ordering::Release);
                        self.server.stopped_wait_queue.wake_all();
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
                while let Ok(Some(v)) = o.try_strong_observe() {
                    if started {
                        write_header_once::<T>(&mut data_buf_handler, &self.path, &mut paths)?;

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

impl<T: Copy + Send + Serialize + 'static> DataCaptureFile<T> {
    /// Attach a new OQueue to the output. If output has already started, then the path will not
    /// appear in the block header.
    pub fn register_observer(
        &self,
        attachment: ObserverRegistration<T>,
    ) -> Result<(), DataCaptureError> {
        self.command_producer
            .produce(DataCaptureFileCommand::RegisterObserver(attachment));
        Ok(())
    }

    /// Write values the caller already holds, without routing them through an OQueue.
    pub fn write_values(
        &self,
        values: Box<dyn Iterator<Item = T> + Send>,
    ) -> Result<(), DataCaptureError> {
        self.command_producer
            .produce(DataCaptureFileCommand::WriteValues(values));
        Ok(())
    }

    /// Sync writes to disk.
    pub fn sync(&self) -> Result<(), DataCaptureError> {
        self.command_producer.produce(DataCaptureFileCommand::Sync);
        Ok(())
    }

    /// Stop the capture thread.
    pub fn stop(&self) -> Result<(), DataCaptureError> {
        if !self.stopped.load(core::sync::atomic::Ordering::Acquire) {
            self.command_producer.produce(DataCaptureFileCommand::Stop);
        }
        self.stopped_wait_queue.wait_until(|| {
            self.stopped
                .load(core::sync::atomic::Ordering::SeqCst)
                .then_some(())
        });
        Ok(())
    }

    /// Enable capturing to this file.
    pub fn start(&self) -> Result<(), DataCaptureError> {
        self.started
            .store(true, core::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

/// A builder for a capture file provided by a capture device.
/// The file type is chosen after allocating space on the device.
pub struct DataCaptureFileBuilder {
    pub(crate) block_device: Arc<dyn BlockDevice>,
    pub(crate) path: Path,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl DataCaptureFileBuilder {
    /// Construct the capture file for a specific type of data.
    pub fn build<T>(self) -> Arc<DataCaptureFile<T>>
    where
        T: Copy + Send + Serialize + 'static,
    {
        let command_oqueue = ConsumableOQueueRef::new(8, self.path.append(&path!(commands)));
        let command_producer = command_oqueue
            .attach_value_producer()
            .expect("single purpose OQueue failed.");
        let server = Arc::new(DataCaptureFile {
            command_oqueue,
            command_producer,
            started: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            stopped_wait_queue: WaitQueue::new(),
        });

        let thread = DataCaptureFileThread {
            path: self.path,
            command_consumer: server
                .command_oqueue
                .attach_consumer()
                .expect("single purpose OQueue failed."),
            block_device: self.block_device,
            start_bid: Bid::from_offset(self.start),
            end_bid: Bid::from_offset(self.end),
            server: server.clone(),
        };
        ostd::task::spawn_thread(move || Ok(thread.run()?));

        server
    }
}
