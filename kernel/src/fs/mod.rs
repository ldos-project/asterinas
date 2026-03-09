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
#[expect(unused_imports)]
use aster_raid::selection_policies::{Dummy0Policy, LinnOSPolicy, RoundRobinPolicy};
use aster_raid::{Raid1Device, Raid1DeviceError};
use aster_virtio::device::block::device::BlockDevice as VirtIoBlockDevice;

use crate::{
    fs::{ext2::Ext2, fs_resolver::FsPath},
    prelude::*,
};

/// Start a thread of the block device to pop requests from the block device's
/// request queue and process them if there are any. If the request queue is empty,
/// the thread will wait until there is a request in the queue.
pub(crate) fn start_block_device(device_name: &str) -> Result<Arc<dyn BlockDevice>> {
    if let Some(device) = aster_block::get_device(device_name) {
        let cloned_device = device.clone();
        let task_fn = move || {
            info!("spawn the virt-io-block thread");
            let virtio_block_device = cloned_device.downcast_ref::<VirtIoBlockDevice>().unwrap();
            loop {
                virtio_block_device.handle_requests();
            }
        };
        crate::ThreadOptions::new(task_fn).spawn();
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
    for &name in RAID_MEMBER_NAMES {
        match start_block_device(name) {
            Ok(device) => {
                info!("[raid] member '{}' online", name);
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
