// SPDX-License-Identifier: MPL-2.0

#![cfg(not(baseline_asterinas))]

use alloc::{sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicUsize, Ordering};

use aster_block::BlockDevice;
use ostd::{
    Error,
    orpc::{oqueue::ObservationQuery, orpc_server},
};

use crate::server_traits::{ObservableBlockDevice, SelectionPolicy};

#[derive(Debug)]
#[orpc_server]
pub struct Dummy0Policy {
    members: Vec<Arc<dyn BlockDevice>>,
}

impl Dummy0Policy {
    pub fn new(members: Vec<Arc<dyn BlockDevice>>) -> Result<Arc<Self>, Error> {
        let server = Self::new_with(|orpc_internal, _| Self {
            orpc_internal,
            members,
        });
        Ok(server)
    }
}

impl SelectionPolicy for Dummy0Policy {
    fn select_block_device(&self) -> Result<Arc<dyn BlockDevice>, Error> {
        Ok(self.members[0].clone())
    }
}


#[derive(Debug)]
#[orpc_server]
pub struct RoundRobinPolicy {
    read_cursor: AtomicUsize,
    members: Vec<Arc<dyn BlockDevice>>,
}

impl RoundRobinPolicy {
    pub fn new(members: Vec<Arc<dyn BlockDevice>>) -> Result<Arc<Self>, Error> {
        let server = Self::new_with(|orpc_internal, _| Self {
            orpc_internal,
            read_cursor: AtomicUsize::new(0),
            members,
        });
        Ok(server)
    }
}

impl SelectionPolicy for RoundRobinPolicy {
    fn select_block_device(&self) -> Result<Arc<dyn BlockDevice>, Error> {
        let idx = self.read_cursor.fetch_add(1, Ordering::Relaxed);
        Ok(self.members[idx % self.members.len()].clone())
    }
}

#[derive(Debug)]
#[orpc_server]
pub struct LinnOSPolicy {
    read_cursor: AtomicUsize,

    /// Member block devices that support I/O performance tracing.
    members: Vec<Arc<dyn ObservableBlockDevice>>,

    // TODO(yingqi): this is a placeholder for the machine learning model.
    // The model is now a fixed-size array of 8 f32s for 8 features, with 2 per trace data (latency and outstanding requests)
    model: [f32; 8],
}

impl LinnOSPolicy {
    pub fn new(members: Vec<Arc<dyn ObservableBlockDevice>>) -> Result<Arc<Self>, Error> {
        let server = Self::new_with(|orpc_internal, _| Self {
            orpc_internal,
            read_cursor: AtomicUsize::new(0),
            members,
            model: [0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1], // TODO(yingqi): get the actual model weights and hard code it here.
        });

        Ok(server)
    }
}

impl SelectionPolicy for LinnOSPolicy {
    fn select_block_device(&self) -> Result<Arc<dyn BlockDevice>, Error> {
        // // Attach weak observers to each member's completion queue
        // let trace_observers: Vec<_> = self
        //     .members
        //     .iter()
        //     .map(|device| {
        //         device
        //             .bio_completion_oqueue()
        //             .attach_weak_observer(4, ObservationQuery::identity())
        //             .expect("Failed to attach weak observer to bio_completion_oqueue")
        //     })
        //     .collect();

        // loop {
        //     let idx = self.read_cursor.fetch_add(1, Ordering::Relaxed);
        //     let observer = &trace_observers[idx % trace_observers.len()];
        //     let completion_trace = observer
        //         .weak_observe_recent(4)
        //         .expect("Failed to observe recent completions");
        //     let default = Default::default();
        //     let get = |i: usize| completion_trace[i].as_ref().unwrap_or(&default);

        //     // Inference using the ML model
        //     let x = self.model[0] * get(0).latency_us as f32
        //         + self.model[1] * get(0).outstanding_requests as f32
        //         + self.model[2] * get(1).latency_us as f32
        //         + self.model[3] * get(1).outstanding_requests as f32
        //         + self.model[4] * get(2).latency_us as f32
        //         + self.model[5] * get(2).outstanding_requests as f32
        //         + self.model[6] * get(3).latency_us as f32
        //         + self.model[7] * get(3).outstanding_requests as f32;

        //     // FIXME: There isn't a math library in Asterinas yet, so we cannot use sigmoid.
        //     // let e: f32 = 2.71828;
        //     // let prob = 1.0 / (1.0 + e.powf(-x));

        //     // FIXME: Temporariliy use a threshold to determine the selection.
        //     if x > 2.0 {
        //         return Ok(self.members[idx % self.members.len()].clone());
        //     }
        // }
        return Ok(self.members[0].clone());
    }
}
