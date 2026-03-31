// SPDX-License-Identifier: MPL-2.0

//! A server for managing data capture block devices.
//!
//! This module provides [`DataCaptureDevice`], which manages block storage devices and creates
//! [`DataCaptureFile`](crate::data_capture_file::DataCaptureFile) instances.

use alloc::{
    string::{String, ToString as _},
    sync::Arc,
};
use core::sync::atomic::{AtomicUsize, Ordering};

use aster_block::{self, BLOCK_SIZE, BlockDevice, SECTOR_SIZE, id::Bid};
use ostd::{
    new_server,
    orpc::{framework::Server, orpc_impl, orpc_server, orpc_trait, path::Path},
    sync::Mutex,
};
use serde::Serialize;
use snafu::ensure;

use crate::{
    DataCaptureError, DataCaptureFileBuilder, InsufficientSpaceSnafu,
    data_buffering::ChunkingWriteWrapper,
};

/// Describes a file to be created on the device
#[derive(Debug, Clone)]
pub struct FileDescriptor {
    pub length: usize,
    pub path: Path,
}

const DIRECTORY_BLOCKS: usize = 1;

/// A wrapper around a [`BlockDevice`] which supports creating [`DataCaptureFile`]s.
#[orpc_trait]
pub trait DataCaptureDevice {
    /// Create a new file for capturing data, via a builder. The file will start disabled. You must
    /// call `start()` to start it. This allocates `length` bytes on the device and
    /// returns an error if there is not enough space.
    fn new_file(
        &self,
        descriptor: FileDescriptor,
    ) -> Result<DataCaptureFileBuilder, DataCaptureError>;
}

/// An implementation of [`DataCaptureDevice`].
#[orpc_server(DataCaptureDevice)]
pub struct DataCaptureDeviceServer {
    block_device: Arc<dyn BlockDevice>,
    next_block_offset: AtomicUsize,
    directory_writer: Mutex<ChunkingWriteWrapper>,
}

impl DataCaptureDeviceServer {
    pub fn new(block_device: Arc<dyn BlockDevice>) -> Arc<DataCaptureDeviceServer> {
        new_server!(|_| DataCaptureDeviceServer {
            directory_writer: Mutex::new(ChunkingWriteWrapper::new(
                BLOCK_SIZE * 2,
                block_device.clone(),
                Bid::from_offset(0),
                Bid::from_offset(DIRECTORY_BLOCKS * BLOCK_SIZE)
            )),
            block_device,
            next_block_offset: AtomicUsize::new(DIRECTORY_BLOCKS * BLOCK_SIZE),
        })
    }
}

#[orpc_impl]
impl DataCaptureDevice for DataCaptureDeviceServer {
    fn new_file(
        &self,
        descriptor: FileDescriptor,
    ) -> Result<DataCaptureFileBuilder, DataCaptureError> {
        let length = descriptor.length.next_multiple_of(BLOCK_SIZE);
        let start = self.next_block_offset.fetch_add(length, Ordering::Relaxed);
        let end = start + length;
        ensure!(
            end <= self.block_device.metadata().nr_sectors * SECTOR_SIZE,
            InsufficientSpaceSnafu
        );

        #[derive(Serialize)]
        struct FileRecord {
            offset: u64,
            length: u64,
            path: String,
        }

        let mut writer = self.directory_writer.lock();
        writer.write_value(&FileRecord {
            offset: start as u64,
            length: length as u64,
            path: descriptor.path.to_string(),
        });
        writer.sync()?;
        drop(writer);

        Ok(DataCaptureFileBuilder {
            block_device: self.block_device.clone(),
            path: descriptor.path,
            start,
            end,
            // TODO(arthurp): This uses the internal ORPC base. A public API for this should probably exist.
            server: self.orpc_server_base().get_ref().unwrap(),
        })
    }
}
