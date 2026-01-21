// SPDX-License-Identifier: MPL-2.0

//! Box-like types without a type parameter.
//!
//! These store their layout dynamically to allow safe deallocation instead of relying on a static
//! types to determine the layout.
//!
//! There are a wide range of possible design choices, trading off performance, safety, and
//! capabilities. This module can be expanded with additional options as needed (for example, a
//! packed slice which can return misaligned pointer).
//!
//! This is inspired by the [`untyped_box` crate](https://crates.io/crates/untyped-box).

use alloc::alloc::{AllocError, Allocator, Global};
use core::{alloc::Layout, mem::MaybeUninit, ptr::NonNull};

/// A type-erased boxed slice.
///
/// This encapsulates the pointer math, layout handling, and memory management. It does not try to
/// provide any safe interface. It does not store the type it was created with, so it cannot check
/// it dynamically.
///
/// This does **not** handle dropping values in the buffer. It cannot because the values may or may
/// not be initialized (see the use of [`MaybeUninit`] in the method return types). Code using this
/// type should include a drop implementation which iterates over valid indices and uses
/// [`MaybeUninit::assume_init_drop`] (or [`assume_init_read`](`MaybeUninit::assume_init_read`)) to
/// drop the value in that location.
///
/// ## Implementation
///
/// Theoretically this could be made smaller by only storing the array layout, and inferring the
/// stride in the indexing operations. However, this would make [`Self::len()`] require a division
/// and `len` is deamed to be a common and useful operation.
///
/// The array layout must be stored so that it can be used in the `drop` implementation, so the
/// minimum size of this is 24.
pub struct SliceAllocation<A: Allocator = Global> {
    /// A pointer to the underlying buffer. It has size `stride * len`.
    ptr: NonNull<u8>,
    /// The alignment of the elements. This is required for safe deallocation.
    alignment: usize,
    /// The stride of the elements of the slice. Used to compute the offset of array elements.
    stride: usize,
    /// The number of elements in the slice.
    len: usize,
    /// The allocator.
    alloc: A,
}

unsafe impl<A: Allocator + Send> Send for SliceAllocation<A> {}

impl SliceAllocation {
    /// Create a new allocation for a slice (an array, if you will) with elements of type `T` and
    /// length `len`. Use the global allocator.
    pub fn new<T>(len: usize) -> Result<Self, AllocError> {
        Self::new_in::<T>(len, Global)
    }
}

impl<A: Allocator> SliceAllocation<A> {
    /// Create a new allocation for a slice (an array, if you will) with elements of type `T` and
    /// length `len`. Use the given allocator.
    pub fn new_in<T>(len: usize, alloc: A) -> Result<Self, AllocError> {
        let elem_layout = Layout::new::<T>();
        // Pad the element layout to it's alignment so they are correctly aligned elements in the
        // array.
        let padded_elem_layout = elem_layout.pad_to_align();
        let stride = padded_elem_layout.size();
        // Create a layout for all elements in the array.
        let array_layout = Layout::from_size_align(stride * len, padded_elem_layout.align())
            .map_err(|_| AllocError)?;
        let ptr = alloc.allocate(array_layout)?.cast::<u8>();
        Ok(Self {
            ptr,
            alignment: array_layout.align(),
            stride,
            len,
            alloc,
        })
    }

    /// Get a mutable pointer to an element.
    ///
    /// ## SAFETY
    ///
    /// `T` must be exactly the same as that passed to `new` (or `new_in`) when constructing `self`.
    pub unsafe fn at_mut_ptr_unchecked<T>(&mut self, i: usize) -> *mut T {
        // SAFETY: The memory was allocated for the array layout
        unsafe { self.ptr.add(i * self.stride).cast::<T>().as_ptr() }
    }

    /// Get a const pointer to an element.
    ///
    /// ## SAFETY
    ///
    /// `T` must be exactly the same as that passed to `new` (or `new_in`) when constructing `self`.
    pub unsafe fn at_ptr_unchecked<T>(&self, i: usize) -> *const T {
        // SAFETY: The memory was allocated for the array layout
        unsafe { self.ptr.add(i * self.stride).cast::<T>().as_ptr() }
    }

    /// Get a mutable reference to an element. Elements are not initialized at creation, so the result is `MaybeUninit`.
    ///
    /// ## SAFETY
    ///
    /// `T` must be exactly the same as that passed to `new` (or `new_in`) when constructing `self`.
    pub unsafe fn at_mut_unchecked<T>(&mut self, i: usize) -> &mut MaybeUninit<T> {
        // SAFETY: The memory was allocated for the array layout
        unsafe {
            self.ptr
                .add(i * self.stride)
                .cast::<MaybeUninit<T>>()
                .as_mut()
        }
    }

    /// Get a reference to an element. Elements are not initialized at creation, so the result is `MaybeUninit`.
    ///
    /// ## SAFETY
    ///
    /// `T` must be exactly the same as that passed to `new` (or `new_in`) when constructing `self`.
    pub unsafe fn at_ref_unchecked<T>(&self, i: usize) -> &MaybeUninit<T> {
        // SAFETY: The memory was allocated for the array layout
        unsafe {
            self.ptr
                .add(i * self.stride)
                .cast::<MaybeUninit<T>>()
                .as_ref()
        }
    }

    /// The layout of the overall array.
    pub fn layout(&self) -> Layout {
        Layout::from_size_align(self.stride * self.len, self.alignment).expect(
            "layout was invalid, despite being valid when constructed identically in new_in",
        )
    }

    /// The length of the array in elements.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the array contains no elements.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<A: Allocator> Drop for SliceAllocation<A> {
    fn drop(&mut self) {
        let layout = self.layout();
        // SAFETY: The layout is a reconstructed form of array_layout in `Self::new_in`. The data at
        // `self.ptr` is owned by self and never escapes.
        unsafe {
            self.alloc.deallocate(self.ptr, layout);
        }
    }
}

#[cfg(ktest)]
mod test {
    use super::*;
    use crate::prelude::*;

    #[ktest]
    fn test_basic_allocation() {
        let allocation = SliceAllocation::new::<u64>(5).unwrap();
        assert_eq!(allocation.len(), 5);
    }

    #[ktest]
    fn test_zero_length_allocation() {
        let allocation = SliceAllocation::new::<u64>(0).unwrap();
        assert_eq!(allocation.len(), 0);
    }

    #[ktest]
    fn test_write_and_read_elements() {
        let mut allocation = SliceAllocation::new::<u64>(2).unwrap();

        // Write values
        unsafe {
            allocation.at_mut_unchecked::<u64>(0).write(100);
            allocation.at_mut_unchecked::<u64>(1).write(200);
        }

        // Read values
        unsafe {
            assert_eq!(allocation.at_ref_unchecked::<u64>(0).assume_init(), 100);
            assert_eq!(allocation.at_ref_unchecked::<u64>(1).assume_init(), 200);
        }
    }

    #[ktest]
    fn test_write_and_read_via_pointers() {
        let mut allocation = SliceAllocation::new::<i32>(2).unwrap();

        unsafe {
            allocation.at_mut_ptr_unchecked::<i32>(0).write(42);
            allocation.at_mut_ptr_unchecked::<i32>(1).write(-99);

            assert_eq!(*allocation.at_ptr_unchecked::<i32>(0), 42);
            assert_eq!(*allocation.at_ptr_unchecked::<i32>(1), -99);
        }
    }

    #[ktest]
    fn test_layout_calculation() {
        let allocation = SliceAllocation::new::<u64>(10).unwrap();

        let layout = allocation.layout();

        // Total layout should accommodate all elements
        assert!(layout.size() >= core::mem::size_of::<u64>() * 10);
        assert_eq!(layout.align(), core::mem::align_of::<u64>());
    }

    #[repr(align(64))]
    #[derive(Clone, Copy)]
    struct Aligned64 {
        value: u64,
    }

    #[ktest]
    fn test_large_alignment_type() {
        let mut allocation = SliceAllocation::new::<Aligned64>(3).unwrap();

        unsafe {
            allocation
                .at_mut_unchecked::<Aligned64>(0)
                .write(Aligned64 { value: 111 });
            allocation
                .at_mut_unchecked::<Aligned64>(2)
                .write(Aligned64 { value: 222 });

            assert_eq!(
                allocation
                    .at_ref_unchecked::<Aligned64>(0)
                    .assume_init()
                    .value,
                111
            );
            assert_eq!(
                allocation
                    .at_ref_unchecked::<Aligned64>(2)
                    .assume_init()
                    .value,
                222
            );
        }
    }

    #[ktest]
    fn test_layout_calculation_large_alignment() {
        let allocation = SliceAllocation::new::<Aligned64>(3).unwrap();

        // Verify layout
        let layout = allocation.layout();
        assert!(layout.size() >= 64 * 3);
        assert_eq!(layout.align(), 64);
    }
}
