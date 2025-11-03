// SPDX-License-Identifier: MPL-2.0

#![no_std] // BlockDevice crate also not using rust std and not using unsafe code.
#![deny(unsafe_code)]

extern crate alloc;

use alloc::{string::String, sync::Arc, vec::Vec};
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
    members: Vec<Arc<dyn BlockDevice>>,
    queue: BioRequestSingleQueue,
    metadata: BlockDeviceMeta, // BlockDeviceMeta keeps the basic capacity limits for a device.
    read_cursor: AtomicUsize, // lock free integer type for round robin selection of members to read from. Atomically implemented by CPU instructions. No need for a mutex and would be safe for multiple threads to use.
}

/// Need to wrap BlockDevice with Arc because a single block device might be used by multiple subsystems in the OS. Thus, this
/// provides a thread safe shared ownership without copying the device object. Arc let everyone holds a clone and drops it when
/// the last reference is gone. Also, we need to add dyn before it .
impl Raid1Device {
    /// Creates a new RAID-1 device backed by `members`.
    ///
    /// # Panics
    ///
    /// Panics if fewer than two members are provided.
    pub fn new(members: Vec<Arc<dyn BlockDevice>>) -> Arc<Self> {
        assert!(
            members.len() >= 2,
            "Raid1Device requires at least two members"
        );

        let metadata = Self::min_metadata(&members); // find the min segments and min bandwidth among all the block devices in this RAID array. 
        let queue = BioRequestSingleQueue::with_max_nr_segments_per_bio(
            // initialize a queue to submit the IO request to the RAID array.
            metadata.max_nr_segments_per_bio,
        );

        Arc::new(Self {
            members,
            queue,
            metadata,
            read_cursor: AtomicUsize::new(0),
        })
    }

    /// Registers a RAID-1 device into the global block device table.
    /// i.e. make it a block device that can be used by the filesystem.
    pub fn register(name: &str, members: Vec<Arc<dyn BlockDevice>>) -> Arc<Self> {
        let device = Self::new(members);
        // call String::from() to copy the string to the registerar
        // call clone() to share the ownership with the filesystem, increasing a reference count.
        aster_block::register_device(String::from(name), device.clone());
        device
    }

    /// Dequeues and processes the next request from the staging queue.
    pub fn handle_requests(&self) {
        let request = self.queue.dequeue();
        self.process_request(request);
    }

    /// Check the Bio request type and call the corresponding function (read/write/flush)
    /// This implies the request for RAID1 device is the same as the request submitted
    /// to a standard block device in Asterinas.  to process the request.
    /// See function handle_requests() in kernel/comps/virtio/src/device/block/device.rs
    fn process_request(&self, request: BioRequest) {
        match request.type_() {
            BioType::Read => self.process_read(request),
            BioType::Write => self.process_write(request),
            BioType::Flush => self.process_flush(request),
            // BioType::Discard => self.complete_all(request, BioStatus::NotSupported),
            _ => todo!("Discard (or {:?}) operation not supported", request.type_()),
        }
    }

    /// Process the read request for the RAID-1 devices by selecting block devices to read in a round robin manner.
    /// Parent is the fs block io request layer, and the child is the block device mirror layer for the bio.
    fn process_read(&self, request: BioRequest) {
        // parent request are submitted by the fs layer. And the child requrest are for each block device mirror.
        for parent in request.bios() {
            // iterate through all the fs submitted block io requests.
            let member = self.select_read_member(parent.sid_range()); // for each request, select which block device to read from in a round robin manner. 
            let child = Bio::new(
                // create a child request for the selected block device.
                BioType::Read, // request type is read from the selected block device.
                parent.sid_range().start, // the starting sector id of the request. The sector id is indexing the block device.
                Self::clone_segments(parent), // which segments to put the data after reading from the selected block device, segments indexing the (main) memory.
                None,
            );
            // submit the child request to the selected block device, then wait for the completion synchronously. This is a blocking call, and could be converted to asynchronous later with submit().
            let status = match child.submit_and_wait(member.as_ref()) {
                // as_ref() borrows the inner trait object as &dyn BlockDevice.
                Ok(status) => status,
                Err(_) => BioStatus::IoError,
            };
            // report the completion status of the child request to the fs layer.
            parent.complete(status);
        }
    }

    /// Process the write request for the RAID-1 devices by fanning out the write request to all the block device mirrors.
    fn process_write(&self, request: BioRequest) {
        for parent in request.bios() {
            // iterate through all the fs submitted block io write requests.
            // take the request and fan out to all the block device mirrors.
            let status = self.fanout_to_members(parent, BioType::Write, || {
                // pass in a function that create a clone for the segments, so all the block device could use it to make its own copy of the segments.
                Self::clone_segments(parent)
            });
            parent.complete(status); // report the completion status of the child request to the fs layer. 
        }
    }

    fn process_flush(&self, request: BioRequest) {
        for parent in request.bios() {
            let status = self.fanout_to_members(parent, BioType::Flush, Vec::new);
            parent.complete(status);
        }
    }

    fn fanout_to_members<F>(
        &self,
        parent: &SubmittedBio,
        bio_type: BioType,
        mut segments_builder: F,
    ) -> BioStatus
    where
        F: FnMut() -> Vec<BioSegment>, // F must implement FnMut() -> Vec<BioSegment>, a mutable function that returns a vector of BioSegment.
    {
        // BioWaiter structure holds a list of `Bio` requests and provides functionality to
        // wait for their completion and retrieve their statuses.
        let mut waiter = BioWaiter::new(); // create a new BioWaiter to hold the child requests.
        let mut submission_failed = false; // flag to track if any of the child requests failed to submit.

        for member in &self.members {
            let child = Bio::new(bio_type, parent.sid_range().start, segments_builder(), None); // a clone of the parent segments is passed in each time we call segment_builder()
            match child.submit(member.as_ref()) {
                // submit asynchronously to each selected block device mirror, and wait for the results.
                Ok(child_waiter) => waiter.concat(child_waiter), // add the bio requets statu of a child to the waiter.
                Err(_) => submission_failed = true, // if any of the child requests fails to submit, set the flag to true.
            }
        }

        let mut aggregated_status = if submission_failed {
            // set error if any of the child requests fails to submit.
            Some(BioStatus::IoError)
        } else {
            None
        };

        if waiter.nreqs() > 0 {
            // if the request is submitted, wait for completion of all the child requests.
            let wait_result = waiter.wait(); // wait all of them to complete, if all the requests are completed successfully, it guarantees to have a BioStatus::Complete flag. 
            if wait_result.is_none() {
                // for idx in 0..waiter.nreqs() {
                //     let status = waiter.status(idx);
                //     if status != BioStatus::Complete {
                //         Self::update_status(&mut aggregated_status, status);
                //         if aggregated_status == Some(BioStatus::IoError) {
                //             break;
                //         }
                //     }
                // }

                // report some io error status to the fs layer.
                aggregated_status = Some(BioStatus::IoError);
            }
        }

        aggregated_status.unwrap_or(BioStatus::Complete) // if the aggragated_status is none, then the request is complete, we return a complete status. If not then unwrape the error and return. 
    }

    /// Design choice: complete all the child request (report to fs layer) with a bio status of error set if the bio type is not implemented.
    // fn complete_all(&self, request: BioRequest, status: BioStatus) {
    //     for bio in request.bios() {
    //         bio.complete(status);
    //     }
    // }

    /// Increment the read cursor index for where we read, and return the index to read from.
    /// Use modulo operation to determine which block device to read from.
    fn select_read_member(&self, _sid_range: &Range<Sid>) -> Arc<dyn BlockDevice> {
        let idx = self.read_cursor.fetch_add(1, Ordering::Relaxed);
        self.members[idx % self.members.len()].clone()
    }

    /// Find the minimum metadata for all members.
    /// We need this because when we write a piece of data to all the devices, we can only submit
    /// the block io request with the sizes of the smallest device, i.e., the device with the smallest
    /// number of segment and the smallest bandwidth (segments per bio).  
    fn min_metadata(members: &[Arc<dyn BlockDevice>]) -> BlockDeviceMeta {
        let mut iter = members.iter();
        let first = iter
            .next()
            .expect("Raid1Device requires at least one member");
        let mut meta = first.metadata();

        for member in iter {
            let member_meta = member.metadata();
            meta.nr_sectors = cmp::min(meta.nr_sectors, member_meta.nr_sectors); // nr_sectors: total number of addressable sectors
            meta.max_nr_segments_per_bio = cmp::min(
                // max_nr_segments_per_bio: maximum number of segments per bio
                meta.max_nr_segments_per_bio,
                member_meta.max_nr_segments_per_bio,
            );
        }

        meta
    }

    /// Clone the segments from the parent request to the child request.
    fn clone_segments(parent: &SubmittedBio) -> Vec<BioSegment> {
        parent.segments().iter().cloned().collect()
    }

    /// helper function to report the worst io submission error in the group of write requests submitted to
    /// the block devices in the RAID array.
    fn update_status(current: &mut Option<BioStatus>, candidate: BioStatus) {
        if candidate == BioStatus::Complete {
            return;
        }

        match current {
            Some(existing) => {
                if Self::status_priority(candidate) > Self::status_priority(*existing) {
                    *current = Some(candidate);
                }
            }
            None => *current = Some(candidate),
        }
    }

    /// helper function to assign a priority to the bio status, so we can report the worst io submission error in the group of write requests submitted to
    fn status_priority(status: BioStatus) -> u8 {
        match status {
            BioStatus::IoError => 3,
            BioStatus::NoSpace => 2,
            BioStatus::NotSupported => 1,
            _ => 0,
        }
    }

    /// Check if the sector id range is within the capacity of the RAID array.
    fn range_within_capacity(&self, range: &Range<Sid>) -> bool {
        range.end.to_raw() <= self.metadata.nr_sectors as u64
    }
}

impl BlockDevice for Raid1Device {
    /// Enqueue the bio request to the RAID array.
    fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
        // if the data to write is beyond the capacity of the RAID array, return an error.
        if !self.range_within_capacity(bio.sid_range()) {
            return Err(BioEnqueueError::Refused);
        }
        // Else enqueue the bio request.
        self.queue.enqueue(bio)
    }

    /// Return the metadata of the RAID array.
    fn metadata(&self) -> BlockDeviceMeta {
        self.metadata
    }
}
