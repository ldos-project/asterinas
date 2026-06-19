// SPDX-License-Identifier: MPL-2.0

//! OQueue data capture utilities.

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{result::Result, time::Duration};

use aster_block::BlockDevice;
use ostd::{
    error, ignore_err, info, new_server,
    orpc::{
        framework::{notifier::Notifier as _, spawn_thread},
        oqueue::{OQueueBase as _, ObservationQuery, registry::lookup_by_type},
        orpc_server,
        path::Path,
    },
    sync::Mutex,
};
use serde::Serialize;

use crate::{kcmdline, prelude::*, util::timer::TimerServer};

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

fn find_block_device(device_name: &str) -> Option<Arc<dyn aster_block::BlockDevice>> {
    aster_block::collect_all()
        .into_iter()
        .find(|d| d.name() == device_name)
}

/// Initializes a data capture device based on a specified kernel argument.
///
/// Retrieves the kernel argument `data_capture.<arg_name>`. If present, attempts to find the
/// corresponding block device. If found, passes the device to `init_device`. If the argument is
/// missing, this logs a warning. If the device can't be found, it logs an error.
fn init_capture_device_from_arg(
    arg_name: &str,
    init_device: impl FnOnce(Arc<dyn BlockDevice + 'static>),
) {
    let cmdline = kcmdline::get_kernel_cmd_line();
    let Some(name) =
        cmdline.and_then(|cl| cl.get_module_arg_by_name::<String>("data_capture", arg_name))
    else {
        warn!(
            "[kernel] Missing argument 'data_capture.{}'; disabling the associated data capture.",
            arg_name
        );
        return;
    };

    match find_block_device(&name) {
        Some(device) => {
            init_device(device);
            info!("[kernel] Initialized data capture device ({})", name);
        }
        None => {
            error!("[kernel] Failed to find data capture device ({})", name);
        }
    }
}

pub(super) fn start_capture_devices() {
    init_capture_device_from_arg("legacy_device", |server| {
        DATA_CAPTURE_DEVICE_LEGACY.lock().replace(
            mariposa_data_capture::legacy::DataCaptureDeviceServer::new(server),
        );
    });

    init_capture_device_from_arg("device", |server| {
        DATA_CAPTURE_DEVICE
            .lock()
            .replace(mariposa_data_capture::DataCaptureDeviceServer::new(server));
    });

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
) -> Option<Arc<dyn mariposa_data_capture::legacy::DataCaptureFile<T>>> {
    let ret = DATA_CAPTURE_DEVICE_LEGACY
        .lock()
        .as_ref()?
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
    Some(ret)
}

/// Create a new data capture file for OQueues.
pub fn new_data_capture_file<T: serde::Serialize + Copy + Send + 'static>(
    descriptor: mariposa_data_capture::FileDescriptor,
) -> Option<Arc<dyn mariposa_data_capture::DataCaptureFile<T>>> {
    let ret = DATA_CAPTURE_DEVICE
        .lock()
        .as_ref()?
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
    Some(ret)
}

/// Capture data from all OQueues of a given type.
///
/// This encapsulates the idea that there may be a bunch of OQueues with the same type that
/// represent the same kind of event for different entities. For example, the I/O operations for
/// different disks.
///
/// The `query` argument is a function which creates `ObservationQuery`s. This additional level of
/// indirection is needed because `ObservationQuery` is not `Clone`.
///
/// There is no way to put the path of the actual source OQueue into the output file via the query.
/// This is because `Path` is not `Copy`, so it can't go in the event. (see
/// https://github.com/ldos-project/asterinas/issues/232)
pub fn new_data_capture_data_file_by_type<
    T: Send + 'static,
    E: Copy + Sync + Send + Serialize + 'static,
>(
    capture_path: Path,
    length: usize,
    query: impl Fn() -> ObservationQuery<T, E>,
) {
    let oqueues = lookup_by_type::<T>();
    if !oqueues.is_empty() {
        let Some(capture_file) =
            new_data_capture_file::<E>(mariposa_data_capture::FileDescriptor {
                path: capture_path.clone(),
                length,
            })
        else {
            return;
        };

        for oqueue in oqueues {
            let Some(oqueue_path) = oqueue.path().cloned() else {
                warn!(
                    "Found anonymous OQueue while collecting OQueues for {}. Not capturing.",
                    capture_path
                );
                continue;
            };
            ignore_err!(capture_file.register_observer(
                mariposa_data_capture::ObserverRegistration {
                    observer: oqueue.attach_strong_observer(query()).unwrap(),
                    path: oqueue_path,
                }
            ));
        }
        ignore_err!(capture_file.start());
    }
}
