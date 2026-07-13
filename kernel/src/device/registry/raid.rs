// SPDX-License-Identifier: MPL-2.0

//! RAID-1 logical block device assembly.
//!
//! This module assembles the RAID-1 device from its virtio member disks and
//! registers it with the block layer. The device then gets a devtmpfs node
//! (`/dev/raid`) like any other block device, so user space can mount a
//! filesystem on it (e.g., `mount -t ext2 /dev/raid /raid1`).

#[cfg(not(baseline_asterinas))]
use aster_raid::selection_policies::RoundRobinPolicy;
use aster_raid::{Raid1Device, Raid1DeviceError};
use aster_virtio::device::block::device::BlockDevice as VirtIoBlockDevice;
use device_id::{DeviceId, MinorId};
use spin::Once;

use crate::{
    kcmdline,
    prelude::*,
    sched::{RealTimePolicy, SchedPolicy},
    thread::kernel_thread::ThreadOptions,
};

const RAID_DEVICE_NAME: &str = "raid";

/// The kernel command-line module and argument naming the RAID-1 member disks.
///
/// The member disks are configured via `raid.members=<name>,<name>,...`, a
/// comma-separated list of block device names (e.g. `raid.members=vdc,vdd,vde`).
/// The list is ordered: the position of each name is used as the member's
/// logical index (see [`tagged_member_index`]).
const RAID_MODULE_NAME: &str = "raid";
const RAID_MEMBERS_ARG: &str = "members";

/// Owns the dynamically allocated major ID of the RAID-1 device.
static RAID_MAJOR: Once<aster_block::MajorIdOwner> = Once::new();  // FIXME: Yingqi

/// A magic tag identifying `device_index` values that belong to a RAID-1
/// member, encoded in the high 32 bits (the low 32 bits hold the member's
/// position). `device_index` is a bare `u64` with no other field to say
/// which device family it came from, and it flows into the system-wide
/// `io.block.completion` capture stream (see `init.rs`) alongside every
/// other block device's stats. Without a tag, a RAID member's index would
/// be indistinguishable from any other device in the captured data. 
/// With this magic prefix, RAID member device id will look like 
/// `0x5241_4431_xxxx_xxxx`plus the member index. The magic tag decodes 
/// back to ASCII "RAD1".
const RAID1_MEMBER_INDEX_TAG: u64 = (u32::from_be_bytes(*b"RAD1") as u64) << 32;

/// Tags a RAID-1 member's position with [`RAID1_MEMBER_INDEX_TAG`].
fn tagged_member_index(position: u32) -> u64 {
    RAID1_MEMBER_INDEX_TAG | position as u64
}

/// Assembles and registers the RAID-1 device.
///
/// This must run after the member disks' worker threads have been spawned
/// (see `block::init_in_first_kthread`), and before `init_in_first_process`
/// creates the devtmpfs nodes. This is called in `kernel/src/device/registry/mod.rs` 
/// after the block devices have been registered and spawned.
pub(super) fn init_in_first_kthread() {
    if let Err(err) = setup_raid1_device() {
        error!("[raid] failed to set up the RAID-1 device: {:?}", err);
    }
}

/// This is the setup_raid1_device() function migrated from the initialization 
/// prior to the big merge. 
fn setup_raid1_device() -> Result<()> {
    let members = collect_members()?;  // Collect member devices
    let raid_id = allocate_raid_device_id()?;

    #[cfg(not(baseline_asterinas))]
    let init_result = {
        let selection_policy = RoundRobinPolicy::new(members.clone())?;
        Raid1Device::init(RAID_DEVICE_NAME, raid_id, members, selection_policy)
    };
    #[cfg(baseline_asterinas)]
    let init_result = Raid1Device::init(RAID_DEVICE_NAME, raid_id, members);

    init_result.map_err(|err| match err {
        Raid1DeviceError::NotEnoughMembers => {
            Error::with_message(Errno::EINVAL, "RAID-1 device requires at least two members")
        }
        Raid1DeviceError::BlockError(_) => {
            Error::with_message(Errno::EEXIST, "failed to register the RAID-1 device")
        }
    })?;

    spawn_worker_thread(raid_id);

    info!(
        "[raid] RAID-1 device '{}' registered and worker thread spawned",
        RAID_DEVICE_NAME
    );
    Ok(())
}

/// Collects the RAID-1 member devices named on the kernel command line.
///
/// The member disk names are read from the `raid.members` argument (see
/// [`RAID_MEMBERS_ARG`]) rather than hardcoded, so the disk layout can be
/// reconfigured from `OSDK.toml` / `tools/qemu_args.sh` without touching the
/// kernel. Devices are looked up by name via [`aster_block::collect_all`]
/// (the refactored registry keys devices by `DeviceId`, and virtio disks are
/// auto-named `vda`, `vdb`, ... in probe order).
fn collect_members() -> Result<Vec<Arc<dyn aster_block::BlockDevice>>> {
    let Some(members_arg) = kcmdline::get_kernel_cmd_line()
        .and_then(|cl| cl.get_module_arg_by_name::<String>(RAID_MODULE_NAME, RAID_MEMBERS_ARG))
    else {
        return_errno_with_message!(
            Errno::EINVAL,
            "the RAID-1 member disks are not configured (set 'raid.members=<name>,...')"
        );
    };

    let member_names: Vec<&str> = members_arg
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();

    let all_devices = aster_block::collect_all();

    let mut members = Vec::with_capacity(member_names.len());
    for (index, name) in member_names.iter().enumerate() {
        let Some(device) = all_devices.iter().find(|device| device.name() == *name) else {
            return_errno_with_message!(Errno::ENOENT, "a RAID-1 member disk is missing");
        };

        // Tag the member with its logical index, which is used to attribute
        // I/O completion stats to the right member.
        if let Some(virtio_device) = device.downcast_ref::<VirtIoBlockDevice>() {
            virtio_device.set_device_index(tagged_member_index(index as u32));
        }
        members.push(device.clone());
    }

    Ok(members)
}

fn allocate_raid_device_id() -> Result<DeviceId> {
    let major = aster_block::allocate_major()
        .map_err(|_| Error::with_message(Errno::EBUSY, "no major ID is available for RAID-1"))?;
    let id = DeviceId::new(major.get(), MinorId::new(0));

    // The devtmpfs node resolves back to the device via this ID, so the
    // major ID must stay allocated for the lifetime of the kernel.
    RAID_MAJOR.call_once(|| major);

    Ok(id)
}

fn spawn_worker_thread(raid_id: DeviceId) {
    let device = aster_block::lookup(raid_id).unwrap();
    let task_fn = move || {
        info!("spawn the RAID-1 device thread");
        let raid = device.downcast_ref::<Raid1Device>().unwrap();
        loop {
            raid.handle_requests();
        }
    };

    // Elevate to RealTime 50 so this I/O thread is not starved by other
    // threads, which would otherwise cause long tail latencies.
    ThreadOptions::new(task_fn)
        .sched_policy(SchedPolicy::RealTime {
            rt_prio: 50.try_into().unwrap(),
            rt_policy: RealTimePolicy::RoundRobin {
                base_slice_factor: None,
            },
        })
        .spawn();
}
