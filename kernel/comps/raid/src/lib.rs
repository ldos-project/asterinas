// SPDX-License-Identifier: MPL-2.0

//! RAID-1 virtual block device
//!
//! This module implements a simple RAID-1 adapter that exposes a single logical
//! `BlockDevice` backed by multiple member devices (mirrors). Requests are
//! admitted through a software queue, then translated into per-member BIOs:
//!
//! - Reads: each BIO is dispatched to one selected member (round-robin) and
//!   submitted asynchronously so different BIOs can overlap across devices.
//! - Writes: the same data is fanned out to all members; completion is reported
//!   only after all replicas complete.
//!
//! Capacity and limits are the minimum across members (e.g., `nr_sectors`,
//! `max_nr_segments_per_bio`). Error handling is conservative: any failed child
//! write makes the parent fail.

#![no_std] // BlockDevice crate also not using rust std and not using unsafe code.
#![deny(unsafe_code)]

extern crate alloc;

#[cfg(not(baseline_asterinas))]
pub mod decision_tree_predictions;
#[cfg(not(baseline_asterinas))]
pub mod heimdall;
#[cfg(not(baseline_asterinas))]
pub mod heimdall_weights;
#[cfg(not(baseline_asterinas))]
pub mod linnos_plus_weights;
#[cfg(not(baseline_asterinas))]
pub mod linnos_weights;
#[cfg(not(baseline_asterinas))]
pub mod selection_policies;
#[cfg(not(baseline_asterinas))]
pub mod server_traits;

use alloc::{borrow::ToOwned, string::String, sync::Arc, vec::Vec};
#[cfg(baseline_asterinas)]
use core::sync::atomic::AtomicUsize;
use core::{
    cmp,
    ops::Range,
    sync::atomic::{AtomicU32, Ordering},
};

use aster_block::{
    BlockDevice, BlockDeviceMeta, DeviceId, MajorIdOwner,
    bio::{
        Bio, BioEnqueueError, BioSegment, BioStatus, BioType, BioWaiter, ParentGuard, SubmittedBio,
    },
    id::Sid,
    request_queue::{BioRequest, BioRequestSingleQueue},
};
use device_id::MinorId;
use ostd::orpc::orpc_server;
use snafu::{ResultExt as _, Snafu, ensure};
use spin::Once;

#[cfg(not(baseline_asterinas))]
use crate::server_traits::{BioCandidates, SelectionPolicy};

/// A RAID-1 block device that mirrors I/O to multiple member devices.
#[derive(Debug)]
#[orpc_server]
pub struct Raid1Device {
    /// Member block devices that store identical data (mirrors).
    members: Vec<Arc<dyn BlockDevice>>,
    queue: BioRequestSingleQueue,

    /// Basic capacity limits for the logical device (min across members).
    metadata: BlockDeviceMeta,

    /// A cursor for selecting the next member for read operations (round-robin).
    #[cfg(baseline_asterinas)]
    read_cursor: AtomicUsize,

    /// The policy to select the read member.
    #[cfg(not(baseline_asterinas))]
    selection_policy: Arc<dyn SelectionPolicy>,

    /// The optional admission policy that pre-filters which members a read may
    /// target. When present, only the admitted members are offered to the
    /// selection policy.
    #[cfg(not(baseline_asterinas))]
    admission: Option<Arc<crate::heimdall::Heimdall>>,

    /// Cached list of all member indices (`0..members.len()`), used as the
    /// candidate set when no admission policy is active. Avoids a per-read
    /// allocation on the common path.
    #[cfg(not(baseline_asterinas))]
    all_indices: Vec<usize>,

    name: String,
    id: DeviceId,
}

#[derive(Debug, Snafu)]
pub enum Raid1DeviceError {
    #[snafu(display("RAID-1 device requires at least two members"))]
    NotEnoughMembers,
    #[snafu(display("failed to register the RAID-1 device"))]
    Block { source: aster_block::Error },
}

/// The major ID of the RAID device class.
static RAID_MAJOR_ID: Once<MajorIdOwner> = Once::new();

/// A counter that hands out a unique minor ID to each instantiated RAID device.
static NR_RAID_DEVICE: AtomicU32 = AtomicU32::new(0);

/// Allocates a device ID for a new RAID device.
/// The RAID class major ID is allocated lazily on first use;
/// the minor ID is a simple counter.
fn allocate_device_id() -> Result<DeviceId, Raid1DeviceError> {
    if RAID_MAJOR_ID.get().is_none() {
        let major = aster_block::allocate_major().context(BlockSnafu)?;
        RAID_MAJOR_ID.call_once(|| major);
    }
    let major = RAID_MAJOR_ID.get().unwrap().get();
    let minor = NR_RAID_DEVICE.fetch_add(1, Ordering::Relaxed);
    log::info!(
        "[raid] allocating device ID for RAID-1 device: major={}, minor={}",
        major.get(),
        minor
    );
    Ok(DeviceId::new(major, MinorId::new(minor)))
}

impl Raid1Device {
    /// Creates a new RAID-1 device backed by `members`.
    ///
    /// # Panics
    ///
    /// Panics if fewer than two members are provided.
    pub fn init(
        name: &str,
        members: Vec<Arc<dyn BlockDevice>>,
        #[cfg(not(baseline_asterinas))] selection_policy: Arc<dyn SelectionPolicy>,
        #[cfg(not(baseline_asterinas))] admission: Option<Arc<crate::heimdall::Heimdall>>,
    ) -> Result<DeviceId, Raid1DeviceError> {
        ensure!(members.len() >= 2, NotEnoughMembersSnafu);

        let id = allocate_device_id()?;

        // Compute the minimal metadata across all members.
        let metadata = Self::min_metadata(&members);
        // Initialize the admission queue using the strictest segment limit.
        let queue =
            BioRequestSingleQueue::with_max_nr_segments_per_bio(metadata.max_nr_segments_per_bio);

        #[cfg(not(baseline_asterinas))]
        let all_indices = (0..members.len()).collect();

        #[cfg(not(baseline_asterinas))]
        let device = Self::new_with(|orpc_internal, _weak_self| Raid1Device {
            orpc_internal,
            members,
            queue,
            metadata,
            selection_policy,
            admission,
            all_indices,
            name: name.to_owned(),
            id,
        });
        #[cfg(baseline_asterinas)]
        let device = Arc::new_cyclic(|_weak_self| Raid1Device {
            members,
            queue,
            metadata,
            read_cursor: AtomicUsize::new(0),
            name: name.to_owned(),
            id,
        });

        aster_block::register(device.clone()).context(BlockSnafu)?;

        Ok(id)
    }

    /// Dequeues and processes the next request from the staging queue.
    pub fn handle_requests(&self) {
        let request = self.queue.dequeue();
        self.process_request(request);
    }

    /// Dispatches a request by type. The RAID-1 device accepts the same BIOs as
    /// any `BlockDevice` and applies RAID semantics underneath.
    fn process_request(&self, request: BioRequest) {
        match request.type_() {
            BioType::Read => self.process_read_async(request),
            BioType::Write => self.process_write_async(request),
            BioType::Flush => self.process_flush(request),
        }
    }

    /// Processes read requests synchronously.
    /// Asterinas Baseline Version, i.e., not selecting read member, just use device 0.
    ///
    /// Each `SubmittedBio` in the merged `BioRequest` is assigned to device 0
    /// and submitted with `Bio::submit`. Completion of the parent is reported
    /// after the child finishes.
    #[expect(dead_code)]
    #[cfg(baseline_asterinas)]
    fn process_read(&self, request: BioRequest) {
        for parent in request.bios() {
            // Baseline Asterinas should use round robin policy
            let member = self.members
                [self.read_cursor.fetch_add(1, Ordering::Relaxed) % self.members.len()]
            .clone();
            let child = Bio::new(
                BioType::Read,
                parent.sid_range().start,
                Self::clone_segments(parent),
                None,
            );
            match child.submit(&*member) {
                Ok(waiter) => {
                    let status = match waiter.wait() {
                        Some(s) => s,
                        None => BioStatus::IoError,
                    };
                    parent.complete(status);
                }
                Err(_) => parent.complete(BioStatus::IoError),
            }
        }
    }

    /// Chooses the member device to serve a read.
    ///
    /// The admission policy (if any) first pre-filters the members down to those
    /// it admits; the selection policy then chooses among the admitted members.
    /// If the admission policy rejects every member, all members are offered so
    /// that I/O is never stalled.
    #[cfg(not(baseline_asterinas))]
    fn select_member(&self, bio: &mut SubmittedBio) -> Arc<dyn BlockDevice> {
        // Fast path: no admission policy — offer every member without allocating.
        let Some(heimdall) = &self.admission else {
            let selection = BioCandidates {
                bio,
                candidates: &self.all_indices,
            };
            let _guard = ostd::task::disable_preempt();
            return self
                .selection_policy
                .select_block_device(selection)
                .unwrap();
        };

        // Admission active: offer only the members Heimdall currently admits,
        // falling back to all members if it admits none (so I/O is never stalled).
        let admitted: Vec<usize> = self
            .all_indices
            .iter()
            .copied()
            .filter(|&index| heimdall.is_device_fast(index))
            .collect();
        let candidates: &[usize] = if admitted.is_empty() {
            &self.all_indices
        } else {
            &admitted
        };
        let selection = BioCandidates { bio, candidates };
        let _guard = ostd::task::disable_preempt();
        self.selection_policy
            .select_block_device(selection)
            .unwrap()
    }

    #[expect(dead_code)]
    #[cfg(not(baseline_asterinas))]
    fn process_read(&self, request: BioRequest) {
        // Submit all children first to overlap device I/O.
        let mut pending: alloc::vec::Vec<(SubmittedBio, BioWaiter)> = alloc::vec::Vec::new();

        for mut parent in request.into_bios() {
            let member = self.select_member(&mut parent);
            let child = Bio::new(
                // Child BIO mirrors the parent’s type, range, and buffers.
                BioType::Read,
                parent.sid_range().start,
                Self::clone_segments(&parent),
                None,
            );
            match child.submit(&*member) {
                Ok(waiter) => pending.push((parent, waiter)),
                Err(_) => todo!("Failed to submit child BIO, Don't know what to do"),
            }
        }

        // Wait for each submitted child and complete the corresponding parent.
        for (parent, waiter) in pending.into_iter() {
            let status = match waiter.wait() {
                // Guaranteed to be Complete on success when Some is returned.
                Some(s) => s,
                None => BioStatus::IoError,
            };
            // Report the completion status to the upper layer.
            parent.complete(status);
        }
    }

    /// Processes read requests asynchronously.
    ///
    /// Each `SubmittedBio` in the merged `BioRequest` is assigned to a read
    /// member by the selection policy (device 0 if asterinas baseline) and submitted with `Bio::submit` to overlap device
    /// I/O. Completion of the parent is reported after the child finishes.    
    fn process_read_async(&self, request: BioRequest) {
        for mut parent in request.into_bios() {
            #[cfg(not(baseline_asterinas))]
            let member = self.select_member(&mut parent);

            #[cfg(baseline_asterinas)]
            let member = self.members[0].clone();

            let start_sid = parent.sid_range().start;
            let segments = parent.segments().to_vec();
            let guard = ParentGuard::new(parent);
            let child = Bio::new_with_closure(
                BioType::Read,
                start_sid,
                segments,
                move |child_bio: &SubmittedBio| {
                    guard.complete(child_bio.status());
                },
            );
            let _ = child.submit(&*member);
        }
    }

    /// Completes all the parents with the same status.
    #[expect(dead_code)]
    fn complete_all(&self, request: BioRequest, status: BioStatus) {
        for parent in request.bios() {
            parent.complete(status);
        }
    }

    /// Processes write requests by fanning out to all mirrors and aggregating
    /// the results (all must succeed).
    #[expect(dead_code)]
    fn process_write(&self, request: BioRequest) {
        for parent in request.bios() {
            // Submit the same write to all members.
            let status =
                self.fanout_to_members(parent, BioType::Write, || Self::clone_segments(parent));
            parent.complete(status);
        }
    }

    /// Processes write requests asynchronously by fanning out to all mirrors.
    ///
    /// Each child BIO carries a callback that atomically decrements a shared
    /// counter. The last callback to fire (or the dispatch thread on submission
    /// failure) completes the parent. Any failed member marks the write as
    /// `IoError`; all members must succeed for `Complete` to be reported.
    fn process_write_async(&self, request: BioRequest) {
        use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        use ostd::sync::{LocalIrqDisabled, SpinLock};

        for parent in request.into_bios() {
            let n = self.members.len();
            let remaining = Arc::new(AtomicUsize::new(n));
            let had_error = Arc::new(AtomicBool::new(false));

            // Extract before moving parent into the guard.
            let start_sid = parent.sid_range().start;
            let segments = parent.segments().to_vec();
            let guard = Arc::new(SpinLock::<_, LocalIrqDisabled>::new(Some(
                ParentGuard::new(parent),
            )));

            for member in &self.members {
                let remaining_cb = remaining.clone();
                let had_error_cb = had_error.clone();
                let guard_cb = guard.clone();
                let remaining_err = remaining.clone();
                let had_error_err = had_error.clone();
                let guard_err = guard.clone();
                let member = member.clone();

                let child = Bio::new_with_closure(
                    BioType::Write,
                    start_sid,
                    segments.clone(),
                    move |child_bio: &SubmittedBio| {
                        if child_bio.status() != BioStatus::Complete {
                            had_error_cb.store(true, Ordering::Release);
                        }
                        if remaining_cb.fetch_sub(1, Ordering::AcqRel) == 1 {
                            let status = if had_error_cb.load(Ordering::Acquire) {
                                BioStatus::IoError
                            } else {
                                BioStatus::Complete
                            };
                            if let Some(g) = guard_cb.lock().take() {
                                g.complete(status);
                            }
                        }
                    },
                );

                if member.submit(child).is_err() {
                    had_error_err.store(true, Ordering::Release);
                    if remaining_err.fetch_sub(1, Ordering::AcqRel) == 1 {
                        if let Some(g) = guard_err.lock().take() {
                            g.complete(BioStatus::IoError);
                        }
                    }
                }
            }
        }
    }

    /// Propagates a flush to all members and completes after they finish.
    fn process_flush(&self, request: BioRequest) {
        for parent in request.bios() {
            let status = self.fanout_to_members(parent, BioType::Flush, Vec::new);
            parent.complete(status);
        }
    }

    /// Submits the same BIO to all members and aggregates completion.
    ///
    /// Returns `Complete` only if every child completes; otherwise
    /// returns a conservative `IoError`.
    fn fanout_to_members<F>(
        &self,
        parent: &SubmittedBio,
        bio_type: BioType,
        mut segments_builder: F,
    ) -> BioStatus
    where
        F: FnMut() -> Vec<BioSegment>,
    {
        let mut waiter = BioWaiter::new();
        let mut submission_failed = false;

        for member in &self.members {
            // Build a child BIO for this member.
            let child = Bio::new(bio_type, parent.sid_range().start, segments_builder(), None);
            match member.submit(child) {
                Ok(child_waiter) => waiter.concat(child_waiter),
                Err(_) => submission_failed = true,
            }
        }

        let mut aggregated_status = if submission_failed {
            // set error if any of the child requests fails to submit.
            Some(BioStatus::IoError)
        } else {
            None
        };

        if waiter.nreqs() > 0 {
            // Wait for all children; success is Some(Complete), otherwise None.
            let wait_result = waiter.wait();
            if wait_result.is_none() {
                aggregated_status = Some(BioStatus::IoError);
            }
        }

        // Default to success if no errors were observed.
        aggregated_status.unwrap_or(BioStatus::Complete)
    }

    /// Computes minimal metadata across members (capacity and segment limit).
    fn min_metadata(members: &[Arc<dyn BlockDevice>]) -> BlockDeviceMeta {
        let mut iter = members.iter();
        let first = iter
            .next()
            .expect("Raid1Device requires at least one member");
        let mut meta = first.metadata();

        for member in iter {
            let member_meta = member.metadata();
            // Total number of addressable sectors is bounded by the smallest member.
            meta.nr_sectors = cmp::min(meta.nr_sectors, member_meta.nr_sectors);
            // Segment limit per BIO is also bounded by the smallest member.
            meta.max_nr_segments_per_bio = cmp::min(
                meta.max_nr_segments_per_bio,
                member_meta.max_nr_segments_per_bio,
            );
        }

        meta
    }

    /// Clones segments from a parent BIO for use by a child BIO.
    fn clone_segments(parent: &SubmittedBio) -> Vec<BioSegment> {
        parent.segments().to_vec()
    }

    /// Updates an aggregated status with a candidate error using priority.
    #[expect(dead_code)]
    fn update_status(current: &mut Option<BioStatus>, candidate: BioStatus) {
        if candidate == BioStatus::Complete {
            return;
        }

        match current {
            Some(existing) => {
                if candidate.priority() > (*existing).priority() {
                    *current = Some(candidate);
                }
            }
            None => *current = Some(candidate),
        }
    }

    /// Checks whether a sector range fits within the logical device capacity.
    fn range_within_capacity(&self, range: &Range<Sid>) -> bool {
        range.end.to_raw() <= self.metadata.nr_sectors as u64
    }
}

impl BlockDevice for Raid1Device {
    /// Enqueues a BIO to the RAID device’s admission queue.
    fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
        // Reject BIOs that exceed the logical capacity.
        if !self.range_within_capacity(bio.sid_range()) {
            return Err(BioEnqueueError::Refused);
        }
        // Otherwise, enqueue for processing.
        self.queue.enqueue(bio)
    }

    /// Returns the logical device metadata.
    fn metadata(&self) -> BlockDeviceMeta {
        self.metadata
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> DeviceId {
        self.id
    }
}
