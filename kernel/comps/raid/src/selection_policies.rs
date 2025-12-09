use ostd::sync::atomic::AtomicUsize;

use aster_block::BlockDevice;
use alloc::{sync::Arc, vec::Vec, boxed::Box};
use ostd::Error;
use ostd::orpc::oqueue::WeakObserver;

pub struct RoundRobinPolicy {
    read_cursor: AtomicUsize,
    members: Vec<Arc<dyn BlockDevice>>,
}

pub impl RoundRobinPolicy {
    pub fn new(members: Vec<Arc<dyn BlockDevice>>) -> Self {
        Self { read_cursor: AtomicUsize::new(0), members }
    }
}

impl SelectionPolicy for RoundRobinPolicy {
    fn select_block_device(&self) -> Result<Arc<dyn BlockDevice>, Error> {
        let idx = self.read_cursor.fetch_add(1, Ordering::Relaxed);
        Ok(self.members[idx % self.members.len()].clone())
    }
}

pub struct LinnOSPolicy {
    read_cursor: AtomicUsize,
    /// Weak observers for each tracer's io_performance_traces OQueue.
    /// These allow observing I/O performance data without blocking producers.
    trace_observers: Vec<Box<dyn WeakObserver<BlockDeviceCompletionTrace>>>,

    members: Vec<Arc<dyn BlockDevice>>,

    // TODO(yingqi): this is a placeholder for the machine learning model.
    // The model is now a fixed-size array of 8 u16s for 8 features, with 2 per trace data (latency and outstanding requests)
    model: [f32; 8],
}

pub impl LinnOSPolicy {
    pub fn new(members: Vec<Arc<dyn BlockDevice>>) -> Self {
        let trace_observers = members
            .iter()
            .map(|device| {
                device
                    .bio_completion_oqueue()
                    .attach_weak_observer()
                    .expect("Failed to attach weak observer to bio_completion_oqueue")
            })
            .collect();

        Self {
            read_cursor: AtomicUsize::new(0),
            trace_observers,
            members,
            model: [0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1],  // TODO(yingqi): get the actual model weights and hard code it here. 
        }
    }

    pub fn select_block_device(&self) -> Result<Arc<dyn BlockDevice>, Error> {
        while true {
            let idx = self.read_cursor.fetch_add(1, Ordering::Relaxed);
            let observer = self.trace_observers[idx % self.trace_observers.len()];
            let completion_trace = observer.weak_observe_recent(4)?;
            // inference
            let x = self.model[0] * completion_trace[0].latency +
                self.model[1] * completion_trace[0].outstanding_requests +
                self.model[2] * completion_trace[1].latency +
                self.model[3] * completion_trace[1].outstanding_requests +
                self.model[4] * completion_trace[2].latency +
                self.model[5] * completion_trace[2].outstanding_requests;

            let prob = 1.0/(1.0 + (-x).exp());

            if prob > 0.5 {
                return Ok(self.members[idx % self.members.len()].clone());
            }
        }
        Err(Error::new("No fast block device found"))
    }
}