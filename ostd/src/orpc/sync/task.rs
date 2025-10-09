//! An implementation of kernel-thread-like task operations. It attempts to emulate the behavior of OS tasks, but will
//! be slow and not show the same race conditions. It is definitely not appropriate for fuzzing, but may be useful for
//! deterministic testing.

use crate::prelude::Arc;

/// A reference to a Task which can be passed around. This is given a separate name to make porting code to a
/// non-reference counted form easier if that is required and to make it clear what the canonical way to reference a
/// task is.
pub type TaskRef = Arc<crate::task::Task>;
