// SPDX-License-Identifier: MPL-2.0

//! OQueue data capture utilities.

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::time::Duration;

use log::{error, info};
use ostd::{
    ignore_err, new_server,
    orpc::{
        framework::{notifier::Notifier as _, spawn_thread},
        orpc_server,
    },
    sync::Mutex,
};

use crate::{fs, kcmdline, util::timer::TimerServer};

#[orpc_server]
struct DataCaptureManager {}

impl DataCaptureManager {
    pub fn spawn(period: Duration) -> Arc<Self> {
        let server = new_server!(|_| Self {});
        spawn_thread(server.clone(), {
            let server = server.clone();
            move || server.main(period)
        });
        server
    }

    pub fn main(&self, period: Duration) -> Result<(), Box<dyn core::error::Error>> {
        let notify_server = TimerServer::spawn(period);

        let notify_observer = notify_server
            .notification_oqueue()
            .attach_strong_observer()?;
        loop {
            notify_observer.strong_observe();
            for f in DATA_CAPTURE_FILE_SYNCERS.lock().iter() {
                f();
            }
        }
    }
}

static DATA_CAPTURE_DEVICE_LEGACY: Mutex<
    Option<Arc<dyn mariposa_data_capture::legacy::DataCaptureDevice>>,
> = Mutex::new(None);

static DATA_CAPTURE_DEVICE: Mutex<Option<Arc<dyn mariposa_data_capture::DataCaptureDevice>>> =
    Mutex::new(None);

pub(super) static DATA_CAPTURE_FILE_FINALIZERS: Mutex<Vec<Box<dyn Fn() + Send>>> =
    Mutex::new(Vec::new());

static DATA_CAPTURE_FILE_SYNCERS: Mutex<Vec<Box<dyn Fn() + Send>>> = Mutex::new(Vec::new());

pub(super) fn start_capture_devices() {
    match fs::start_block_device("capture_legacy") {
        Ok(capture_block_device) => {
            DATA_CAPTURE_DEVICE_LEGACY.lock().replace(
                mariposa_data_capture::legacy::DataCaptureDeviceServer::new(capture_block_device),
            );
            info!("[kernel] Initialized legacy data capture device (capture_legacy)");
        }
        Err(e) => error!(
            "[kernel] Failed to initialize legacy data capture device (capture_legacy): {}",
            e
        ),
    }

    match fs::start_block_device("capture") {
        Ok(capture_block_device) => {
            DATA_CAPTURE_DEVICE.lock().replace(
                mariposa_data_capture::DataCaptureDeviceServer::new(capture_block_device),
            );
            info!("[kernel] Initialized new data capture device (capture)");
        }
        Err(e) => error!(
            "[kernel] Failed to initialized new data capture device (capture): {}",
            e
        ),
    }

    // Start a server which syncs the data_capture devices every `secs` seconds based on the
    // kcmdline arg `data_capture.sync_period`. If `data_capture.sync_period` is not provided or is
    // <= 0, then do not sync periodically.
    if let Some(secs) = kcmdline::get_kernel_cmd_line()
        .and_then(|cl| cl.get_module_arg_by_name("data_capture", "sync_period"))
        && secs > 0.0
    {
        DataCaptureManager::spawn(Duration::from_secs_f32(secs));
    }
}

/// Create a new data capture file for legacy OQueues.
pub fn new_legacy_data_capture_file<T: serde::Serialize + Copy + Send + 'static>(
    descriptor: mariposa_data_capture::legacy::FileDescriptor,
) -> Arc<dyn mariposa_data_capture::legacy::DataCaptureFile<T>> {
    let ret = DATA_CAPTURE_DEVICE_LEGACY
        .lock()
        .as_ref()
        .unwrap()
        .new_file(descriptor)
        .unwrap()
        .build();
    DATA_CAPTURE_FILE_FINALIZERS.lock().push(Box::new({
        let ret = ret.clone();
        move || {
            ignore_err!(ret.stop());
            info!("[kernel] Stopped data capture device (capture)");
        }
    }));
    DATA_CAPTURE_FILE_SYNCERS.lock().push(Box::new({
        let ret = ret.clone();
        move || {
            ignore_err!(ret.sync());
            info!("[kernel] Sync'd data capture device (capture)");
        }
    }));
    ret
}

/// Create a new data capture file for OQueues.
pub fn new_data_capture_file<T: serde::Serialize + Copy + Send + 'static>(
    descriptor: mariposa_data_capture::FileDescriptor,
) -> Arc<dyn mariposa_data_capture::DataCaptureFile<T>> {
    let ret = DATA_CAPTURE_DEVICE
        .lock()
        .as_ref()
        .unwrap()
        .new_file(descriptor)
        .unwrap()
        .build();
    DATA_CAPTURE_FILE_FINALIZERS.lock().push(Box::new({
        let ret = ret.clone();
        move || {
            ignore_err!(ret.stop());
            info!("[kernel] Stopped legacy data capture device (capture_legacy)");
        }
    }));
    DATA_CAPTURE_FILE_SYNCERS.lock().push(Box::new({
        let ret = ret.clone();
        move || {
            ignore_err!(ret.sync());
            info!("[kernel] Sync'd legacy data capture device (capture_legacy)");
        }
    }));
    ret
}
