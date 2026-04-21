// SPDX-License-Identifier: MPL-2.0

use alloc::{
    string::{String, ToString as _},
    sync::Arc,
    vec::Vec,
};

use aster_block::{
    BLOCK_SIZE, BlockDevice,
    bio::{BioDirection, BioSegment},
    id::Bid,
};
use minicbor_serde::Serializer;
use ostd::orpc::path::Path;
use serde::Serialize;
use snafu::ensure;

use crate::{DataCaptureError, InsufficientSpaceSnafu};

/// The "magic number" included at the start of files to catch errors and allow finding them in
/// corrupted data.
const MAGIC: &[u8; 17] = b"MARIPOSALDOSDATA\0";

/// A buffer for managing data which will be written bit by bit, but the extracted in larger blocks.
struct DataBuf {
    data: Serializer<Vec<u8>>,
}

impl DataBuf {
    /// Creates a new DataBuf with the specified size.
    pub fn new(size: usize) -> Self {
        DataBuf {
            data: Serializer::new(Vec::with_capacity(size)),
        }
    }

    /// Writes a [`Serialize`] value to the buffer at the current offset.
    pub fn write_value<T>(&mut self, v: &T)
    where
        T: Serialize,
    {
        v.serialize(&mut self.data).unwrap()
    }

    /// Writes raw bytes to the buffer at the current position.
    pub fn write_bytes(&mut self, v: &[u8]) {
        let data = self.data.encoder_mut().writer_mut();
        data.extend(v);
    }

    /// Returns a slice containing all written data in the buffer.
    pub fn written_data(&self) -> &[u8] {
        self.data.encoder().writer()
    }

    /// Removes the leading n bytes by shifting remaining data to start of buffer.
    pub fn delete(&mut self, n: usize) {
        let data = self.data.encoder_mut().writer_mut();
        let remaining = data.len() - n;
        data.drain(0..n);
        debug_assert_eq!(data.len(), remaining);
    }

    /// Returns the number of bytes currently in the buffer.
    pub fn len(&self) -> usize {
        self.written_data().len()
    }
}

/// Handles buffering and flushing data to a block device.
pub(crate) struct ChunkingWriteWrapper {
    data_buf: DataBuf,
    pub(crate) block_device: Arc<dyn aster_block::BlockDevice>,
    pub(crate) current_bid: Bid,
    end_bid: Bid,
}

impl ChunkingWriteWrapper {
    /// Create a new wrapper for chunking writes.
    pub fn new(
        buffer_size: usize,
        block_device: Arc<dyn BlockDevice>,
        start_bid: Bid,
        end_bid: Bid,
    ) -> ChunkingWriteWrapper {
        ChunkingWriteWrapper {
            data_buf: DataBuf::new(buffer_size),
            block_device,
            current_bid: start_bid,
            end_bid,
        }
    }

    /// Writes a [`Serialize`] value to the output.
    ///
    /// See [`DataBuf::write_value`].
    pub fn write_value<T>(&mut self, v: &T)
    where
        T: Serialize,
    {
        self.data_buf.write_value(v);
    }

    /// Flushes the a block to storage if it contains more than one block's worth of data.
    pub fn flush_if_needed(&mut self) -> Result<(), DataCaptureError> {
        if self.data_buf.len() > BLOCK_SIZE {
            let n_written = self.flush()?;
            self.current_bid = self.current_bid + 1;
            self.data_buf.delete(n_written);
        }
        Ok(())
    }

    /// Flush the buffered data to storage regardless of the state. If there is no data in the
    /// buffer, this writes a block of 0xff (the CBOR "break" command). This serves to mark the end
    /// of the data.
    ///
    /// The same page may be written again if this is called again and if it was not full when this
    /// was called the first time.
    pub fn flush(&mut self) -> Result<usize, DataCaptureError> {
        ensure!(self.current_bid < self.end_bid, InsufficientSpaceSnafu);

        let raw_data = self.data_buf.written_data();
        let bio_segment = BioSegment::alloc(1, BioDirection::ToDevice);
        let mut writer = bio_segment.writer().expect("segment direction known");
        let n_written = writer.write(&mut raw_data.into());
        writer.fill(0xffu8);
        let _ = self
            .block_device
            .write_blocks_async(self.current_bid, bio_segment)?;
        Ok(n_written)
    }

    /// Sync all of the data to the block device.
    pub fn sync(&mut self) -> Result<(), DataCaptureError> {
        self.flush()?;
        self.block_device.sync()?;
        Ok(())
    }

    /// Writes a structured header with magic number, type information, and paths.
    pub fn write_header<T>(&mut self, name: &str, paths: &[Path]) -> Result<(), DataCaptureError> {
        // Write magic number
        self.data_buf.write_bytes(MAGIC);

        #[derive(Serialize)]
        struct Header<'a> {
            name: &'a str,
            type_name: &'a str,
            oqueues: Vec<String>,
        }

        self.data_buf.write_value(&Header {
            name,
            type_name: core::any::type_name::<T>(),
            oqueues: paths.iter().map(|p| p.to_string()).collect(),
        });

        Ok(())
    }
}

#[cfg(ktest)]
mod test {
    use alloc::vec;

    use aster_block::test_utils::MemoryDisk;
    use ostd::{path, prelude::*};

    use super::*;

    #[ktest]
    fn test_new() {
        let buf = DataBuf::new(10);
        assert_eq!(buf.len(), 0);
    }

    #[ktest]
    fn test_write_value() {
        #[derive(Debug, PartialEq, Eq, Serialize)]
        struct TestStruct {
            a: u32,
            b: u16,
        }

        let mut buf = DataBuf::new(size_of::<TestStruct>());
        let value = TestStruct {
            a: 0x12345678,
            b: 0x9012,
        };

        buf.write_value(&value);

        // Verify the bytes were written correctly
        let data = buf.written_data();
        assert_eq!(
            data,
            &[
                0xa2, 0x61, 0x61, 0x1a, 0x12, 0x34, 0x56, 0x78, 0x61, 0x62, 0x19, 0x90, 0x12
            ]
        )
    }

    #[ktest]
    fn test_write_bytes() {
        let mut buf = DataBuf::new(5);
        buf.write_bytes(b"hello");

        assert_eq!(buf.written_data(), b"hello");
        assert_eq!(buf.len(), 5);
    }

    #[ktest]
    fn test_delete() {
        let mut buf = DataBuf::new(10);
        buf.write_bytes(b"1234567890");

        // Delete first 3 bytes
        buf.delete(3);
        assert_eq!(buf.written_data(), b"4567890");
        assert_eq!(buf.len(), 7);

        // Delete all remaining bytes
        buf.delete(7);
        assert!(buf.written_data().is_empty());
        assert_eq!(buf.len(), 0);
    }

    #[ktest]
    fn test_write_header() {
        let paths = vec![path!(test)];
        let block_device = Arc::new(MemoryDisk::new(4096 * 8));
        let mut wrapper = ChunkingWriteWrapper {
            data_buf: DataBuf::new(4096 * 2),
            block_device: block_device.clone(),
            current_bid: Bid::new(0),
            end_bid: Bid::new(4),
        };

        wrapper.write_header::<u32>("test", &paths).unwrap();
        assert_eq!(&wrapper.data_buf.written_data()[..16], b"MARIPOSALDOSDATA");

        wrapper.flush().unwrap();
        assert_eq!(&block_device.data.lock()[..16], b"MARIPOSALDOSDATA");
    }

    #[ktest]
    fn test_flush_if_needed() {
        let block_device = Arc::new(MemoryDisk::new(4096 * 2));
        let mut wrapper = ChunkingWriteWrapper {
            data_buf: DataBuf::new(4096),
            block_device: block_device.clone(),
            current_bid: Bid::new(0),
            end_bid: Bid::new(4),
        };

        // Test case 1: Buffer doesn't need flushing (<= BLOCK_SIZE)
        wrapper.data_buf.write_bytes(&[1; 3072]);
        assert_eq!(wrapper.current_bid, Bid::new(0));
        wrapper.flush_if_needed().unwrap();
        assert_eq!(wrapper.current_bid, Bid::new(0));

        // Test case 2: Buffer needs flushing (> BLOCK_SIZE)
        wrapper.data_buf.write_bytes(&[2; 3072]);
        wrapper.flush_if_needed().unwrap();
        assert_eq!(wrapper.current_bid, Bid::new(1));

        // Verify data was written to the first block
        let device_data = block_device.data.lock();
        assert_eq!(&device_data[..4], [1, 1, 1, 1]);
        assert_eq!(&device_data[4092..4096], [2, 2, 2, 2]);
    }
}
