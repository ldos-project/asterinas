// SPDX-License-Identifier: MPL-2.0

#![cfg(not(baseline_asterinas))]

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicUsize, Ordering};

use aster_block::BlockDevice;
use aster_block::bio::{BlockDeviceCompletionStats, SubmittedBio};
use ostd::orpc::legacy_oqueue::WeakObserver;
use ostd::sync::Mutex;
use ostd::{Error, orpc::orpc_server};

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
    fn select_block_device(&self, submitted: &SubmittedBio) -> Result<Arc<dyn BlockDevice>, Error> {
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
    fn select_block_device(&self, submitted: &SubmittedBio) -> Result<Arc<dyn BlockDevice>, Error> {
        let idx = self.read_cursor.fetch_add(1, Ordering::Relaxed);
        Ok(self.members[idx % self.members.len()].clone())
    }
}

#[orpc_server]
pub struct LinnOSPolicy {
    read_cursor: AtomicUsize,

    /// Member block devices that support I/O performance tracing.
    members: Vec<Arc<dyn ObservableBlockDevice>>,

    // List of observers attached to the list of member block devices.
    observers: Vec<Mutex<Box<dyn WeakObserver<BlockDeviceCompletionStats>>>>,

    // This is a placeholder for the machine learning model.
    // There is one model per device. Each model contains three layers, an input layer,
    // a hidden layer with 256 neurons, and an output layer with 2 nuerons for the binary
    // classification (fast/slow). Thus, there are two matrices per device, a 31*256 matrix
    // for the hidden layer and a 256*2 matrix for the output layer.
    //
    // Each latency number is decomposed into 4 digits, and each number of outstanding
    // request number is decomposed to 3 digits. Thus, the total number of input features
    // is 3+4*(3+4) = 31. The number of history is R=4.
    //
    // Number of Outstanding Requests: number of pending 4KB pages.
    hidden_layers: Vec<[[f32; 256]; 31]>,
    output_layers: Vec<[[f32; 2]; 256]>,
}

impl core::fmt::Debug for LinnOSPolicy {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LinnOSPolicy")
            .field("read_cursor", &self.read_cursor)
            .field("members", &self.members)
            .field("observers", &format_args!("[{} observers]", self.observers.len()))
            .finish()
    }
}

impl LinnOSPolicy {
    pub fn new(members: Vec<Arc<dyn ObservableBlockDevice>>) -> Result<Arc<Self>, Error> {
        use crate::linnos_weights::{HIDDEN_WEIGHTS, OUTPUT_WEIGHTS};

        let num_devices = members.len();

        // Copy hardcoded weights into Vecs, one entry per device
        let hidden_layers: Vec<[[f32; 256]; 31]> = (0..num_devices)
            .map(|i| *HIDDEN_WEIGHTS[i])
            .collect();
        let output_layers: Vec<[[f32; 2]; 256]> = (0..num_devices)
            .map(|i| *OUTPUT_WEIGHTS[i])
            .collect();

        // Attach one weak observer per device, each peeking 4 steps in the history.
        // Wrapped in Mutex because WeakObserver is Send but not Sync.
        let observers: Vec<_> = members
            .iter()
            .map(|device| {
                Mutex::new(
                    device
                        .bio_completion_oqueue()
                        .attach_weak_observer()
                        .expect("Failed to attach weak observer to bio_completion_oqueue"),
                )
            })
            .collect();

        let server = Self::new_with(|orpc_internal, _| Self {
            orpc_internal,
            read_cursor: AtomicUsize::new(0),
            members,
            observers,
            hidden_layers,
            output_layers,
        });

        Ok(server)
    }
}


impl SelectionPolicy for LinnOSPolicy {
    // FIXME: Need a holder for the most recent request. 
    fn select_block_device(&self, submitted: &SubmittedBio) -> Result<Arc<dyn BlockDevice>, Error> {
        let num_devices = self.members.len();
        let mut fail_cnt = 0;

        loop {
            let idx = self.read_cursor.fetch_add(1, Ordering::Relaxed);
            let device_idx = idx % num_devices;
            let observer = self.observers[device_idx].lock();
            let completion_trace = observer.weak_observe_recent(4); // observe 4 steps in the history

            // Build the 31-element input feature vector:
            //   [0..3]:  current outstanding requests (3 digits, from most recent trace)
            //   For each history step i (0..4):
            //     [3+i*7 .. 3+i*7+3]: outstanding requests (3 digits)
            //     [3+i*7+3 .. 3+i*7+7]: latency in microseconds (4 digits)
            let mut input = [0.0f32; 31];

            // Current outstanding requests: use most recent trace entry, decompose into 3 digits
            let current_outstanding = submitted.num_outstanding_requests().unwrap_or(0);
            input[0] = ((current_outstanding / 100) % 10) as f32;
            input[1] = ((current_outstanding / 10) % 10) as f32;
            input[2] = (current_outstanding % 10) as f32;
            
            // Feature Engineering in LinnOS: Decompose numbers into digits.
            // Historical features: 4 steps, each with 3 digits outstanding + 4 digits latency
            for i in 0..4 {
                let outstanding = completion_trace[i].outstanding_requests;
                let latency_us = completion_trace[i].latency.as_micros() as usize;
                let base = 3 + i * 7;

                // Outstanding requests -> 3 digits (hundreds, tens, ones)
                input[base] = ((outstanding / 100) % 10) as f32;
                input[base + 1] = ((outstanding / 10) % 10) as f32;
                input[base + 2] = (outstanding % 10) as f32;

                // Latency in microseconds -> 4 digits (thousands, hundreds, tens, ones)
                input[base + 3] = ((latency_us / 1000) % 10) as f32;
                input[base + 4] = ((latency_us / 100) % 10) as f32;
                input[base + 5] = ((latency_us / 10) % 10) as f32;
                input[base + 6] = (latency_us % 10) as f32;
            }

            // Hidden layer: input (31) x hidden_weights (31x256) -> hidden_out (256)
            let hidden_weights = &self.hidden_layers[device_idx];
            let mut hidden_out = [0.0f32; 256];
            for j in 0..256 {
                let mut sum = 0.0f32;
                for i in 0..31 {
                    sum += input[i] * hidden_weights[i][j];
                }
                // ReLU activation
                hidden_out[j] = if sum > 0.0 { sum } else { 0.0 };
            }

            // Output layer: hidden_out (256) x output_weights (256x2) -> output (2)
            let output_weights = &self.output_layers[device_idx];
            let mut output = [0.0f32; 2];
            for k in 0..2 {
                for j in 0..256 {
                    output[k] += hidden_out[j] * output_weights[j][k];
                }
            }

            // Argmax: output[0] > output[1] means fast, otherwise slow
            if output[0] > output[1] {
                return Ok(self.members[device_idx].clone());
            }

            fail_cnt += 1;
            // All devices predicted slow -- fall back to round-robin
            if fail_cnt >= num_devices {
                let fallback_idx = self.read_cursor.fetch_add(1, Ordering::Relaxed) % num_devices;
                return Ok(self.members[fallback_idx].clone());
            }
        }
    }
}
