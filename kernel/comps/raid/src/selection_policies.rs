// SPDX-License-Identifier: MPL-2.0

#![cfg(not(baseline_asterinas))]

use alloc::{sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicUsize, Ordering};

use aster_block::{
    BlockDevice,
    bio::{BlockDeviceCompletionStats, SubmittedBio},
};
use ostd::{
    Error,
    orpc::orpc_server,
    sync::Mutex,
};

use crate::server_traits::SelectionPolicy;

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
    fn select_block_device(
        &self,
        _submitted: &mut SubmittedBio,
    ) -> Result<Arc<dyn BlockDevice>, Error> {
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
    fn select_block_device(
        &self,
        _submitted: &mut SubmittedBio,
    ) -> Result<Arc<dyn BlockDevice>, Error> {
        let idx = self.read_cursor.fetch_add(1, Ordering::Relaxed);
        Ok(self.members[idx % self.members.len()].clone())
    }
}

/// hidden_layers, hidden_biases, output_layers, output_biases: machine learning model weights.
/// There is one model per device. Each model contains three layers, an input layer,
/// a hidden layer with 256 neurons, and an output layer with 2 neurons for the binary
/// classification (fast/slow). Thus, there are two weight matrices and two bias vectors
/// per device: a 31x256 matrix + 256 bias for the hidden layer and a 256x2 matrix + 2
/// bias for the output layer.
/// Each latency number is decomposed into 4 digits, and each number of outstanding
/// request number is decomposed into 3 digits. Thus, the total number of input features
/// is 3+4*(3+4) = 31. The number of history is R=4.
///
/// Number of Outstanding Requests: number of pending 4KB pages.
/// List of observers attached to the list of member block devices.
#[orpc_server]
pub struct LinnOSPolicy {
    read_cursor: AtomicUsize,
    members: Vec<Arc<dyn BlockDevice>>,
    observers: Vec<Mutex<ostd::orpc::oqueue::WeakObserver<BlockDeviceCompletionStats>>>,
    hidden_layers: Vec<[[f32; 256]; 31]>,
    hidden_biases: Vec<[f32; 256]>,
    output_layers: Vec<[[f32; 2]; 256]>,
    output_biases: Vec<[f32; 2]>,
}

impl core::fmt::Debug for LinnOSPolicy {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LinnOSPolicy")
            .field("read_cursor", &self.read_cursor)
            .field("members", &self.members)
            .field(
                "observers",
                &format_args!("[{} observers]", self.observers.len()),
            )
            .finish()
    }
}

impl LinnOSPolicy {
    pub fn new(
        members: Vec<Arc<dyn BlockDevice>>,
        observers: Vec<Mutex<ostd::orpc::oqueue::WeakObserver<BlockDeviceCompletionStats>>>,
    ) -> Result<Arc<Self>, Error> {
        use crate::linnos_weights::{HIDDEN_BIASES, HIDDEN_WEIGHTS, OUTPUT_BIASES, OUTPUT_WEIGHTS};

        let num_devices = members.len();

        // Copy hardcoded weights into Vecs, one entry per device
        let hidden_layers: Vec<[[f32; 256]; 31]> =
            (0..num_devices).map(|i| *HIDDEN_WEIGHTS[i]).collect();
        let hidden_biases: Vec<[f32; 256]> =
            (0..num_devices).map(|i| *HIDDEN_BIASES[i]).collect();
        let output_layers: Vec<[[f32; 2]; 256]> =
            (0..num_devices).map(|i| *OUTPUT_WEIGHTS[i]).collect();
        let output_biases: Vec<[f32; 2]> =
            (0..num_devices).map(|i| *OUTPUT_BIASES[i]).collect();

        let server = Self::new_with(|orpc_internal, _| Self {
            orpc_internal,
            read_cursor: AtomicUsize::new(0),
            members,
            observers,
            hidden_layers,
            hidden_biases,
            output_layers,
            output_biases,
        });

        Ok(server)
    }
}

impl SelectionPolicy for LinnOSPolicy {
    fn select_block_device(&self, submitted: &mut SubmittedBio) -> Result<Arc<dyn BlockDevice>, Error> {
        let num_devices = self.members.len();
        let mut fail_cnt = 0;
        let num_pages = submitted.num_pages();

        loop {
            let idx = self.read_cursor.fetch_add(1, Ordering::Relaxed);
            let device_idx = idx % num_devices;
            let observer = self.observers[device_idx].lock();
            let completion_trace = observer
                .weak_observe_recent(4)
                .expect("Failed to observe completion trace");

            // Build the 31-element input feature vector:
            //   [0..3]:  current outstanding pages (3 digits, from most recent trace)
            //   For each history step i (0..4):
            //     [3+i*7 .. 3+i*7+3]: outstanding pages (3 digits)
            //     [3+i*7+3 .. 3+i*7+7]: latency in microseconds (4 digits)
            let mut input = [0.0f32; 31];

            // Current outstanding pages: use most recent trace entry, decompose into 3 digits
            let current_outstanding = num_pages as usize + self.members[device_idx].num_outstanding_pages() as usize;
            input[0] = ((current_outstanding / 100) % 10) as f32;
            input[1] = ((current_outstanding / 10) % 10) as f32;
            input[2] = (current_outstanding % 10) as f32;

            // Feature Engineering in LinnOS: Decompose numbers into digits.
            // Historical features: 4 steps, each with 3 digits outstanding + 4 digits latency
            let mut observed: [(usize, usize); 4] = [(0, 0); 4];
            for (i, trace_entry) in completion_trace.iter().enumerate().take(4) {
                let Some(trace_entry) = trace_entry else {
                    continue;
                };
                let outstanding = trace_entry.outstanding_pages as usize;
                let latency_us = trace_entry.latency_us as usize;
                let base = 3 + i * 7;

                observed[i] = (outstanding, latency_us);

                // Outstanding pages -> 3 digits (hundreds, tens, ones)
                input[base] = ((outstanding / 100) % 10) as f32;
                input[base + 1] = ((outstanding / 10) % 10) as f32;
                input[base + 2] = (outstanding % 10) as f32;

                // Latency in microseconds -> 4 digits (thousands, hundreds, tens, ones)
                input[base + 3] = ((latency_us / 1000) % 10) as f32;
                input[base + 4] = ((latency_us / 100) % 10) as f32;
                input[base + 5] = ((latency_us / 10) % 10) as f32;
                input[base + 6] = (latency_us % 10) as f32;
            }

            // log::info!(
            //     "LinnOS dev={} cur_outstanding={} outstanding=[{},{},{},{}] latency_us=[{},{},{},{}]",
            //     device_idx, current_outstanding,
            //     observed[0].0, observed[1].0, observed[2].0, observed[3].0,
            //     observed[0].1, observed[1].1, observed[2].1, observed[3].1,
            // );

            // Hidden layer: input (31) x hidden_weights (31x256) + bias (256) -> hidden_out (256)
            let hidden_weights = &self.hidden_layers[device_idx];
            let hidden_bias = &self.hidden_biases[device_idx];
            let mut hidden_out = [0.0f32; 256];
            for j in 0..256 {
                let mut sum = hidden_bias[j];
                for i in 0..31 {
                    sum += input[i] * hidden_weights[i][j];
                }
                // ReLU activation
                hidden_out[j] = if sum > 0.0 { sum } else { 0.0 };
            }

            // Output layer: hidden_out (256) x output_weights (256x2) + bias (2) -> output (2)
            let output_weights = &self.output_layers[device_idx];
            let output_bias = &self.output_biases[device_idx];
            let mut output = [output_bias[0], output_bias[1]];
            for k in 0..2 {
                for j in 0..256 {
                    output[k] += hidden_out[j] * output_weights[j][k];
                }
            }

            // Argmax: output[0] < output[1] means fast, otherwise slow
            if output[0] < output[1] {
                // log::info!("Submitting to device {} predicted FAST. output=[{:.4},{:.4}]", device_idx, output[0], output[1]);
                return Ok(self.members[device_idx].clone());
            }

            fail_cnt += 1;
            // All devices predicted slow -- fall back to round-robin
            if fail_cnt >= num_devices {
                let fallback_idx = self.read_cursor.fetch_add(1, Ordering::Relaxed) % num_devices;
                // log::info!("Submitting to device {} as all devices are busy. output=[{:.4},{:.4}]", fallback_idx, output[0], output[1]);
                return Ok(self.members[fallback_idx].clone());
            }
        }
    }
}

/// Decision tree selection policy.
///
/// Uses a per-device binary decision tree trained on the same 31-element LinnOS
/// feature vector (3 digits current outstanding + 4 history steps × 7 digits).
/// The prediction functions are generated by `export_dt.py --format rust` and
/// live in `decision_tree_predictions`. Each function takes `&[u8; 31]` (one
/// digit per feature, 0–9) and returns 0 (slow) or 1 (fast).
///
/// Looping and fallback logic mirrors LinnOS exactly.
#[orpc_server]
pub struct DecisionTreePolicy {
    read_cursor: AtomicUsize,
    members: Vec<Arc<dyn BlockDevice>>,
    observers: Vec<Mutex<ostd::orpc::oqueue::WeakObserver<BlockDeviceCompletionStats>>>,
}

impl core::fmt::Debug for DecisionTreePolicy {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DecisionTreePolicy")
            .field("read_cursor", &self.read_cursor)
            .field("members", &self.members)
            .field(
                "observers",
                &format_args!("[{} observers]", self.observers.len()),
            )
            .finish()
    }
}

impl DecisionTreePolicy {
    pub fn new(
        members: Vec<Arc<dyn BlockDevice>>,
        observers: Vec<Mutex<ostd::orpc::oqueue::WeakObserver<BlockDeviceCompletionStats>>>,
    ) -> Result<Arc<Self>, Error> {
        let server = Self::new_with(|orpc_internal, _| Self {
            orpc_internal,
            read_cursor: AtomicUsize::new(0),
            members,
            observers,
        });
        Ok(server)
    }
}

impl SelectionPolicy for DecisionTreePolicy {
    fn select_block_device(&self, submitted: &mut SubmittedBio) -> Result<Arc<dyn BlockDevice>, Error> {
        use crate::decision_tree_predictions::{predict_device0, predict_device1, predict_device2};

        let num_devices = self.members.len();
        let mut fail_cnt = 0;
        let num_pages = submitted.num_pages();

        loop {
            let idx = self.read_cursor.fetch_add(1, Ordering::Relaxed);
            let device_idx = idx % num_devices;
            let observer = self.observers[device_idx].lock();
            let completion_trace = observer
                .weak_observe_recent(4)
                .expect("Failed to observe completion trace");

            // Build the 31-element input feature vector as u8 digits (0–9)
            let mut input = [0u8; 31];

            let current_outstanding = num_pages as usize
                + self.members[device_idx].num_outstanding_pages() as usize;
            input[0] = ((current_outstanding / 100) % 10) as u8;
            input[1] = ((current_outstanding / 10) % 10) as u8;
            input[2] = (current_outstanding % 10) as u8;

            for (i, trace_entry) in completion_trace.iter().enumerate().take(4) {
                let Some(trace_entry) = trace_entry else {
                    continue;
                };
                let outstanding = trace_entry.outstanding_pages as usize;
                let latency_us = trace_entry.latency_us as usize;
                let base = 3 + i * 7;

                input[base]     = ((outstanding / 100) % 10) as u8;
                input[base + 1] = ((outstanding / 10) % 10) as u8;
                input[base + 2] = (outstanding % 10) as u8;

                input[base + 3] = ((latency_us / 1000) % 10) as u8;
                input[base + 4] = ((latency_us / 100) % 10) as u8;
                input[base + 5] = ((latency_us / 10) % 10) as u8;
                input[base + 6] = (latency_us % 10) as u8;
            }

            let prediction = match device_idx {
                0 => predict_device0(&input),
                1 => predict_device1(&input),
                2 => predict_device2(&input),
                _ => 1, // unknown device: predict fast
            };

            if prediction == 1 {
                return Ok(self.members[device_idx].clone());
            }

            fail_cnt += 1;
            if fail_cnt >= num_devices {
                let fallback_idx = self.read_cursor.fetch_add(1, Ordering::Relaxed) % num_devices;
                return Ok(self.members[fallback_idx].clone());
            }
        }
    }
}

/// LinnOSPlus: a deeper variant of the LinnOS neural-network selection policy.
///
/// Architecture (per device):
///   Linear(31, 8) -> ReLU -> Linear(8, 8) -> ReLU -> Linear(8, 2)
///
/// The input feature vector is identical to LinnOS (31 elements).
/// Weights are loaded from `linnos_plus_weights`.
#[orpc_server]
pub struct LinnOSPlusPolicy {
    read_cursor: AtomicUsize,
    members: Vec<Arc<dyn BlockDevice>>,
    observers: Vec<Mutex<ostd::orpc::oqueue::WeakObserver<BlockDeviceCompletionStats>>>,
    hidden1_weights: Vec<[[f32; 8]; 31]>,
    hidden1_biases: Vec<[f32; 8]>,
    hidden2_weights: Vec<[[f32; 8]; 8]>,
    hidden2_biases: Vec<[f32; 8]>,
    output_weights: Vec<[[f32; 2]; 8]>,
    output_biases: Vec<[f32; 2]>,
}

impl core::fmt::Debug for LinnOSPlusPolicy {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LinnOSPlusPolicy")
            .field("read_cursor", &self.read_cursor)
            .field("members", &self.members)
            .field(
                "observers",
                &format_args!("[{} observers]", self.observers.len()),
            )
            .finish()
    }
}

impl LinnOSPlusPolicy {
    pub fn new(
        members: Vec<Arc<dyn BlockDevice>>,
        observers: Vec<Mutex<ostd::orpc::oqueue::WeakObserver<BlockDeviceCompletionStats>>>,
    ) -> Result<Arc<Self>, Error> {
        use crate::linnos_plus_weights::{
            HIDDEN1_BIASES, HIDDEN1_WEIGHTS, HIDDEN2_BIASES, HIDDEN2_WEIGHTS, OUTPUT_BIASES,
            OUTPUT_WEIGHTS,
        };

        let num_devices = members.len();

        let hidden1_weights: Vec<[[f32; 8]; 31]> =
            (0..num_devices).map(|i| *HIDDEN1_WEIGHTS[i]).collect();
        let hidden1_biases: Vec<[f32; 8]> =
            (0..num_devices).map(|i| *HIDDEN1_BIASES[i]).collect();
        let hidden2_weights: Vec<[[f32; 8]; 8]> =
            (0..num_devices).map(|i| *HIDDEN2_WEIGHTS[i]).collect();
        let hidden2_biases: Vec<[f32; 8]> =
            (0..num_devices).map(|i| *HIDDEN2_BIASES[i]).collect();
        let output_weights: Vec<[[f32; 2]; 8]> =
            (0..num_devices).map(|i| *OUTPUT_WEIGHTS[i]).collect();
        let output_biases: Vec<[f32; 2]> =
            (0..num_devices).map(|i| *OUTPUT_BIASES[i]).collect();

        let server = Self::new_with(|orpc_internal, _| Self {
            orpc_internal,
            read_cursor: AtomicUsize::new(0),
            members,
            observers,
            hidden1_weights,
            hidden1_biases,
            hidden2_weights,
            hidden2_biases,
            output_weights,
            output_biases,
        });

        Ok(server)
    }
}

impl SelectionPolicy for LinnOSPlusPolicy {
    fn select_block_device(&self, submitted: &mut SubmittedBio) -> Result<Arc<dyn BlockDevice>, Error> {
        let num_devices = self.members.len();
        let mut fail_cnt = 0;
        let num_pages = submitted.num_pages();

        loop {
            let idx = self.read_cursor.fetch_add(1, Ordering::Relaxed);
            let device_idx = idx % num_devices;
            let observer = self.observers[device_idx].lock();
            let completion_trace = observer
                .weak_observe_recent(4)
                .expect("Failed to observe completion trace");

            // Build the 31-element input feature vector (same as LinnOS)
            let mut input = [0.0f32; 31];

            let current_outstanding = num_pages as usize + self.members[device_idx].num_outstanding_pages() as usize;
            input[0] = ((current_outstanding / 100) % 10) as f32;
            input[1] = ((current_outstanding / 10) % 10) as f32;
            input[2] = (current_outstanding % 10) as f32;

            for (i, trace_entry) in completion_trace.iter().enumerate().take(4) {
                let Some(trace_entry) = trace_entry else {
                    continue;
                };
                let outstanding = trace_entry.outstanding_pages as usize;
                let latency_us = trace_entry.latency_us as usize;
                let base = 3 + i * 7;

                input[base] = ((outstanding / 100) % 10) as f32;
                input[base + 1] = ((outstanding / 10) % 10) as f32;
                input[base + 2] = (outstanding % 10) as f32;

                input[base + 3] = ((latency_us / 1000) % 10) as f32;
                input[base + 4] = ((latency_us / 100) % 10) as f32;
                input[base + 5] = ((latency_us / 10) % 10) as f32;
                input[base + 6] = (latency_us % 10) as f32;
            }

            // Hidden layer 1: input (31) x hidden1_weights (31x8) + bias (8) -> hidden1_out (8)
            let h1_weights = &self.hidden1_weights[device_idx];
            let h1_bias = &self.hidden1_biases[device_idx];
            let mut hidden1_out = [0.0f32; 8];
            for j in 0..8 {
                let mut sum = h1_bias[j];
                for i in 0..31 {
                    sum += input[i] * h1_weights[i][j];
                }
                hidden1_out[j] = if sum > 0.0 { sum } else { 0.0 };
            }

            // Hidden layer 2: hidden1_out (8) x hidden2_weights (8x8) + bias (8) -> hidden2_out (8)
            let h2_weights = &self.hidden2_weights[device_idx];
            let h2_bias = &self.hidden2_biases[device_idx];
            let mut hidden2_out = [0.0f32; 8];
            for j in 0..8 {
                let mut sum = h2_bias[j];
                for i in 0..8 {
                    sum += hidden1_out[i] * h2_weights[i][j];
                }
                hidden2_out[j] = if sum > 0.0 { sum } else { 0.0 };
            }

            // Output layer: hidden2_out (8) x output_weights (8x2) + bias (2) -> output (2)
            let out_weights = &self.output_weights[device_idx];
            let out_bias = &self.output_biases[device_idx];
            let mut output = [out_bias[0], out_bias[1]];
            for k in 0..2 {
                for j in 0..8 {
                    output[k] += hidden2_out[j] * out_weights[j][k];
                }
            }

            // Argmax: output[0] < output[1] means fast, otherwise slow
            if output[0] < output[1] {
                // log::info!("LinnOSPlus: device {} predicted FAST. output=[{:.4},{:.4}]", device_idx, output[0], output[1]);
                return Ok(self.members[device_idx].clone());
            }

            fail_cnt += 1;
            if fail_cnt >= num_devices {
                let fallback_idx = self.read_cursor.fetch_add(1, Ordering::Relaxed) % num_devices;
                // log::info!("LinnOSPlus: device {} fallback (all busy). output=[{:.4},{:.4}]", fallback_idx, output[0], output[1]);
                return Ok(self.members[fallback_idx].clone());
            }
        }
    }
}
