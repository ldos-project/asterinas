// SPDX-License-Identifier: MPL-2.0

use alloc::{boxed::Box, format, sync::Arc, vec::Vec};
use core::error::Error;

use aster_block::{
    BLOCK_SIZE, BlockDevice,
    bio::{BioDirection, BioSegment},
    id::Bid,
};
use binary_serde::{BinarySerde, Endianness};
use ostd::orpc::path::Path;

/// A buffer for managing data which will be written bit by bit, but the extracted in larger blocks.
struct DataBuf {
    data: Vec<u8>,
}

impl DataBuf {
    /// Creates a new DataBuf with the specified size.
    pub fn new(size: usize) -> Self {
        DataBuf {
            data: Vec::with_capacity(size),
        }
    }

    /// Writes a [`BinarySerde`] value to the buffer at the current offset.
    ///
    /// [`BinarySerde`] is derivable. See
    /// https://docs.rs/binary_serde/latest/binary_serde/index.html.
    pub fn write_value<T>(&mut self, v: &T)
    where
        T: BinarySerde,
    {
        let serialization_start = self.data.len();
        self.data
            .resize(serialization_start + T::SERIALIZED_SIZE, 0);
        v.binary_serialize(&mut self.data[serialization_start..], Endianness::NATIVE);
    }

    /// Writes raw bytes to the buffer at the current position.
    pub fn write_bytes(&mut self, v: &[u8]) {
        self.data.extend(v);
    }

    /// Returns a slice containing all written data in the buffer.
    pub fn written_data(&self) -> &[u8] {
        &self.data
    }

    /// Removes the leading n bytes by shifting remaining data to start of buffer.
    pub fn delete(&mut self, n: usize) {
        let remaining = self.data.len() - n;
        self.data.drain(0..n);
        debug_assert_eq!(self.data.len(), remaining);
    }

    /// Returns the number of bytes currently in the buffer.
    pub fn len(&self) -> usize {
        self.data.len()
    }
}

/// Handles buffering and flushing data to a block device.
pub(crate) struct ChunkingWriteWrapper {
    data_buf: DataBuf,
    pub(crate) block_device: Arc<dyn aster_block::BlockDevice>,
    pub(crate) current_bid: Bid,
}

impl ChunkingWriteWrapper {
    /// Create a new wrapper for chunking writes.
    pub fn new(
        size: usize,
        block_device: Arc<dyn BlockDevice>,
        start_bid: Bid,
    ) -> ChunkingWriteWrapper {
        ChunkingWriteWrapper {
            data_buf: DataBuf::new(size),
            block_device,
            current_bid: start_bid,
        }
    }

    /// Writes a [`BinarySerde`] value to the output.
    ///
    /// [`BinarySerde`] is derivable. See
    /// https://docs.rs/binary_serde/latest/binary_serde/index.html.
    ///
    /// See [`DataBuf::write_value`].
    pub fn write_value<T>(&mut self, v: &T)
    where
        T: BinarySerde,
    {
        self.data_buf.write_value(v);
    }

    /// Flushes the buffer to storage if it contains more than one block's worth of data.
    pub fn flush_if_needed(&mut self) -> Result<(), Box<dyn Error + 'static>> {
        if self.data_buf.len() > BLOCK_SIZE {
            let n_written = self.flush()?;
            self.current_bid = self.current_bid + 1;
            self.data_buf.delete(n_written);
        }
        Ok(())
    }

    /// Flush the buffered data to storage regardless of the state.
    ///
    /// The same page may be written again if it was not full when this was called.
    pub fn flush(&mut self) -> Result<usize, Box<dyn Error + 'static>> {
        let raw_data = self.data_buf.written_data();
        let bio_segment = BioSegment::alloc(1, BioDirection::ToDevice);
        let n_written = bio_segment.writer()?.write(&mut raw_data.into());
        let _ = self
            .block_device
            .write_blocks_async(self.current_bid, bio_segment)?;
        Ok(n_written)
    }

    pub fn sync(&mut self) -> Result<(), Box<dyn Error + 'static>> {
        self.block_device.sync()?;
        Ok(())
    }

    /// Writes a structured header with magic number, type information, and paths.
    pub fn write_header<T>(&mut self, paths: &[Path]) -> Result<(), Box<dyn Error + 'static>> {
        // Write magic number
        let magic = b"MARIPOSALDOSDATA\0";
        self.data_buf.write_bytes(magic);

        // Create JSON header with type information and optional paths
        let mut json_header = format!("{{\"type\":\"{}\"", core::any::type_name::<T>());

        json_header.push_str(", \"oqueues\": [");
        for (i, path) in paths.iter().enumerate() {
            json_header.push_str(&format!("{}\"{}\"", if i > 0 { "," } else { "" }, path));
        }
        json_header.push(']');

        json_header.push('}');
        self.data_buf.write_bytes(json_header.as_bytes());

        self.data_buf.write_bytes(&[0]);

        let padding = 64 - (self.data_buf.len() % 64);
        if padding != 0 {
            // The weird [0; 64] avoids an allocation by referencing a static block of 0s.
            self.data_buf.write_bytes(&[0; 64][..padding]);
        }

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
        #[derive(Debug, PartialEq, Eq, BinarySerde)]
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
        assert_eq!(data.len(), 6);
        assert_eq!(
            u32::from_le_bytes(data[..4].try_into().unwrap()),
            0x12345678
        );
        assert_eq!(u16::from_le_bytes(data[4..6].try_into().unwrap()), 0x9012);
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
        };

        wrapper.write_header::<u32>(&paths).unwrap();
        assert_eq!(wrapper.data_buf.len(), 64);
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
