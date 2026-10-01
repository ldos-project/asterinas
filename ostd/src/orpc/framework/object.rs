// SPDX-License-Identifier: MPL-2.0

//! The framework for ORPC objects
//!
//! This does not cover the methods of ORPC objects.

use core::{
    any::{Any, TypeId, type_name},
    fmt::Display,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::Serialize;

/// The next object ID to assign.
static NEXT_OBJECT_ID: AtomicU64 = AtomicU64::new(1);

/// The ID of an [ORPC object](OrpcObject).
/// 
/// The [`Display`] implementation for  [`ObjectId`] uses the format `id` followed by hex bytes
/// (e.g., `iddeadbeef`). However, it will attempt to show any tag added by [`Self::new_tagged`]
/// using the overall format of `id` followed by the quoted tag and the remaining bytes (e.g.,
/// `id'TG'adbeef`).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
pub struct ObjectId(u64);

impl Display for ObjectId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let tag = (self.0 >> 48) as u16;
        let tag = tag.to_le_bytes();

        if let Ok(tag_str) = core::str::from_utf8(&tag) && tag_str.is_ascii() {
            let rest = self.0 & 0xFF_FF_FF_FF_FF_FF;
            write!(f, "id'{}'{:012x}", tag_str.escape_debug(), rest)
        } else {
            write!(f, "id{:016x}", self.0)
        }
    }
}

impl ObjectId {
    /// Create a new unique ID.
    /// 
    /// If you have trouble identifying IDs in logs, look at [`Self::new_tagged`].
    pub fn new() -> ObjectId {
        ObjectId(NEXT_OBJECT_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// Create a new unique ID. The first 2 bytes of `tag` is included in the high-bytes of the
    /// output, but may be arbitrarily overwritten by the actual unique ID. The [`Display`]
    /// implementation for [`ObjectId`] displays these characters.
    /// 
    /// Note: The unique ID will mix with the tag if the ID is greater than 2^48, but this will
    /// realistically not happen for at least 90 years (assuming one ID per microsecond).
    pub fn new_tagged(tag: &str) -> ObjectId {
        let tag_bytes = tag.as_bytes();
        let mut tag_truncated = [0u8; 2];
        let len = tag_bytes.len().min(tag_truncated.len());
        tag_truncated[..len].copy_from_slice(&tag_bytes[..len]);
        let tag = u16::from_le_bytes(tag_truncated) as u64;
        // TODO(arthurp):PERFORMANCE: This could easily become a performance bottleneck because it
        // will create an atomic operation during every object creation.
        let unique_id = NEXT_OBJECT_ID.fetch_add(1, Ordering::Relaxed);
        ObjectId((tag << 48) | unique_id)
    }

    // TODO(arthurp):PERFORMANCE: There is no way to create the same ID deterministically. This
    // means that objects will need to store their ObjectId even if they already store some other ID
    // which could be used to compute a unique ID as needed.
}

/// Metadata about an [`OrpcObject`].
pub trait Metadata: 'static {
    /// The textual representation of the type of the object, generally as provided by
    /// [`type_name`]. Like that function, there are no guarantees as to the format and uniqueness
    /// of this string.
    fn type_name(&self) -> &'static str {
        type_name::<Self>()
    }

    /// The unique ID of the type of the object. This is only unique and stable within a build.
    fn type_id(&self) -> TypeId {
        TypeId::of::<Self>()
    }
}

/// An object which can interact with the ORPC framework.
///
/// This provides introspection capabilities primarily to enable observation.
pub trait OrpcObject: Any + 'static {
    /// Return the global ID of `self`. This value will be unique among all objects in the system.
    fn id(&self) -> ObjectId;

    /// Get metadata about `self`.
    ///
    /// *Implementation note*: This can generally be implemented by implementing [`Metadata`] for
    /// `Self` and returning `self`. In most cases, this will totally eliminate the storage overhead
    /// of the metadata.
    fn metadata(&self) -> &dyn Metadata;
}
