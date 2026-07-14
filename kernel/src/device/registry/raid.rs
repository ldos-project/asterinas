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
use device_id::DeviceId;
use spin::Once;

use crate::{
    prelude::*,
    sched::{RealTimePolicy, SchedPolicy},
    thread::kernel_thread::ThreadOptions,
};

const RAID_DEVICE_NAME: &str = "raid";

/// The kernel command-line parameter naming the RAID-1 member disks.
///
/// The member disks are configured via `raid.members=<name>,<name>,...`, a
/// comma-separated list of block device names (e.g. `raid.members=vdc,vdd,vde`).
/// The list is ordered: the position of each name is used as the member's
/// logical index. The value is collected during early boot into [`RAID_MEMBERS`]
/// by the [`aster_cmdline`] registration below.
const RAID_MEMBERS_PARAM: &str = "raid.members";

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
    let members = collect_members()?; // Collect member devices

    #[cfg(not(baseline_asterinas))]
    let init_result = {
        let selection_policy = RoundRobinPolicy::new(members.clone())?;
        Raid1Device::init(RAID_DEVICE_NAME, members, selection_policy)
    };
    #[cfg(baseline_asterinas)]
    let init_result = Raid1Device::init(RAID_DEVICE_NAME, members);

    // `Raid1DeviceError` is a foreign (component-crate) error, and the kernel's
    // `Error` is not being extended to carry it as a source, so bridge it here
    // by picking the errno the caller should see. The descriptive text stays in
    // sync with the variant's `Display`.
    let raid_id = init_result.map_err(|err| match err {
        Raid1DeviceError::NotEnoughMembers => {
            Error::with_message(Errno::EINVAL, "RAID-1 device requires at least two members")
        }
        Raid1DeviceError::Block { .. } => {
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
/// The member disk names are read from the `raid.members` parameter (see
/// [`RAID_MEMBERS_PARAM`]) rather than hardcoded, so the disk layout can be
/// reconfigured from `OSDK.toml` / `tools/qemu_args.sh` without touching the
/// kernel. Devices are looked up by name via [`aster_block::collect_all`]
/// (the refactored registry keys devices by `DeviceId`, and virtio disks are
/// auto-named `vda`, `vdb`, ... in probe order).
fn collect_members() -> Result<Vec<Arc<dyn aster_block::BlockDevice>>> {
    let Some(members_arg) = RAID_MEMBERS.get() else {
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
    for name in &member_names {
        let Some(device) = all_devices.iter().find(|device| device.name() == *name) else {
            return_errno_with_message!(Errno::ENOENT, "a RAID-1 member disk is missing");
        };
        members.push(device.clone());
    }

    Ok(members)
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

/// The comma-separated RAID-1 member disk names, populated during early boot
/// from the `raid.members` kernel command-line argument (see
/// [`RAID_MEMBERS_PARAM`]).
static RAID_MEMBERS: Once<String> = Once::new();
aster_cmdline::define_kv_param!(RAID_MEMBERS_PARAM, RAID_MEMBERS);
