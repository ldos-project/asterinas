// SPDX-License-Identifier: MPL-2.0

use alloc::sync::Arc;
use core::time::Duration;

use aster_time::read_monotonic_time;
use ostd::task::{
    scheduler::{EnqueueFlags, UpdateFlags},
    Task,
};

use super::{CurrentRuntime, SchedAttr, SchedClassRq};

/// The per-cpu run queue for the IDLE scheduling class.
///
/// This run queue is used for the per-cpu idle entity, if any.
pub(super) struct IdleClassRq {
    entity: Option<Arc<Task>>,
    idle_time: Duration,
    last_start_time: Option<Duration>,
}

impl IdleClassRq {
    pub fn new() -> Self {
        Self {
            entity: None,
            idle_time: Duration::from_nanos(0),
            last_start_time: None,
        }
    }

    pub fn get_idle_time(&self) -> Duration {
        self.idle_time
    }
}

impl core::fmt::Debug for IdleClassRq {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.entity.is_some() {
            write!(f, "Idle: occupied")?;
        } else {
            write!(f, "Idle: empty")?;
        }
        Ok(())
    }
}

impl SchedClassRq for IdleClassRq {
    fn enqueue(&mut self, entity: Arc<Task>, _: Option<EnqueueFlags>) {
        self.idle_time += self
            .last_start_time
            .map(|t| read_monotonic_time() - t)
            .unwrap_or(Duration::from_nanos(0));

        let old = self.entity.replace(entity);
        debug_assert!(
            old.is_none(),
            "the length of the idle queue should be no larger than 1"
        );
    }

    fn len(&self) -> usize {
        usize::from(!self.is_empty())
    }

    fn is_empty(&self) -> bool {
        self.entity.is_none()
    }

    fn pick_next(&mut self) -> Option<Arc<Task>> {
        self.last_start_time = Some(read_monotonic_time());
        self.entity.take()
    }

    fn update_current(&mut self, _: &CurrentRuntime, _: &SchedAttr, _flags: UpdateFlags) -> bool {
        // Idle entities has the greatest priority value. They should always be preempted.
        true
    }
}
