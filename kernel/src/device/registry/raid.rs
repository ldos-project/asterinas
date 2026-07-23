// SPDX-License-Identifier: MPL-2.0

//! RAID-1 logical block device assembly.
//!
//! This module assembles the RAID-1 device from its virtio member disks and
//! registers it with the block layer. The device then gets a devtmpfs node
//! (`/dev/raid`) like any other block device, so user space can mount a
//! filesystem on it (e.g., `mount -t ext2 /dev/raid /raid1`).

#[cfg(not(baseline_asterinas))]
use core::sync::atomic::{AtomicU32, Ordering};

#[cfg(not(baseline_asterinas))]
use aster_raid::selection_policies;
#[cfg(not(baseline_asterinas))]
use aster_raid::server_traits::SelectionPolicy;
use aster_raid::{Raid1Device, Raid1DeviceError};
#[cfg(not(baseline_asterinas))]
use aster_virtio::device::block::device::BlockDevice as VirtIoBlockDevice;
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

/// The kernel command-line parameter naming the read-selection policy.
///
/// Set via `raid.selection=<name>` where `<name>` is one of `roundrobin`
/// (default), `linnos`, `linnos_plus`, or `decision_tree`. Choosing the policy
/// at runtime (rather than a build-time `cfg`) keeps every policy compiled in a
/// single build, so none can silently bitrot.
#[cfg(not(baseline_asterinas))]
const RAID_SELECTION_PARAM: &str = "raid.selection";

/// The read-selection policy used when `raid.selection` is not set.
#[cfg(not(baseline_asterinas))]
const DEFAULT_RAID_SELECTION: &str = "roundrobin";

/// The kernel command-line parameter selecting the admission policy.
///
/// Set via `raid.admission=heimdall` to enable the Heimdall performance monitor.
/// When unset, no admission policy is used and every member is always offered to
/// the selection policy. Choosing this at runtime keeps the Heimdall code
/// compiled in every build rather than hidden behind a `cfg`.
#[cfg(not(baseline_asterinas))]
const RAID_ADMISSION_PARAM: &str = "raid.admission";

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
        // The read-selection policy is chosen at runtime via the `raid.selection`
        // kernel parameter (default: round-robin). Every policy is compiled
        // unconditionally, so a change to one cannot silently bitrot another and
        // CI exercises all of them in a single build.
        let selection = RAID_SELECTION
            .get()
            .map(String::as_str)
            .unwrap_or(DEFAULT_RAID_SELECTION);
        let selection_policy: Arc<dyn SelectionPolicy> = match selection {
            "roundrobin" => selection_policies::RoundRobinPolicy::new(members.clone())?,
            "linnos" => selection_policies::LinnOSPolicy::new(
                members.clone(),
                attach_weak_observers(&members),
            )?,
            "linnos_plus" => selection_policies::LinnOSPlusPolicy::new(
                members.clone(),
                attach_weak_observers(&members),
            )?,
            "decision_tree" => selection_policies::DecisionTreePolicy::new(
                members.clone(),
                attach_weak_observers(&members),
            )?,
            other => {
                warn!("[raid] unknown raid.selection '{other}'; falling back to round-robin");
                selection_policies::RoundRobinPolicy::new(members.clone())?
            }
        };

        // Build the optional admission layer (`raid.admission` kernel parameter).
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

/// Builds the optional admission layer selected at runtime via the
/// `raid.admission` kernel parameter. Returns `None` (admission disabled) unless
/// `raid.admission=heimdall`, in which case the Heimdall monitor thread is also
/// spawned.
#[cfg(not(baseline_asterinas))]
fn build_admission(
    members: &[Arc<dyn aster_block::BlockDevice>],
) -> Option<Arc<aster_raid::heimdall::Heimdall>> {
    match RAID_ADMISSION.get().map(String::as_str) {
        // Default: no admission policy.
        None => None,
        Some("heimdall") => Some(setup_heimdall(members)),
        Some(other) => {
            warn!("[raid] unknown raid.admission '{other}'; admission disabled");
            None
        }
    }
}

/// Attaches one weak observer per member to its bio-completion OQueue, each
/// wrapped in a `Mutex` as the observer-based selection policies expect.
///
/// # Panics
///
/// Panics if a member is not a `VirtIoBlockDevice`.
#[cfg(not(baseline_asterinas))]
fn attach_weak_observers(
    members: &[Arc<dyn aster_block::BlockDevice>],
) -> Vec<
    ostd::sync::Mutex<
        ostd::orpc::oqueue::WeakObserver<aster_block::bio::BlockDeviceCompletionStats>,
    >,
> {
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
}

#[cfg(not(baseline_asterinas))]
static HEIMDALL_BATCH_SIZE: AtomicU32 =
    AtomicU32::new(aster_raid::heimdall::DEFAULT_BATCH_SIZE as u32);
#[cfg(not(baseline_asterinas))]
aster_cmdline::define_kv_param!("heimdall.batch_size", HEIMDALL_BATCH_SIZE);

#[cfg(not(baseline_asterinas))]
static HEIMDALL_INFERENCE_TIMEOUT_MS: AtomicU32 =
    AtomicU32::new(aster_raid::heimdall::DEFAULT_INFERENCE_TIMEOUT_MS as u32);
#[cfg(not(baseline_asterinas))]
aster_cmdline::define_kv_param!(
    "heimdall.inference_timeout_ms",
    HEIMDALL_INFERENCE_TIMEOUT_MS
);

/// Creates the Heimdall performance monitor over `members` and spawns its
/// background inference thread. The returned handle is shared with the RAID
/// device so the admission decisions stay in sync with the monitor.
#[cfg(not(baseline_asterinas))]
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

/// The read-selection policy name, populated during early boot from the
/// `raid.selection` kernel command-line argument (see [`RAID_SELECTION_PARAM`]).
#[cfg(not(baseline_asterinas))]
static RAID_SELECTION: Once<String> = Once::new();
#[cfg(not(baseline_asterinas))]
aster_cmdline::define_kv_param!(RAID_SELECTION_PARAM, RAID_SELECTION);

/// The admission policy name, populated during early boot from the
/// `raid.admission` kernel command-line argument (see [`RAID_ADMISSION_PARAM`]).
#[cfg(not(baseline_asterinas))]
static RAID_ADMISSION: Once<String> = Once::new();
#[cfg(not(baseline_asterinas))]
aster_cmdline::define_kv_param!(RAID_ADMISSION_PARAM, RAID_ADMISSION);
