// SPDX-License-Identifier: MPL-2.0

//! TEMPORARY: Legacy OQueue variants of [`DataCaptureDevice`] and [`DataCaptureFile`].
//!
//! These are structurally identical to the top-level versions but accept
//! [`ostd::orpc::legacy_oqueue`] strong observers instead of new-OQueue observers.

// TODO(arthurp, after SOSP): This entire module is temporary for a research project. It will be removed very
// soon.

mod data_capture_device;
mod data_capture_file;

pub use data_capture_device::{DataCaptureDevice, DataCaptureDeviceServer, FileDescriptor};
pub use data_capture_file::{DataCaptureFile, DataCaptureFileBuilder, ObserverRegistration};

#[cfg(all(ktest, not(baseline_asterinas)))]
mod legacy_tests {
    use alloc::sync::Arc;
    use core::time::Duration;

    use aster_block::test_utils::MemoryDisk;
    use ostd::{
        assertion::sleep,
        orpc::legacy_oqueue::{OQueue as _, locking::ObservableLockingQueue},
        prelude::*,
    };

    use crate::legacy::{
        DataCaptureDevice as _, DataCaptureDeviceServer, FileDescriptor, ObserverRegistration,
    };

    #[ktest]
    fn test_legacy_capture_server() {
        // Create memory disk with space for 4 blocks
        let block_device = Arc::new(MemoryDisk::new(4096 * 4));
        let device = DataCaptureDeviceServer::new(block_device.clone());

        let builder = device
            .new_file(FileDescriptor { length: 4096 * 2 })
            .unwrap();
        let server = builder.build::<u8>();

        // Attach a legacy OQueue to the capture
        let oqueue = ObservableLockingQueue::<u8>::new(4, 8);
        let attachment = ObserverRegistration {
            observer: oqueue.attach_strong_observer().unwrap(),
        };
        server.register_observer(attachment).unwrap();

        let producer = oqueue.attach_producer().unwrap();

        // Test capturing disabled initially
        producer.produce(10u8);
        sleep(Duration::from_millis(10));

        // Enable capturing
        server.set_capturing(true).unwrap();
        producer.produce(42u8);
        producer.produce(100u8);
        producer.produce(200u8);
        sleep(Duration::from_millis(10));

        // Flush and give time for capture to complete
        server.flush().unwrap();

        sleep(Duration::from_millis(10));

        let device_data = block_device.data.lock();

        assert_eq!(device_data[64..64 + 3], [42, 100, 200]);
    }
}
