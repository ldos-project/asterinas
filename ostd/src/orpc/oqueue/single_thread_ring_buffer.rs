// SPDX-License-Identifier: MPL-2.0
//! A simple implementation of OQueues used for [`super::OQueue`] using locks. This is a baseline
//! implementation that supports all features, but it may have performance issues in highly
//! contended cases.

use alloc::alloc::AllocError;
#[cfg(debug_assertions)]
use core::any::{TypeId, type_name};
use core::{marker::PhantomData, mem::MaybeUninit};

use smallvec::SmallVec;

use crate::{orpc::oqueue::Cursor, util::untyped_box::SliceAllocation};

/// A non-thread-safe ring-buffer which supports multiple strong readers (consumers and strong
/// observers) and weak observers. This is used for the internal ring-buffers of the locking OQueue
/// implementation ([`LockingOQueueInner`]). It will management multiple instances to store
/// extracted data for observers.
///
/// This does not need to distinguish consumers from strong observers because it is not thread-safe,
/// so multiple readers can use the same head. When there are consumers they will all use head `0`.
/// Observation reads require `Copy` elements, so reading a consumed slot leaves a valid value
/// for other observers. Consuming a non-`Copy` element requires exactly one strong head.
///
/// The allocation stores opaque bytes, but the ring retains its element type. Typed accessors
/// therefore cannot reinterpret the allocation as another type.
///
/// Many operation require `Send` on the type they are manipulating, since the `RingBuffer` needs to
/// be `Send` so it can be placed inside a lock. This type can only be constructed with `T: Sized`,
/// but the type allows non-Sized types to simplify the definition of `OQueueInner` and avoid a
/// level of indirection.
///
/// ## Terminology
///
/// * a *strong reader* is either a consumer or a strong observers. Both have the same behavior in
///   the context of this type.
/// * a *slot* is a space in the buffer to hold an element. Often identified by an index into the
///   array. The term "slot" distinguishes a value form an index.
/// * an *index* is a position into the abstract infinite length sequence. This RingBuffer stores a
///   tail of that sequence.
///
/// ## Safety Note
///
/// This type is `Send` and that is only safe if the erased type held in this is also `Send`. This
/// is guaranteed by the type bound on [`RingBuffer::new`].
///
/// This will attempt to drop any values still in the buffer when this is dropped when they are
/// overwritten. However, if there is no consumer, this will not happen. Because of this, any
/// RingBuffer with a type which is not `Copy` should have a consumer.
pub(super) struct RingBuffer<T: ?Sized> {
    /// The type of the elements.
    #[cfg(debug_assertions)]
    element_type: TypeId,

    /// The name of the type of this ring buffer for debug messages. The errors are rare enough that
    /// this will likely be removed in the future.
    #[cfg(debug_assertions)]
    element_type_name: &'static str,

    /// The storage space for ringbuffer elements. This is a raw buffer which will be indexed based
    /// on the actual type of elements.
    buffer: SliceAllocation,

    /// The index of the next element to write in the buffer. Used by producers. This must be stored
    /// for each buffer because different ring may move at different speed if there are filters in
    /// queries.
    tail_index: usize,

    /// The heads used by consumers and strong observers.
    ///
    /// Using [`SmallVec`] here allows up to 2 heads to exist inline to the RingBuffer struct. This
    /// will be enough for many cases and avoids needing a pointer dereference to get the head in
    /// the consumer case.
    strong_reader_heads: SmallVec<[Option<usize>; 2]>,

    /// Function to drop all values still in the queue. This is safe, because it will simply discard
    /// values until the queue has no droppable values in it.
    drop_fn: fn(&mut RingBuffer<T>),
    // Construction requires Send + 'static elements. The non-owning marker lets descriptor-
    // generic queues contain an empty consumer ring even for unsized or non-Send elements.
    element: PhantomData<fn() -> T>,
}

impl<E: ?Sized> Drop for RingBuffer<E> {
    fn drop(&mut self) {
        (self.drop_fn)(self);
    }
}

impl<E: Send + 'static> RingBuffer<E> {
    /// Create a new ring buffer for a specific type.
    pub(super) fn new(n_buffer_elements: usize) -> Result<Self, AllocError> {
        // TODO: PERFORMANCE: We could potentially eliminate the padding and not align the values in
        // the buffer to save space. However, safety of this will require more thinking and may well
        // not help, so we leave it as is for now.
        let buffer = SliceAllocation::new::<E>(n_buffer_elements)?;

        Ok(RingBuffer {
            #[cfg(debug_assertions)]
            element_type: TypeId::of::<E>(),
            #[cfg(debug_assertions)]
            element_type_name: type_name::<E>(),
            buffer,
            tail_index: 0,
            strong_reader_heads: SmallVec::default(),
            drop_fn: |this| {
                let Some(head_id) = this.strong_reader_heads.iter().position(Option::is_some)
                else {
                    // There is no reader so there is no way to know what part of the buffer can be
                    // safely dropped.
                    return;
                };
                while let Some(v) = this.try_get_for_head(head_id) {
                    // SAFETY: The return value of `try_get_for_head` is always initialized. If head
                    // is the consumer and `E` has a `drop`, then we are dropping the values as
                    // if they were consumed. If the head is a strong observer, draining is not
                    // needed because `E: Copy`. Regardless, there are no active consumer or
                    // observers because `this` is being dropped.
                    unsafe { v.assume_init_read() };
                    *this.strong_reader_heads[head_id].as_mut().unwrap() += 1;
                }
            },
            element: PhantomData,
        })
    }

    /// Get a reference to the slot which is used for the slot index `i`. This does no checking as
    /// to the data that is actually held in that slot. It could be uninitialized.
    fn slot_mut(&mut self, i: usize) -> &mut MaybeUninit<E> {
        self.assert_type::<E>();

        // SAFETY: the ring was constructed for E, which is Send, so
        // reading the value out of `self` (which is `Send`) is allowed.
        unsafe { self.buffer.at_mut_unchecked(i) }
    }

    /// Try to write the value into the buffer. This will return `Some(v)` if `v` could not be moved
    /// into the buffer.
    pub(super) fn try_produce(&mut self, v: E) -> Option<E> {
        self.assert_type::<E>();

        // TODO: PERFORMANCE: The caller will often already have checked `can_produce`, so maybe we
        // should use a `produce_unchecked` instead.
        if !self.can_produce() {
            return Some(v);
        };

        let uninit_ref = self.slot_mut(self.mod_len(self.tail_index));

        // This can destroy an instance without dropping when there is no consumer. This is still
        // safe. See the note in the safety section of the RingBuffer doc.
        uninit_ref.write(v);

        self.tail_index += 1;

        None
    }

    /// Get a reference to a value at the head stored in `strong_reader_heads[head_id]`. The
    /// returned value is always initialized when returned. It is returned as `MaybeUninit<E>`, so
    /// allow mutating it leaving it in an uninitialized state. This does not update that head.
    fn try_get_for_head(&mut self, head_id: usize) -> Option<&mut MaybeUninit<E>> {
        self.assert_type::<E>();

        if self.can_get_for_head(head_id) {
            Some(self.slot_mut(self.mod_len(self.strong_reader_heads[head_id].unwrap())))
        } else {
            None
        }
    }

    /// Consume the value at head 0 (`strong_reader_heads[0]`).
    /// Panics if more than one strong head slot has been allocated.
    pub(super) fn try_consume(&mut self) -> Option<E> {
        self.assert_type::<E>();

        assert_eq!(
            self.strong_reader_heads.len(),
            1,
            "consuming from ring buffer with more than one head"
        );
        let res = self.try_get_for_head(0)?;
        // SAFETY: The return value of `try_get_for_head` is always initialized. The only head is
        // incremented below, meaning the slot will be treated as uninit until overwritten again
        // preventing a double drop.
        let res = unsafe { res.assume_init_read() };
        *self.strong_reader_heads[0].as_mut().unwrap() += 1;
        Some(res)
    }

    /// Copies the next observation for the specified reader.
    pub(super) fn try_strong_observe(&mut self, head_id: usize) -> Option<E>
    where
        E: Copy,
    {
        self.assert_type::<E>();

        let res = self.try_get_for_head(head_id)?;
        // SAFETY: The return value of `try_get_for_head` is always initialized. This leaves the
        // value initialized (as it is `Copy`). `E` is `!Drop` (due to being `Copy`), so it's OK to
        // leave the value initialized until it is overwritten.
        let res = unsafe { res.assume_init_read() };
        *self.strong_reader_heads[head_id].as_mut().unwrap() += 1;
        Some(res)
    }

    /// Copies the retained observation at the specified cursor.
    pub(super) fn try_weak_observe(&mut self, index: Cursor) -> Option<E>
    where
        E: Copy,
    {
        self.assert_type::<E>();

        let Cursor(index) = index;
        if index < self.tail_index.saturating_sub(self.buffer.len()) || index >= self.tail_index {
            return None;
        }
        let slot = self.slot_mut(self.mod_len(index));
        // SAFETY: `index < self.tail_index` so the slot must have been initialized at some point
        // and Copy values remain initialized even if consumed. Safety does
        // not depend on the slot containing the current value.
        Some(unsafe { slot.assume_init_read() })
    }
}

impl<E: ?Sized> RingBuffer<E> {
    /// Panics in debug builds if the type does not match this ring buffer.
    #[inline(always)]
    #[track_caller]
    fn assert_type<T: Send + 'static>(&self) {
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            TypeId::of::<T>(),
            self.element_type,
            "requested {} constructed with {}",
            type_name::<T>(),
            self.element_type_name
        );
    }

    fn mod_len(&self, i: usize) -> usize {
        i % self.buffer.len()
    }

    /// True if it is safe to produce by writing a value into the buffer and increasing the
    /// tail_index.
    pub(super) fn can_produce(&self) -> bool {
        let next_tail_slot = self.mod_len(self.tail_index + 1);

        !self
            .strong_reader_heads
            .iter()
            .flatten()
            .any(|h| next_tail_slot == self.mod_len(*h))
    }

    pub(super) fn can_get_for_head(&self, head_id: usize) -> bool {
        self.strong_reader_heads[head_id].unwrap() != self.tail_index
    }

    /// Allocate a new strong reader head ID. To allow consumers, this should only be called once.
    pub(super) fn new_strong_reader(&mut self) -> usize {
        if let Some(id) = self.strong_reader_heads.iter().position(Option::is_none) {
            self.strong_reader_heads[id] = Some(self.tail_index);
            id
        } else {
            let id = self.strong_reader_heads.len();
            self.strong_reader_heads.push(Some(self.tail_index));
            id
        }
    }

    /// Removes a head without changing the IDs of other readers.
    pub(super) fn remove_strong_reader(&mut self, head: usize) {
        self.strong_reader_heads[head] = None;
    }

    /// Returns whether this reader prevents producing another value.
    pub(super) fn is_head_full(&self, head: usize) -> bool {
        self.mod_len(self.tail_index + 1) == self.mod_len(self.strong_reader_heads[head].unwrap())
    }

    /// Get the cursor for the most recent value in `self`.
    pub(super) fn newest_cursor(&self) -> Cursor {
        Cursor(self.tail_index.saturating_sub(1))
    }

    /// Get the cursor for the oldest value still available in `self`.
    pub(super) fn oldest_cursor(&self) -> Cursor {
        let Cursor(i) = self.newest_cursor();
        // Return the (most recent() - (the buffer size) or zero if the buffer isn't full yet.
        if i < self.buffer.len() {
            Cursor(0)
        } else {
            Cursor(i - (self.buffer.len() - 1))
        }
    }
}

#[cfg(ktest)]
mod test {
    use super::*;
    use crate::prelude::*;

    #[ktest]
    fn consumed_copy_values_remain_observable() {
        let mut ring = RingBuffer::<u32>::new(3).unwrap();
        let head = ring.new_strong_reader();
        assert_eq!(ring.try_produce(1), None);
        assert_eq!(ring.try_produce(2), None);
        assert_eq!(ring.try_consume(), Some(1));
        assert_eq!(ring.try_weak_observe(Cursor(0)), Some(1));
        assert_eq!(ring.try_strong_observe(head), Some(2));
        assert_eq!(ring.try_consume(), None);
    }
}
