// SPDX-License-Identifier: MPL-2.0

//! A server for managing data capture block devices.
//!
//! This module provides [`DataCaptureDevice`], which manages block storage devices and creates
//! [`DataCaptureFile`](crate::data_capture_file::DataCaptureFile) instances.

use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

use aster_block::{self, BlockDevice, SECTOR_SIZE};
use ostd::{
    new_server,
    orpc::{errors::RPCError, framework::Server, orpc_impl, orpc_server, orpc_trait, path::Path},
    ostd_error,
};
use snafu::{Snafu, ensure};

use crate::DataCaptureFileBuilder;

/// Describes a file to be created on the device
#[derive(Debug, Clone)]
pub struct FileDescriptor {
    pub length: usize,
    pub path: Path,
}

#[non_exhaustive]
#[ostd_error]
#[derive(Debug, Snafu)]
#[snafu()]
pub enum DataCaptureDeviceError {
    #[snafu(transparent)]
    RPCError { source: RPCError },
    #[snafu(display("Insufficient space on device"))]
    InsufficientSpace {},
}

/// A wrapper around a [`BlockDevice`] which supports creating [`DataCaptureFile`]s.
#[orpc_trait]
pub trait DataCaptureDevice {
    /// Create a new file for capturing data, via a builder. The file will start disabled. You must
    /// call `set_capturing(true)` to start it. This allocates `length` bytes on the device and
    /// returns an error if there is not enough space.
    fn new_file(
        &self,
        descriptor: FileDescriptor,
    ) -> Result<DataCaptureFileBuilder, DataCaptureDeviceError>;
}

/// An implementation of [`DataCaptureDevice`].
#[orpc_server(DataCaptureDevice)]
pub struct DataCaptureDeviceServer {
    block_device: Arc<dyn BlockDevice>,
    next_block_offset: AtomicUsize,
}

impl DataCaptureDeviceServer {
    pub fn new(block_device: Arc<dyn BlockDevice>) -> Arc<DataCaptureDeviceServer> {
        new_server!(|_| DataCaptureDeviceServer {
            block_device,
            next_block_offset: AtomicUsize::new(0),
        })
    }
}

#[orpc_impl]
impl DataCaptureDevice for DataCaptureDeviceServer {
    fn new_file(
        &self,
        descriptor: FileDescriptor,
    ) -> Result<DataCaptureFileBuilder, DataCaptureDeviceError> {
        let start = self
            .next_block_offset
            .fetch_add(descriptor.length, Ordering::Relaxed);
        let end = start + descriptor.length;
        ensure!(
            end <= self.block_device.metadata().nr_sectors * SECTOR_SIZE,
            InsufficientSpaceSnafu
        );
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
