// SPDX-License-Identifier: MPL-2.0

pub mod device;
pub mod devpts;
pub mod epoll;
pub mod exfat;
pub mod ext2;
pub mod file_handle;
pub mod file_table;
pub mod fs_resolver;
pub mod inode_handle;
pub mod named_pipe;
pub mod overlayfs;
pub mod path;
pub mod pipe;
pub mod procfs;
pub mod ramfs;
pub mod rootfs;
#[cfg(not(baseline_asterinas))]
pub mod server_traits;
pub mod sysfs;
pub mod thread_info;
pub mod utils;

use aster_block::BlockDevice;
#[cfg(not(baseline_asterinas))]
use aster_raid::selection_policies::{RoundRobinPolicy, Dummy0Policy};
use aster_raid::{Raid1Device, Raid1DeviceError};
use aster_virtio::device::block::device::BlockDevice as VirtIoBlockDevice;

use crate::{
    fs::{ext2::Ext2, fs_resolver::FsPath},
    prelude::*,
};

/// Start a thread of the block device to pop requests from the block device's
/// request queue and process them if there are any. If the request queue is empty,
/// the thread will wait until there is a request in the queue.
fn start_block_device(device_name: &str) -> Result<Arc<dyn BlockDevice>> {
    if let Some(device) = aster_block::get_device(device_name) {
        let cloned_device = device.clone();
        let task_fn = move || {
            info!("spawn the virt-io-block thread");
            let virtio_block_device = cloned_device.downcast_ref::<VirtIoBlockDevice>().unwrap();
            loop {
                virtio_block_device.handle_requests();
            }
        };
        // Elevate to RealTime 50 so these I/O threads are not starved by other RealTime threads.
        crate::ThreadOptions::new(task_fn)
            .sched_policy(crate::sched::SchedPolicy::RealTime {
                rt_prio: 50.try_into().unwrap(),
                rt_policy: crate::sched::RealTimePolicy::RoundRobin { base_slice_factor: None },
            })
            .spawn();
        Ok(device)
    } else {
        return_errno_with_message!(Errno::ENOENT, "Device does not exist")
    }
}

pub fn lazy_init() {
    //The device name is specified in qemu args as --serial={device_name}
    let ext2_device_name = "vext2";
    let raid1_device_name = "raid_device";

    if let Ok(block_device_ext2) = start_block_device(ext2_device_name) {
        let ext2_fs = Ext2::open(block_device_ext2).unwrap();
        let target_path = FsPath::try_from("/ext2").unwrap();
        self::rootfs::mount_fs_at(ext2_fs, &target_path).unwrap();
        info!("[kernel] Mounted Ext2 fs at {:?} ", target_path);
    }

    // Starting the ExFat filesystem cause hanging at boot.
    // See issue: https://github.com/ldos-project/asterinas/issues/149
    // let exfat_device_name = "vexfat";
    // if let Ok(block_device_exfat) = start_block_device(exfat_device_name) {
    //     let exfat_fs = ExfatFS::open(block_device_exfat, ExfatMountOptions::default()).unwrap();
    //     let target_path = FsPath::try_from("/exfat").unwrap();
    //     self::rootfs::mount_fs_at(exfat_fs, &target_path).unwrap();
    //     info!("[kernel] Mount ExFat fs at {:?} ", target_path);
    // }

    // single disk benchmark
    // let nvme_device_name = "raid0";
    // if let Ok(block_device_nvme) = start_block_device(nvme_device_name) {
    //     let nvme_fs = Ext2::open(block_device_nvme).unwrap();
    //     let target_path = FsPath::try_from("/raid1").unwrap();
    //     self::rootfs::mount_fs_at(nvme_fs, &target_path).unwrap();
    //     info!("[kernel] Mounted NVMe fs at {:?} ", target_path);
    // } else {
    //     error!("[kernel] Failed to start NVMe block device '{}'", nvme_device_name);
    // }
    // return;

    info!("[raid] initializing RAID-1 device: {:?}", raid1_device_name);
    if let Err(err) = setup_raid1_device(raid1_device_name) {
        error!("[raid] failed to setup RAID-1 device: {:?}", err);
    }

    if let Some(raid) = aster_block::get_device(raid1_device_name) {

        match Ext2::open(raid) {
            Ok(raid_fs) => {
                let target_path = FsPath::try_from("/raid1").unwrap();
                self::rootfs::mount_fs_at(raid_fs, &target_path).unwrap();
                info!("[kernel] Mounted RAID-1 at {:?} ", target_path);
            }
            Err(err) => {
                error!("[raid] failed to mount RAID-1 at /raid1: {:?}", err);
            }
        }
    } else {
        error!("[raid] failed to get RAID-1 device: {:?}", Errno::ENOENT);
    }
}

fn setup_raid1_device(raid_device_name: &str) -> Result<()> {
    const RAID_MEMBER_NAMES: &[&str] = &["raid0", "raid1", "raid2"];
    info!(
        "[raid] initializing RAID-1 '{}' with members {:?}",
        raid_device_name, RAID_MEMBER_NAMES
    );

    let mut members = Vec::with_capacity(RAID_MEMBER_NAMES.len());

    // Start the RAID-1's underlying member devices.
    for (index, &name) in RAID_MEMBER_NAMES.iter().enumerate() {
        match start_block_device(name) {
            Ok(device) => {
                info!("[raid] member '{}' online", name);
                if let Some(virtio_dev) = device.downcast_ref::<VirtIoBlockDevice>() {
                    virtio_dev.set_device_index(index as u64);
                }
                members.push(device);
            }
            Err(err) => {
                error!(
                    "[raid] failed to start member '{}': {:?}. RAID-1 init aborted",
                    name, err
                );
                return Err(err);
            }
        }
    }

    // #[cfg(not(baseline_asterinas))]
    setup_data_capture(&members, RAID_MEMBER_NAMES);

    #[cfg(not(baseline_asterinas))]
    info!("[raid] creating selection policy");
    #[cfg(not(baseline_asterinas))]
    let selection_policy = RoundRobinPolicy::new(members.clone()).unwrap();
    #[cfg(not(baseline_asterinas))]
    let raid1device = Raid1Device::init(raid_device_name, members, selection_policy);
    #[cfg(baseline_asterinas)]
    let raid1device = Raid1Device::init(raid_device_name, members);
    raid1device.map_err(|err| match err {
        Raid1DeviceError::NotEnoughMembers => {
            Error::with_message(Errno::EINVAL, "RAID-1 device requires at least two members")
        }
    })?;
    info!("[raid] RAID-1 device created");

    let worker = aster_block::get_device(raid_device_name).unwrap();
    // The registry stores `Arc<dyn BlockDevice>`. Use `downcast_ref` on the captured Arc each
    // iteration to call the RAID-specific helper without needing ownership of `Raid1Device`.
    // TODO(Yingqi): Merge the starting of the RAID-1 thread inside block device server.
    let task_fn = move || {
        info!("spawn the RAID-1 device thread");
        let raid = worker.downcast_ref::<Raid1Device>().unwrap();
        loop {
            raid.handle_requests();
        }
    };

    crate::ThreadOptions::new(task_fn).sched_policy(crate::sched::SchedPolicy::RealTime { 
        rt_prio: 50.try_into().unwrap(), 
        rt_policy: crate::sched::RealTimePolicy::RoundRobin { base_slice_factor: None }, 
    }).spawn();

    info!(
        "[raid] RAID-1 device '{}' registered and worker thread spawned",
        raid_device_name
    );
    Ok(())
}

/// Set up data capture for the RAID-1 member devices' bio completion stats.
///
/// This starts the capture block device and uses the legacy `DataCaptureDevice` /
/// `DataCaptureFile` server to observe each member's `bio_completion_oqueue` and write the
/// serialized data to disk.
#[cfg(not(baseline_asterinas))]
fn setup_data_capture(
    members: &[Arc<dyn BlockDevice>],
    member_names: &[&str],
) {
    use aster_block::{SECTOR_SIZE, bio::BlockDeviceCompletionStats};
    use aster_virtio::device::block::server_traits::BlockIOObservable as _;
    use mariposa_data_capture::{
        DataCaptureDevice as _, DataCaptureDeviceServer, DataCaptureFile as _, FileDescriptor,
        ObserverRegistration,
    };
    use ostd::orpc::oqueue::{OQueueBase as _, ObservationQuery};

    // Start the capture block device
    // let capture_dev = match start_block_device("capture") {
    //     Ok(dev) => dev,
    //     Err(e) => {
    //         error!("[capture] failed to start capture device: {:?}", e);
    //         return;
    //     }
    // };
    let device_name = "capture";
    let capture_dev = aster_block::get_device(device_name).unwrap_or_else(|| {
        panic!("[capture] failed to get capture device '{}'", device_name);
    });
    let cloned_device = capture_dev.clone();
    let task_fn = move || {
        info!("[capture] spawn the virt-io-block thread for the capturing device");
        let virtio_block_device = cloned_device.downcast_ref::<VirtIoBlockDevice>().unwrap();
        loop {
            virtio_block_device.handle_requests();
        }
    };
    crate::ThreadOptions::new(task_fn).sched_policy(crate::sched::SchedPolicy::RealTime { 
        rt_prio: 50.try_into().unwrap(), 
        rt_policy: crate::sched::RealTimePolicy::RoundRobin { base_slice_factor: None }, 
    }).spawn();


    // Display the capture device backend info
    let capture_size = capture_dev.metadata().nr_sectors * SECTOR_SIZE;
    info!(
        "[capture] capture device online, size = {} bytes",
        capture_size
    );

    // Create the data capture device and file
    let capture_device = DataCaptureDeviceServer::new(capture_dev.clone());
    let capture_path = ostd::path!(data_capture.bio_completion);
    let capture_file = match capture_device.new_file(FileDescriptor { length: 65536, path: capture_path.clone() }) {  // 512MB * 1024 * 1024 / 2 / 4096  (using half of the space, and number of pages here)
        Ok(builder) => builder.build::<BlockDeviceCompletionStats>(),
        Err(e) => {
            error!("[capture] failed to create capture file: {:?}", e);
            return;
        }
    };

    // Attach a strong observer to each RAID member's bio_completion_oqueue
    // and register it directly with the capture file.
    for (member, &name) in members.iter().zip(member_names.iter()) {  // (member, name)
        let virtio_dev = member.downcast_ref::<VirtIoBlockDevice>().unwrap();
        let oqueue = virtio_dev.bio_completion_oqueue();
        let observer_path = capture_path.append(&ostd::path!({name}));
        match oqueue.attach_strong_observer(ObservationQuery::identity()) {
            Ok(observer) => {
                let registration = ObserverRegistration { path: observer_path, observer };
                if let Err(e) = capture_file.register_observer(registration) {
                    error!("[capture] failed to register observer for '{}': {:?}", name, e);
                } else {
                    info!("[capture] attached observer to '{}'", name);
                }
            }
            Err(e) => {
                error!("[capture] failed to attach observer to '{}': {:?}", name, e);
            }
        }
        // match oqueue.attach_weak_observer(1, ObservationQuery::identity()) {
        //     Ok(observer) => {
        //         let registration = ObserverRegistration { path: observer_path, observer };
        //         if let Err(e) = capture_file.register_observer(registration) {
        //             error!("[capture] failed to register observer for '{}': {:?}", name, e);
        //         } else {
        //             info!("[capture] attached observer to '{}'", name);
        //         }
        //     }
        //     Err(e) => {
        //         error!("[capture] failed to attach observer to '{}': {:?}", name, e);
        //     }
        // }
    }

    // Enable capturing
    if let Err(e) = capture_file.start() {
        error!("[capture] failed to enable capturing: {:?}", e);
    }

    // Spawn a timer task that sends TimedFlush every 10 seconds to trigger
    // a flush if data has been idle for that long.
    let capture_file_for_timer = capture_file.clone();
    crate::ThreadOptions::new(move || {
        use core::time::Duration;
        use ostd::timer::Jiffies;
        loop {
            let target = Jiffies::elapsed().as_duration() + Duration::from_secs(5);
            while Jiffies::elapsed().as_duration() < target {
                ostd::task::Task::yield_now();
            }
            if let Err(e) = capture_file_for_timer.timed_flush() {
                log::error!("[capture] timed_flush failed: {:?}", e);
            }
        }
    })
    .spawn();

    info!("[capture] data capture enabled for bio completion stats");
}
