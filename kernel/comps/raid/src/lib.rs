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

use alloc::{borrow::ToOwned, sync::Arc, vec::Vec};
use core::{
    cmp,
    ops::Range,
    sync::atomic::{AtomicUsize, Ordering},
};

use aster_block::{
    BlockDevice, BlockDeviceMeta,
    bio::{Bio, BioEnqueueError, BioSegment, BioStatus, BioType, BioWaiter, SubmittedBio},
    id::Sid,
    request_queue::{BioRequest, BioRequestSingleQueue},
};


/// A RAID-1 block device that mirrors I/O to multiple member devices.
#[derive(Debug)]
pub struct Raid1Device {
    /// Member block devices that store identical data (mirrors).
    members: Vec<Arc<dyn BlockDevice>>,
    queue: BioRequestSingleQueue,
    /// Basic capacity limits for the logical device (min across members).
    metadata: BlockDeviceMeta,
    /// Round-robin cursor for selecting a read member without locking.
    read_cursor: AtomicUsize,
}

#[derive(Debug)]
pub enum Raid1DeviceError {
    NotEnoughMembers,
}

impl Raid1Device {
    /// Creates a new RAID-1 device backed by `members`.
    ///
    /// # Panics
    ///
    /// Panics if fewer than two members are provided.
    pub fn new(members: Vec<Arc<dyn BlockDevice>>) -> Result<Arc<Self>, Raid1DeviceError> {
        if members.len() < 2 {
            return Err(Raid1DeviceError::NotEnoughMembers);
        }

        // Compute the minimal metadata across all members.
        let metadata = Self::min_metadata(&members);
        // Initialize the admission queue using the strictest segment limit.
        let queue = BioRequestSingleQueue::with_max_nr_segments_per_bio(
            metadata.max_nr_segments_per_bio,
        );

        Ok(Arc::new(Self {
            members,
            queue,
            metadata,
            read_cursor: AtomicUsize::new(0),
        }))
    }

    /// Registers a RAID-1 device into the global block device table so it can
    /// be opened by upper layers (e.g., filesystems).
    pub fn register(name: &str, members: Vec<Arc<dyn BlockDevice>>) -> Result<Arc<Self>, Raid1DeviceError> {
        let device = Self::new(members)?;
        // Register under a stable name and return a shared handle.
        aster_block::register_device(name.to_owned(), device.clone());
        Ok(device)
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
            BioType::Read => self.process_read(request),
            BioType::Write => self.process_write(request),
            BioType::Flush => self.process_flush(request),
            BioType::Discard => self.process_discard(request),
            _ => self.complete_all(request, BioStatus::NotSupported),
        }
    }

    /// Processes discard requests by submitting them to all members and completing them after they finish.
    fn process_discard(&self, request: BioRequest) {
        for parent in request.bios() {
            // Submit the same discard to all members.
            let status = self.fanout_to_members(parent, BioType::Discard, || {
                Self::clone_segments(parent)
            });
            parent.complete(status);
        }
    }

    /// Processes read requests asynchronously.
    ///
    /// Each `SubmittedBio` in the merged `BioRequest` is assigned to a read
    /// member (round-robin) and submitted with `Bio::submit` to overlap device
    /// I/O. Completion of the parent is reported after the child finishes.
    fn process_read(&self, request: BioRequest) {
        // Submit all children first to overlap device I/O.
        let mut pending: alloc::vec::Vec<(&SubmittedBio, BioWaiter)> = alloc::vec::Vec::new();

        for parent in request.bios() {
            // Select a member to serve this read (round-robin).
            let member = self.select_read_member(parent.sid_range());
            let child = Bio::new(
                // Child BIO mirrors the parent’s type, range, and buffers.
                BioType::Read,
                parent.sid_range().start,
                Self::clone_segments(parent),
                None,
            );
            match child.submit(member.as_ref()) {
                Ok(waiter) => pending.push((parent, waiter)),
                // Err(_) => parent.complete(BioStatus::IoError),
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

    /// Completes all the parents with the same status.
    fn complete_all(&self, request: BioRequest, status: BioStatus) {
        for parent in request.bios() {
            parent.complete(status);
        }   
    }

    /// Processes write requests by fanning out to all mirrors and aggregating
    /// the results (all must succeed).
    fn process_write(&self, request: BioRequest) {
        for parent in request.bios() {
            // Submit the same write to all members.
            let status = self.fanout_to_members(parent, BioType::Write, || {
                Self::clone_segments(parent)
            });
            parent.complete(status);
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
            match child.submit(member.as_ref()) {
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


    /// Selects a read member using a round-robin cursor (lock-free).
    fn select_read_member(&self, _sid_range: &Range<Sid>) -> Arc<dyn BlockDevice> {
        let idx = self.read_cursor.fetch_add(1, Ordering::Relaxed);
        self.members[idx % self.members.len()].clone()
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
        parent.segments().iter().cloned().collect()
    }

    /// Updates an aggregated status with a candidate error using priority.
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
}
