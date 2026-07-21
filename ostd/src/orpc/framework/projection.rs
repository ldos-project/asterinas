// SPDX-License-Identifier: MPL-2.0

//! Tools for projecting values out of OQueues so they can be observed.

use alloc::sync::Arc;

use crate::orpc::framework::object::{ObjectId, OrpcObject};

/// Provide a default projection for observing a type.
/// 
/// This provides a simple way to "access the data in an OQueue" without writing a specific
/// projection. This is how `oqfs` exposes data in an OQueue.
pub trait DefaultProjection {
    /// The type of the default projected value.
    type Projected: Copy + Sync;
    /// The projection function from [`Self`] to [`Projected`]. This will be passed to
    /// [`attach_strong_observer`] and [`attach_weak_observer`] as the query.
    /// 
    /// [`attach_strong_observer`]: `crate::orpc::oqueue::OQueueBase::attach_strong_observer`
    /// [`attach_weak_observer`]: `crate::orpc::oqueue::OQueueBase::attach_weak_observer`
    fn project(&self) -> Self::Projected;
}

/// Managed references to [ORPC objects](`OrpcObject`) are projected as the [objects ID](`ObjectId`).
impl<T: OrpcObject> DefaultProjection for Arc<T> {
    type Projected = ObjectId;
    
    fn project(&self) -> Self::Projected {
        self.id()
    }
}
