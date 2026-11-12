// SPDX-License-Identifier: MPL-2.0

//! Query-backed streams with independently positioned buffered and inline readers.

use alloc::{alloc::AllocError, boxed::Box};
use core::{
    any::{Any, TypeId},
    marker::PhantomData,
};

use slotmap::{SlotMap, new_key_type};

use super::super::{
    Cursor, ElementDescriptor, OQueueError, ObservationQuery, single_thread_ring_buffer::RingBuffer,
};

new_key_type! {
    pub(super) struct ReaderKey;
}

pub(super) enum ReaderMode {
    Strong,
    RevocableStrong,
    Weak,
}

pub(super) enum ReadPosition {
    Next,
    At(Cursor),
}

/// A destination whose type is guaranteed by its observer handle.
pub(super) struct ErasedOutput<'a> {
    pointer: *mut (),
    #[cfg(debug_assertions)]
    type_id: TypeId,
    lifetime: PhantomData<&'a mut ()>,
}

impl<'a> ErasedOutput<'a> {
    /// # Safety
    /// `pointer` must be aligned storage for the handle's output type, for `'a`.
    pub(super) unsafe fn new(pointer: *mut (), _type_id: TypeId) -> Self {
        Self {
            pointer,
            #[cfg(debug_assertions)]
            type_id: _type_id,
            lifetime: PhantomData,
        }
    }

    /// # Safety
    /// `U` must be the output type of the handle that supplied this destination.
    unsafe fn write<U: 'static>(self, value: U) {
        #[cfg(debug_assertions)]
        debug_assert_eq!(self.type_id, TypeId::of::<U>());
        // SAFETY: The constructor and caller guarantee writable storage of type U.
        unsafe { self.pointer.cast::<U>().write(value) };
    }
}

/// A boxed typed callback, downcast once when attaching it to a stream.
pub(in crate::orpc::oqueue) type ErasedHandler = Box<dyn Any + Send>;

/// A query-backed sequence of observations and the readers attached to it.
///
/// A stream evaluates one query on produced queue elements and retains its results in a ring
/// shared by its buffered readers. Strong readers have independent heads; weak readers access
/// retained results by cursor. Inline readers receive results directly without a ring head.
/// The query's output type is erased, while its input descriptor remains `D`.
pub(super) trait ObservationStream<D: ElementDescriptor>: Send {
    /// Attaches a reader with the requested mode.
    fn attach_reader(&mut self, mode: ReaderMode) -> Result<ReaderKey, AllocError>;

    /// Detaches a reader, returning whether none remain.
    fn detach_reader(&mut self, reader: ReaderKey) -> bool;

    /// Returns whether a full, non-revocable reader blocks production.
    fn blocks_production(&self) -> bool;

    /// Revokes full revocable readers.
    fn revoke_full_readers(&mut self);

    /// Runs the query and produces any result to attached readers.
    fn produce<'a>(&mut self, value: &'a D::Element<'a>);

    /// Returns whether a strong reader has an observation or is revoked.
    fn can_get(&self, reader: ReaderKey) -> bool;

    /// Returns the oldest and newest retained cursors.
    fn cursor_bounds(&self) -> (Cursor, Cursor);

    /// Writes an observation iff this returns `Ok(true)`.
    ///
    /// # Safety
    /// `output` must be valid storage for this stream's output type.
    unsafe fn read_into(
        &mut self,
        reader: ReaderKey,
        position: ReadPosition,
        output: ErasedOutput<'_>,
    ) -> Result<bool, OQueueError>;

    /// Drains this reader and switches it to inline observation, preserving other readers.
    fn make_inline(&mut self, reader: ReaderKey, handler: ErasedHandler);
}

pub(super) struct TypedObservationStream<D: ElementDescriptor, U> {
    projection: ObservationQuery<D, U>,
    ring: Option<RingBuffer<U>>,
    len: usize,
    readers: SlotMap<ReaderKey, Reader<U>>,
}

enum Reader<U> {
    Strong { head: usize, revocable: bool },
    Weak,
    Revoked,
    Inline(Box<dyn Fn(&U) + Send>),
}

impl<D: ElementDescriptor, U: Copy + Send + 'static> TypedObservationStream<D, U> {
    pub(super) fn new(projection: ObservationQuery<D, U>, len: usize) -> Self {
        Self {
            projection,
            ring: None,
            len,
            readers: SlotMap::with_key(),
        }
    }

    fn release_unused_ring(&mut self) {
        if !self
            .readers
            .values()
            .any(|r| matches!(r, Reader::Strong { .. } | Reader::Weak))
        {
            self.ring = None;
        }
    }
}

impl<D: ElementDescriptor, U: Copy + Send + 'static> ObservationStream<D>
    for TypedObservationStream<D, U>
{
    fn attach_reader(&mut self, mode: ReaderMode) -> Result<ReaderKey, AllocError> {
        if self.ring.is_none() {
            self.ring = Some(RingBuffer::new(self.len)?);
        }
        let reader = match mode {
            ReaderMode::Weak => Reader::Weak,
            _ => Reader::Strong {
                head: self.ring.as_mut().unwrap().new_strong_reader(),
                revocable: matches!(mode, ReaderMode::RevocableStrong),
            },
        };
        Ok(self.readers.insert(reader))
    }

    fn detach_reader(&mut self, reader: ReaderKey) -> bool {
        if let Some(Reader::Strong { head, .. }) = self.readers.remove(reader) {
            self.ring.as_mut().unwrap().remove_strong_reader(head);
        }
        self.release_unused_ring();
        self.readers.is_empty()
    }

    fn blocks_production(&self) -> bool {
        self.readers.values().any(|reader| match reader {
            Reader::Strong {
                head,
                revocable: false,
            } => self.ring.as_ref().unwrap().is_head_full(*head),
            _ => false,
        })
    }

    fn revoke_full_readers(&mut self) {
        if let Some(ring) = &mut self.ring {
            for reader in self.readers.values_mut() {
                if let Reader::Strong {
                    head,
                    revocable: true,
                } = reader
                    && ring.is_head_full(*head)
                {
                    ring.remove_strong_reader(*head);
                    *reader = Reader::Revoked;
                }
            }
        }
        self.release_unused_ring();
    }

    fn produce<'a>(&mut self, value: &'a D::Element<'a>) {
        if self.ring.is_none() && self.readers.values().all(|r| matches!(r, Reader::Revoked)) {
            return;
        }
        if let Some(value) = self.projection.call(value) {
            if let Some(ring) = &mut self.ring {
                let rejected = ring.try_produce(value);
                debug_assert!(rejected.is_none());
            }
            for reader in self.readers.values() {
                if let Reader::Inline(handler) = reader {
                    handler(&value);
                }
            }
        }
    }

    fn can_get(&self, reader: ReaderKey) -> bool {
        match self.readers[reader] {
            Reader::Strong { head, .. } => self.ring.as_ref().unwrap().can_get_for_head(head),
            Reader::Revoked => true,
            _ => unreachable!("only buffered strong observers have readiness checks"),
        }
    }

    fn cursor_bounds(&self) -> (Cursor, Cursor) {
        let ring = self
            .ring
            .as_ref()
            .expect("weak observers retain their ring");
        (ring.oldest_cursor(), ring.newest_cursor())
    }

    unsafe fn read_into(
        &mut self,
        reader: ReaderKey,
        position: ReadPosition,
        output: ErasedOutput<'_>,
    ) -> Result<bool, OQueueError> {
        if matches!(self.readers[reader], Reader::Revoked) {
            return super::super::RevokedSnafu.fail();
        }
        let ring = self
            .ring
            .as_mut()
            .expect("buffered observers retain their ring");
        let value = match (&self.readers[reader], position) {
            (Reader::Strong { head, .. }, ReadPosition::Next) => ring.try_strong_observe(*head),
            (Reader::Weak, ReadPosition::At(cursor)) => ring.try_weak_observe(cursor),
            _ => unreachable!("read position must match observer kind"),
        };
        if let Some(value) = value {
            // SAFETY: The caller supplies storage for this stream's U.
            unsafe { output.write(value) };
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn make_inline(&mut self, reader: ReaderKey, handler: ErasedHandler) {
        let handler = *handler
            .downcast::<Box<dyn Fn(&U) + Send>>()
            .expect("inline callback must match the stream's output type");
        if let Reader::Strong { head, .. } = self.readers[reader] {
            let ring = self.ring.as_mut().unwrap();
            while let Some(value) = ring.try_strong_observe(head) {
                handler(&value);
            }
            ring.remove_strong_reader(head);
        } else {
            debug_assert!(matches!(self.readers[reader], Reader::Revoked));
        }
        self.readers[reader] = Reader::Inline(handler);
        self.release_unused_ring();
    }
}
