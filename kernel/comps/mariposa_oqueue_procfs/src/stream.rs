// SPDX-License-Identifier: MPL-2.0

use alloc::{
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use core::sync::atomic::{AtomicBool, Ordering};

use minicbor_serde::Serializer;
use ostd::{
    orpc::{
        oqueue::StrongObserver,
        path::Path,
        sync::{BlockOnMany, Blocker},
    },
    sync::Mutex,
};
use serde::Serialize;

use crate::{config::ProcfsConfig, ring_buffer::RingBuffer};

pub trait StreamSource: Send + Sync {
    fn poll_into(&self, out: &mut [u8]) -> usize;
    fn wait_readable(&self);
    fn is_readable(&self) -> bool;
}

pub(crate) struct TypedStreamSource<U: Copy + Send + Serialize + 'static> {
    queue_path: Path,
    type_name: &'static str,
    observer: Mutex<StrongObserver<U>>,
    ring: Mutex<RingBuffer>,
    header_done: AtomicBool,
}

impl<U: Copy + Send + Serialize + 'static> TypedStreamSource<U> {
    pub fn new(
        queue_path: Path,
        type_name: &'static str,
        observer: StrongObserver<U>,
    ) -> Arc<Self> {
        let buf_size = ProcfsConfig::from_kcmdline().buffer_bytes;
        Arc::new(Self {
            queue_path,
            type_name,
            observer: Mutex::new(observer),
            ring: Mutex::new(RingBuffer::new(buf_size)),
            header_done: AtomicBool::new(false),
        })
    }

    fn encode_value<T: Serialize>(value: &T) -> Vec<u8> {
        let mut serializer = Serializer::new(Vec::new());
        value.serialize(&mut serializer).unwrap();
        serializer.into_encoder().into_writer()
    }

    fn header_cbor(&self) -> Vec<u8> {
        #[derive(Serialize)]
        struct Header {
            name: String,
            oqueues: [String; 1],
            type_name: &'static str,
        }

        let name = self.queue_path.to_string();
        Self::encode_value(&Header {
            name: name.clone(),
            oqueues: [name],
            type_name: self.type_name,
        })
    }
}

impl<U: Copy + Send + Serialize + 'static> StreamSource for TypedStreamSource<U> {
    fn poll_into(&self, out: &mut [u8]) -> usize {
        if !self.header_done.swap(true, Ordering::SeqCst) {
            self.ring.lock().write_drop_oldest(&self.header_cbor());
        }

        let observer = self.observer.lock();
        while let Ok(Some(v)) = observer.try_strong_observe() {
            self.ring.lock().write_drop_oldest(&Self::encode_value(&v));
        }

        self.ring.lock().read(out)
    }

    fn wait_readable(&self) {
        if self.is_readable() {
            return;
        }

        let mut blocker = BlockOnMany::new();
        let observer = self.observer.lock();
        blocker.block_on([&*observer as &dyn Blocker].into_iter());
    }

    fn is_readable(&self) -> bool {
        !self.header_done.load(Ordering::SeqCst)
            || self.observer.lock().should_try()
            || self.ring.lock().readable()
    }
}
