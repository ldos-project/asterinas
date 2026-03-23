// SPDX-License-Identifier: MPL-2.0

#![cfg(ktest)]

use alloc::{boxed::Box, vec};

use ostd::{mm::UntypedMem, orpc::path::Path, path, sync::Mutex};

use crate::{
    BlockDevice, BlockDeviceMeta, SECTOR_SIZE,
    bio::{BioEnqueueError, BioStatus, BioType, SubmittedBio},
};

/// A block device and immediately completes every submitted request.
#[derive(Debug)]
pub struct FakeBlockDevice;

impl BlockDevice for FakeBlockDevice {
    fn enqueue(&self, bio: SubmittedBio) -> core::result::Result<(), BioEnqueueError> {
        bio.complete(BioStatus::Complete);
        Ok(())
    }

    fn metadata(&self) -> crate::BlockDeviceMeta {
        todo!()
    }

    fn path(&self) -> Path {
        Path::test()
    }
}

/// A block device backed by memory.
///
/// This was lifted from the kernel/comps/mlsdisk/src/lib.rs.
#[derive(Debug)]
pub struct MemoryDisk {
    pub data: Mutex<Box<[u8]>>,
}

impl MemoryDisk {
    pub fn new(size: usize) -> Self {
        Self {
            data: Mutex::new(vec![0; size].into()),
        }
    }
}

impl BlockDevice for MemoryDisk {
    fn enqueue(&self, bio: SubmittedBio) -> core::result::Result<(), BioEnqueueError> {
        let bio_type = bio.type_();
        if bio_type == BioType::Flush || bio_type == BioType::Discard {
            bio.complete(BioStatus::Complete);
            return Ok(());
        }

        let mut data = self.data.lock();
        let mut current_offset = bio.sid_range().start.to_offset();
        for segment in bio.segments() {
            let size = match bio_type {
                BioType::Read => {
                    let data = &data.as_ref()[current_offset..current_offset + segment.nbytes()];
                    segment.inner_segment().writer().write(&mut data.into())
                }
                BioType::Write => {
                    let data =
                        &mut data.as_mut()[current_offset..current_offset + segment.nbytes()];
                    segment.inner_segment().reader().read(&mut data.into())
                }
                _ => 0,
            };
            current_offset += size;
        }
        bio.complete(BioStatus::Complete);
        Ok(())
    }

    fn metadata(&self) -> BlockDeviceMeta {
        BlockDeviceMeta {
            max_nr_segments_per_bio: usize::MAX,
            nr_sectors: self.data.lock().len() / SECTOR_SIZE,
        }
    }

    fn path(&self) -> Path {
        path!(memory_disk)
    }
}
