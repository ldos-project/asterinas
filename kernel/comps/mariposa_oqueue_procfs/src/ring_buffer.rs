// SPDX-License-Identifier: MPL-2.0

use alloc::{vec, vec::Vec};

/// Byte ring buffer with drop-oldest overflow semantics.
pub(crate) struct RingBuffer {
    buf: Vec<u8>,
    head: usize,
    tail: usize,
    len: usize,
}

impl RingBuffer {
    pub fn new(cap: usize) -> Self {
        let cap = cap.max(1);
        Self {
            buf: vec![0; cap],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    pub fn write_drop_oldest(&mut self, data: &[u8]) {
        let cap = self.buf.len();
        let data = if data.len() > cap {
            self.tail = 0;
            self.head = 0;
            self.len = 0;
            &data[data.len() - cap..]
        } else {
            let free = cap - self.len;
            if data.len() > free {
                let drop = data.len() - free;
                self.tail = (self.tail + drop) % cap;
                self.len -= drop;
            }
            data
        };

        for &b in data {
            self.buf[self.head] = b;
            self.head = (self.head + 1) % cap;
        }
        self.len += data.len();
    }

    pub fn read(&mut self, out: &mut [u8]) -> usize {
        let n = core::cmp::min(out.len(), self.len);
        for slot in out.iter_mut().take(n) {
            *slot = self.buf[self.tail];
            self.tail = (self.tail + 1) % self.buf.len();
        }
        self.len -= n;
        n
    }

    pub fn readable(&self) -> bool {
        self.len > 0
    }
}

#[cfg(ktest)]
mod test {
    use ostd::prelude::*;

    use super::*;

    #[ktest]
    fn write_then_read() {
        let mut r = RingBuffer::new(8);
        r.write_drop_oldest(b"abc");
        let mut out = [0u8; 4];
        let n = r.read(&mut out);
        assert_eq!(n, 3);
        assert_eq!(&out[..3], b"abc");
    }

    #[ktest]
    fn overflow_drops_oldest() {
        let mut r = RingBuffer::new(4);
        r.write_drop_oldest(b"abcd");
        r.write_drop_oldest(b"ef");
        let mut out = [0u8; 4];
        let n = r.read(&mut out);
        assert_eq!(n, 4);
        assert_eq!(&out, b"cdef");
    }

    #[ktest]
    fn data_larger_than_buffer() {
        let mut r = RingBuffer::new(4);
        r.write_drop_oldest(b"abcdef");
        let mut out = [0u8; 4];
        let n = r.read(&mut out);
        assert_eq!(n, 4);
        assert_eq!(&out, b"cdef");
    }
}
