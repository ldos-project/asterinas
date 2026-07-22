// SPDX-License-Identifier: MPL-2.0

//! RAID-1 logical block device assembly.
//!
//! This module assembles the RAID-1 device from its virtio member disks and
//! registers it with the block layer. The device then gets a devtmpfs node
//! (`/dev/raid`) like any other block device, so user space can mount a
//! filesystem on it (e.g., `mount -t ext2 /dev/raid /raid1`).

#[cfg(not(baseline_asterinas))]
use aster_raid::selection_policies;
use aster_raid::{Raid1Device, Raid1DeviceError};
#[cfg(all(
    not(baseline_asterinas),
    any(
        raid_selection = "linnos",
        raid_selection = "linnos_plus",
        raid_selection = "decision_tree",
        raid_admission = "heimdall"
    )
))]
use aster_virtio::device::block::device::BlockDevice as VirtIoBlockDevice;
#[cfg(all(not(baseline_asterinas), raid_admission = "heimdall"))]
use core::sync::atomic::{AtomicU32, Ordering};

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
        // Observer-based submission policies need one weak observer per member.
        #[cfg(any(
            raid_selection = "linnos",
            raid_selection = "linnos_plus",
            raid_selection = "decision_tree"
        ))]
        let observers = {
            use aster_virtio::device::block::server_traits::BlockIOObservable as _;
            use ostd::orpc::oqueue::{OQueueBase as _, ObservationQuery};
            members
                .iter()
                .map(|member| {
                    let virtio = member
                        .downcast_ref::<VirtIoBlockDevice>()
                        .expect("RAID member must be a VirtIoBlockDevice");
                    ostd::sync::Mutex::new(
                        virtio
                            .bio_completion_oqueue()
                            .attach_weak_observer(4, ObservationQuery::identity())
                            .expect("Failed to attach weak observer to bio_completion_oqueue"),
                    )
                })
                .collect()
        };

        // Select the submission (read-selection) policy chosen at build time via
        // the `raid_selection` cfg; round-robin is the default.
        #[cfg(raid_selection = "linnos")]
        let selection_policy = selection_policies::LinnOSPolicy::new(members.clone(), observers)?;
        #[cfg(raid_selection = "linnos_plus")]
        let selection_policy =
            selection_policies::LinnOSPlusPolicy::new(members.clone(), observers)?;
        #[cfg(raid_selection = "decision_tree")]
        let selection_policy =
            selection_policies::DecisionTreePolicy::new(members.clone(), observers)?;
        #[cfg(not(any(
            raid_selection = "linnos",
            raid_selection = "linnos_plus",
            raid_selection = "decision_tree"
        )))]
        let selection_policy = selection_policies::RoundRobinPolicy::new(members.clone())?;

        // Build the optional Heimdall admission layer (raid_admission cfg).
        let admission = build_admission(&members);

        Raid1Device::init(RAID_DEVICE_NAME, members, selection_policy, admission)
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

/// Builds the optional Heimdall admission layer selected at build time via the
/// `raid_admission` cfg. Returns `None` unless `raid_admission="heimdall"`, in
/// which case the Heimdall monitor thread is also spawned.
#[cfg(not(baseline_asterinas))]
fn build_admission(
    members: &[Arc<dyn aster_block::BlockDevice>],
) -> Option<Arc<aster_raid::heimdall::Heimdall>> {
    #[cfg(raid_admission = "heimdall")]
    {
        Some(setup_heimdall(members))
    }
    #[cfg(not(raid_admission = "heimdall"))]
    {
        let _ = members;
        None
    }
}

#[cfg(all(not(baseline_asterinas), raid_admission = "heimdall"))]
static HEIMDALL_BATCH_SIZE: AtomicU32 =
    AtomicU32::new(aster_raid::heimdall::DEFAULT_BATCH_SIZE as u32);
#[cfg(all(not(baseline_asterinas), raid_admission = "heimdall"))]
aster_cmdline::define_kv_param!("heimdall.batch_size", HEIMDALL_BATCH_SIZE);

#[cfg(all(not(baseline_asterinas), raid_admission = "heimdall"))]
static HEIMDALL_INFERENCE_TIMEOUT_MS: AtomicU32 =
    AtomicU32::new(aster_raid::heimdall::DEFAULT_INFERENCE_TIMEOUT_MS as u32);
#[cfg(all(not(baseline_asterinas), raid_admission = "heimdall"))]
aster_cmdline::define_kv_param!("heimdall.inference_timeout_ms", HEIMDALL_INFERENCE_TIMEOUT_MS);

/// Creates the Heimdall performance monitor over `members` and spawns its
/// background inference thread. The returned handle is shared with the RAID
/// device so the admission decisions stay in sync with the monitor.
#[cfg(all(not(baseline_asterinas), raid_admission = "heimdall"))]
fn setup_heimdall(
    members: &[Arc<dyn aster_block::BlockDevice>],
) -> Arc<aster_raid::heimdall::Heimdall> {
    use aster_virtio::device::block::server_traits::BlockIOObservable as _;
    use ostd::orpc::oqueue::{OQueueBase as _, ObservationQuery};

    let device_observers = members
        .iter()
        .map(|member| {
            let virtio = member
                .downcast_ref::<VirtIoBlockDevice>()
                .expect("RAID member must be a VirtIoBlockDevice");
            let observer = virtio
                .bio_completion_oqueue()
                .attach_strong_observer(ObservationQuery::identity())
                .expect("Failed to attach strong observer for Heimdall");
            (member.clone(), observer)
        })
        .collect();

    // Heimdall's inference cadence is configurable via the kernel command line
    // (`heimdall.batch_size`, `heimdall.inference_timeout_ms`); when unset, the
    // storage statics keep their default initializers (see below).
    let batch_size = HEIMDALL_BATCH_SIZE.load(Ordering::Relaxed) as usize;
    let inference_timeout_ms = HEIMDALL_INFERENCE_TIMEOUT_MS.load(Ordering::Relaxed) as u64;

    // The observers are owned by the monitor thread (not the shared handle), so
    // `new` hands them back for `run` to take ownership of.
    let (heimdall, observers) =
        aster_raid::heimdall::Heimdall::new(device_observers, batch_size, inference_timeout_ms)
            .expect("Failed to create Heimdall monitor");

    let monitor = heimdall.clone();
    ThreadOptions::new(move || {
        info!("[heimdall] Heimdall monitor thread started");
        monitor.run(observers);
    })
    .sched_policy(SchedPolicy::RealTime {
        rt_prio: 50.try_into().unwrap(),
        rt_policy: RealTimePolicy::RoundRobin {
            base_slice_factor: None,
        },
    })
    .spawn();

    info!("[heimdall] Heimdall monitor initialized and thread spawned");
    heimdall
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
