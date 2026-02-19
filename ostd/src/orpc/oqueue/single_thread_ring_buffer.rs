// SPDX-License-Identifier: MPL-2.0
//! A simple implementation of OQueues used for [`super::OQueue`] using locks. This is a baseline
//! implementation that supports all features, but it may have performance issues in highly
//! contended cases.

use alloc::alloc::AllocError;
use core::{
    any::{TypeId, type_name},
    mem::MaybeUninit,
};

use smallvec::SmallVec;

use crate::{orpc::oqueue::Cursor, util::untyped_box::SliceAllocation};

/// A non-thread-safe ring-buffer which supports multiple strong readers (consumers and strong
/// observers) and weak observers. This is used for the internal ring-buffers of the locking OQueue
/// implementation ([`LockingOQueueInner`]). It will management multiple instances to store
/// extracted data for observers.
///
/// This does not need to distinguish consumers from strong observers because it is not thread-safe,
/// so multiple readers can use the same head. When there are consumers they will all use head `0`.
/// Consumers are incompatible with strong or weak observers. This is represented by the fact that
/// `try_consume` is unsafe, because it will cause UB in later `try_strong_observe` and
/// `try_weak_observe` calls.
///
/// The values in the ring buffer and opaque sequences of bytes to erase their type. Some accessors
/// have type parameters to specify the type that is in the buffer (which is also dynamically
/// checked). Specifically, operations which "take" a non-`Copy` value out of the buffer. This is
/// needed to make sure that a potential `Drop` on that type is called.
///
/// Many operation require `Send` on the type they are manipulating, since the `RingBuffer` needs to
/// be `Send` so it can be placed inside a lock.
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
/// is guaranteed by the type bound on [`RingBuffer::new`]. The implementation is internally checked
/// by the bound on [`RingBuffer::slot_mut`].
///
/// This will attempt to drop any values still in the buffer when this is dropped when they are
/// overwritten. However, if there is no consumer, this will not happen. Because of this, any
/// RingBuffer with a type which is not `Copy` should have a consumer.
pub(super) struct RingBuffer {
    /// The type of the elements.
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
    strong_reader_heads: SmallVec<[usize; 2]>,

    /// Function to drop all values still in the queue. This is safe, because it will simply discard
    /// values until the queue has no droppable values in it.
    drop_fn: fn(&mut RingBuffer),
}

impl Drop for RingBuffer {
    fn drop(&mut self) {
        (self.drop_fn)(self);
    }
}

impl RingBuffer {
    /// Create a new ring buffer for a specific type.
    pub(super) fn new<T: Send + 'static>(n_buffer_elements: usize) -> Result<Self, AllocError> {
        // TODO: PERFORMANCE: We could potentially eliminate the padding and not align the values in
        // the buffer to save space. However, safety of this will require more thinking and may well
        // not help, so we leave it as is for now.
        let buffer = SliceAllocation::new::<T>(n_buffer_elements)?;

        Ok(RingBuffer {
            element_type: TypeId::of::<T>(),
            buffer,
            tail_index: 0,
            strong_reader_heads: SmallVec::default(),
            #[cfg(debug_assertions)]
            element_type_name: type_name::<T>(),
            drop_fn: |this| {
                if this.strong_reader_heads.is_empty() {
                    // There is no reader so there is no way to know what part of the buffer can be
                    // safely dropped.
                    return;
                }
                let head_id = 0;
                while let Some(v) = this.try_get_for_head::<T>(head_id) {
                    // SAFETY: The return value of `try_get_for_head` is always initialized. If head
                    // ID 0 is the consumer and `T` has a `drop`, then we are dropping the values as
                    // if they were consumed. If head ID 0 is a strong observer, draining is not
                    // needed because `T: Copy`. Regardless, there are no active consumer or
                    // observers because `this` is being dropped.
                    unsafe { v.assume_init_read() };
                    this.strong_reader_heads[head_id] += 1;
                }
            },
        })
    }

    /// Panic if the type doesn't match this ringbuffer.
    ///
    /// TODO: PERFORMANCE: This is called a lot, and while it will probably be elided most of the
    /// time. Making the callers to this unsafe and having their callers perform the check might
    /// reduce the number of checks.
    #[inline(always)]
    #[track_caller]
    pub(super) fn assert_type<T: Send + 'static>(&self) {
        #[cfg(debug_assertions)]
        let element_type_name = self.element_type_name;
        #[cfg(not(debug_assertions))]
        let element_type_name = "[not captured]";
        assert_eq!(
            TypeId::of::<T>(),
            self.element_type,
            "requested {} constructed with {}",
            type_name::<T>(),
            element_type_name
        );
    }

    fn mod_len(&self, i: usize) -> usize {
        i % self.buffer.len()
    }

    /// Get a reference to the slot which is used for the slot index `i`. This does no checking as
    /// to the data that is actually held in that slot. It could be uninitialized.
    fn slot_mut<T: Send + 'static>(&mut self, i: usize) -> &mut MaybeUninit<T> {
        self.assert_type::<T>();

        // SAFETY: buffer was constructed with type `T` as checked above and the type is `Send`, so
        // reading the value out of `self` (which is `Send`) is allowed.
        unsafe { self.buffer.at_mut_unchecked(i) }
    }

    /// True if it is safe to produce by writing a value into the buffer and increasing the
    /// tail_index.
    pub(super) fn can_produce(&self) -> bool {
        let next_tail_slot = self.mod_len(self.tail_index + 1);

        !self
            .strong_reader_heads
            .iter()
            .any(|h| next_tail_slot == self.mod_len(*h))
    }

    /// Try to write the value into the buffer. This will return `Some(v)` if `v` could not be moved
    /// into the buffer.
    pub(super) fn try_produce<T: Send + 'static>(&mut self, v: T) -> Option<T> {
        self.assert_type::<T>();

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

    pub(super) fn can_get_for_head(&self, head_id: usize) -> bool {
        self.strong_reader_heads[head_id] != self.tail_index
    }

    /// Get a reference to a value at the head stored in `strong_reader_heads[head_id]`. The
    /// returned value is always initialized when returned. It is returned as `MaybeUninit<T>`, so
    /// allow mutating it leaving it in an uninitialized state. This does not update that head.
    fn try_get_for_head<T: Send + 'static>(
        &mut self,
        head_id: usize,
    ) -> Option<&mut MaybeUninit<T>> {
        self.assert_type::<T>();

        if self.can_get_for_head(head_id) {
            Some(self.slot_mut(self.mod_len(self.strong_reader_heads[head_id])))
        } else {
            None
        }
    }

    /// Consume the value at head 0 (`strong_reader_heads[0]`).
    ///
    /// ## Safety
    ///
    /// There must be only one head, i.e., `strong_reader_heads.len() == 1`, and weak observers may
    /// not be in use.
    pub(super) unsafe fn try_consume<T: Send + 'static>(&mut self) -> Option<T> {
        self.assert_type::<T>();

        debug_assert_eq!(
            self.strong_reader_heads.len(),
            1,
            "consuming from ring buffer with more than one head"
        );
        let res = self.try_get_for_head(0)?;
        // SAFETY: The return value of `try_get_for_head` is always initialized. The only head is
        // incremented below, meaning the slot will be treated as uninit until overwritten again
        // preventing a double drop.
        let res = unsafe { res.assume_init_read() };
        self.strong_reader_heads[0] += 1;
        Some(res)
    }

    pub(super) fn try_strong_observe<T>(&mut self, head_id: usize) -> Option<T>
    where
        T: Copy + Send + 'static,
    {
        self.assert_type::<T>();

        let res = self.try_get_for_head(head_id)?;
        // SAFETY: The return value of `try_get_for_head` is always initialized. This leaves the
        // value initialized (as it is `Copy`). `T` is `!Drop` (due to being `Copy`), so it's OK to
        // leave the value initialized until it is overwritten.
        let res = unsafe { res.assume_init_read() };
        self.strong_reader_heads[head_id] += 1;
        Some(res)
    }

    pub(super) fn try_weak_observe<T>(&mut self, index: Cursor) -> Option<T>
    where
        T: Copy + Send + 'static,
    {
        self.assert_type::<T>();

        let Cursor(index) = index;
        if index < self.tail_index.saturating_sub(self.buffer.len()) || index >= self.tail_index {
            return None;
        }
        let slot = self.slot_mut(self.mod_len(index));
        // SAFETY: `index < self.tail_index` so the slot must have been initialized at some point
        // and since there are no consumers the value will not have been invalidated. Safety does
        // not depend on the slot containing the current value.
        Some(unsafe { slot.assume_init_read() })
    }

    /// Allocate a new strong reader head ID. To allow consumers, this should only be called once.
    pub(super) fn new_strong_reader(&mut self) -> usize {
        let id = self.strong_reader_heads.len();
        self.strong_reader_heads.push(self.tail_index);
        id
    }

    /// Get the cursor for the most recent value in `self`.
    pub fn newest_cursor(&self) -> Cursor {
        Cursor(self.tail_index.saturating_sub(1))
    }

    /// Get the cursor for the oldest value still available in `self`.
    pub fn oldest_cursor(&self) -> Cursor {
        let Cursor(i) = self.newest_cursor();
        // Return the (most recent() - (the buffer size) or zero if the buffer isn't full yet.
        if i < self.buffer.len() {
            Cursor(0)
        } else {
            Cursor(i - (self.buffer.len() - 1))
        }
    }
}
