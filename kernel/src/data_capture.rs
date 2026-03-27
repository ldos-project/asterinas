// SPDX-License-Identifier: MPL-2.0

//! OQueue data capture utilities.

use alloc::{boxed::Box, sync::Arc, vec::Vec};

use log::info;
use ostd::{ignore_err, sync::Mutex};

use crate::fs;

static DATA_CAPTURE_DEVICE_LEGACY: Mutex<
    Option<Arc<dyn mariposa_data_capture::legacy::DataCaptureDevice>>,
> = Mutex::new(None);

static DATA_CAPTURE_DEVICE: Mutex<Option<Arc<dyn mariposa_data_capture::DataCaptureDevice>>> =
    Mutex::new(None);

pub(super) static DATA_CAPTURE_FILE_FINALIZERS: Mutex<Vec<Box<dyn Fn() + Send>>> =
    Mutex::new(Vec::new());

pub(super) fn start_capture_devices() {
    if let Ok(capture_block_device) = fs::start_block_device("capture_legacy") {
        DATA_CAPTURE_DEVICE_LEGACY.lock().replace(
            mariposa_data_capture::legacy::DataCaptureDeviceServer::new(capture_block_device),
        );
        info!("[kernel] Initialized legacy data capture device (capture_legacy)");
    }

    if let Ok(capture_block_device) = fs::start_block_device("capture") {
        DATA_CAPTURE_DEVICE
            .lock()
            .replace(mariposa_data_capture::DataCaptureDeviceServer::new(
                capture_block_device,
            ));
        info!("[kernel] Initialized new data capture device (capture)");
    }
}

/// Create a new data capture file for legacy OQueues.
pub fn new_legacy_data_capture_file<T: serde::Serialize + Copy + Send + 'static>(
    descriptor: mariposa_data_capture::legacy::FileDescriptor,
) -> Arc<dyn mariposa_data_capture::legacy::DataCaptureFile<T>> {
    let data_capture_device = DATA_CAPTURE_DEVICE_LEGACY.lock().clone();
    let ret = data_capture_device
        .unwrap()
        .new_file(descriptor)
        .unwrap()
        .build();
    DATA_CAPTURE_FILE_FINALIZERS.lock().push(Box::new({
        let ret = ret.clone();
        move || {
            ignore_err!(ret.stop());
            info!("[kernel] Sync'd data capture device (capture)");
        }
    }));
    ret
}

/// Create a new data capture file for OQueues.
pub fn new_data_capture_file<T: serde::Serialize + Copy + Send + 'static>(
    descriptor: mariposa_data_capture::FileDescriptor,
) -> Arc<dyn mariposa_data_capture::DataCaptureFile<T>> {
    let data_capture_device = DATA_CAPTURE_DEVICE.lock().clone();
    let ret = data_capture_device
        .unwrap()
        .new_file(descriptor)
        .unwrap()
        .build();
    DATA_CAPTURE_FILE_FINALIZERS.lock().push(Box::new({
        let ret = ret.clone();
        move || {
            ignore_err!(ret.stop());
            info!("[kernel] Sync'd legacy data capture device (capture_legacy)");
        }
    }));
    ret
}
