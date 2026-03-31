// SPDX-License-Identifier: MPL-2.0

//! OQueue data capture implementation for Mariposa
//!
//! This allows efficiently capturing data from OQueues for external processing. The goal is to
//! capture the data with as little overhead as possible on the running system.
//!
//! ## The binary data format used in Mariposa OQueue data capture.
//!
//! The format is a large raw file with blocks. Each block must be 4k page aligned and is in the
//! following format:
//! * The magic "number": `MARIPOSALDOSDATA` (null terminated).
//! * A [CBOR](https://cbor.io/) object:
//!     ```
//!     { "name": "file.path", "oqueues": ["oqueue.path", ...], "type_name": "TypeName" }
//!     ```
//!   The `oqueues` and `length` fields are optional.
//! * A series of CBOR objects serialized from type `TypeName` using Serde. The CBOR objects are not
//!   part of a list, instead just being concatinated. In Python, they can be read using repeated
//!   calls to
//!   [`cbor2.CBORDecoder.decode`](https://cbor2.readthedocs.io/en/latest/api.html#cbor2.CBORDecoder.decode).
//!
//! The set of OQueue paths is only for convenience, so it can be incomplete or missing. This may be
//! because of set of OQueues was not known when output started.
#![no_std]
#![deny(unsafe_code)]

#[cfg(not(baseline_asterinas))]
mod data_buffering;
#[cfg(not(baseline_asterinas))]
mod data_capture_device;
#[cfg(not(baseline_asterinas))]
mod data_capture_file;
#[cfg(not(baseline_asterinas))]
pub mod legacy;

#[cfg(not(baseline_asterinas))]
pub use data_capture_device::{DataCaptureDevice, DataCaptureDeviceServer, FileDescriptor};
#[cfg(not(baseline_asterinas))]
pub use data_capture_file::{DataCaptureFile, DataCaptureFileBuilder, ObserverRegistration};

extern crate alloc;

use component::{ComponentInitError, init_component};
use ostd::ostd_error;
use snafu::Snafu;

#[non_exhaustive]
#[ostd_error]
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum DataCaptureError {
    #[snafu(transparent)]
    RPCError {
        source: ostd::orpc::errors::RPCError,
    },
    #[snafu(display("Insufficient space on device or in file"))]
    InsufficientSpaceError {},
    #[snafu(transparent)]
    IOError {
        source: aster_block::bio::BioEnqueueError,
    },
}

#[init_component]
fn init() -> Result<(), ComponentInitError> {
    Ok(())
}

#[cfg(all(ktest, not(baseline_asterinas)))]
mod tests {
    use alloc::sync::Arc;
    use core::time::Duration;

    use aster_block::test_utils::MemoryDisk;
    use ostd::{
        assertion::sleep,
        orpc::oqueue::{OQueue, OQueueBase, OQueueRef, ObservationQuery},
        path,
        prelude::*,
    };

    use crate::{
        DataCaptureDevice as _, DataCaptureDeviceServer, FileDescriptor, ObserverRegistration,
    };

    #[ktest]
    fn test_capture_server() {
        // Create memory disk with space for 4 blocks
        let block_device = Arc::new(MemoryDisk::new(4096 * 4));
        let device = DataCaptureDeviceServer::new(block_device.clone());

        let path = path!(test_capture);
        let builder = device
            .new_file(FileDescriptor {
                length: 4096 * 2,
                path: path.clone(),
            })
            .unwrap();
        let server = builder.build();

        // Attach an OQueue to the capture
        let oqueue: OQueueRef<u32> = OQueueRef::new(4, path.clone());
        let attachment = ObserverRegistration {
            path,
            observer: oqueue
                .attach_strong_observer(ObservationQuery::new(|x| *x as u8))
                .unwrap(),
        };
        server.register_observer(attachment).unwrap();

        let producer = oqueue.attach_ref_producer().unwrap();

        // Test capturing disabled initially
        producer.produce_ref(&10);
        sleep(Duration::from_millis(10));

        // Enable capturing
        server.start().unwrap();
        producer.produce_ref(&42);
        producer.produce_ref(&100);
        producer.produce_ref(&200);
        sleep(Duration::from_millis(10));

        // Flush and give time for capture to complete
        server.flush().unwrap();

        sleep(Duration::from_millis(10));

        let device_data = block_device.data.lock();

        let path_bytes: &[u8] = b"test_capture";
        assert!(
            device_data
                .windows(path_bytes.len())
                .any(|window| window == path_bytes)
        );

        // The CBOR encoding of 42, 100, 200
        let data_bytes = [0x18, 0x2a, 0x18, 0x64, 0x18, 0xc8];
        assert!(
            device_data
                .windows(data_bytes.len())
                .any(|window| window == data_bytes)
        );
    }
}
