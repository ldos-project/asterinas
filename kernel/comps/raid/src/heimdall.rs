// SPDX-License-Identifier: MPL-2.0

#![cfg(not(baseline_asterinas))]

use alloc::{sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicBool, Ordering};

use aster_block::{
    BlockDevice,
    bio::BlockDeviceCompletionStats,
};
use ostd::{
    Error,
    orpc::oqueue::{OQueueError, StrongObserver},
    sync::Mutex,
};

/// Heimdall: an asynchronous device performance monitor for RAID-1 arrays.
///
/// Heimdall maintains one ML model and one strong observer per member device.
/// A dedicated background thread continuously drains completion stats from each
/// device's OQueue.  Every `BATCH_SIZE` (16) completions, it runs an ML inference
/// to update that device's fast/slow indicator.
///
/// Model architecture (per device):
///   Linear(INPUT_DIM, 128) -> ReLU -> Linear(128, 16) -> ReLU -> Linear(16, 1) -> Sigmoid
///
/// Selection policies can query `is_device_fast(idx)` to incorporate Heimdall's
/// classification into their scheduling decisions.
pub struct Heimdall {
    members: Vec<Arc<dyn BlockDevice>>,
    observers: Vec<Mutex<StrongObserver<BlockDeviceCompletionStats>>>,
    /// Per-device fast/slow indicator. `true` means the device is predicted fast.
    fast_indicators: Vec<AtomicBool>,
    /// Per-device fc1 weights: [INPUT_DIM][HIDDEN1_SIZE]
    fc1_weights: Vec<[[f32; HIDDEN1_SIZE]; INPUT_DIM]>,
    /// Per-device fc1 biases: [HIDDEN1_SIZE]
    fc1_biases: Vec<[f32; HIDDEN1_SIZE]>,
    /// Per-device fc2 weights: [HIDDEN1_SIZE][HIDDEN2_SIZE]
    fc2_weights: Vec<[[f32; HIDDEN2_SIZE]; HIDDEN1_SIZE]>,
    /// Per-device fc2 biases: [HIDDEN2_SIZE]
    fc2_biases: Vec<[f32; HIDDEN2_SIZE]>,
    /// Per-device fc3 weights: [HIDDEN2_SIZE]
    fc3_weights: Vec<[f32; HIDDEN2_SIZE]>,
    /// Per-device fc3 biases: scalar
    fc3_biases: Vec<f32>,
}

impl core::fmt::Debug for Heimdall {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Heimdall")
            .field("members", &self.members)
            .field(
                "observers",
                &format_args!("[{} observers]", self.observers.len()),
            )
            .field("fast_indicators", &self.fast_indicators)
            .finish()
    }
}

use crate::heimdall_weights::{HIDDEN1_SIZE, HIDDEN2_SIZE, INPUT_DIM};

/// Number of completion records to drain before running an inference.
const BATCH_SIZE: usize = 16;

/// Inference timeout in milliseconds. If this duration elapses since the last
/// inference for a device, inference is triggered even if fewer than `BATCH_SIZE`
/// records have been observed.
const INFERENCE_TIMEOUT_MS: u64 = 5;

impl Heimdall {
    /// Creates a new Heimdall monitor.
    ///
    /// `members` — the RAID-1 member devices to monitor.
    /// `observers` — one strong observer per member, attached to its bio completion OQueue.
    pub fn new(
        members: Vec<Arc<dyn BlockDevice>>,
        observers: Vec<StrongObserver<BlockDeviceCompletionStats>>,
    ) -> Result<Arc<Self>, Error> {
        use crate::heimdall_weights::{
            FC1_BIASES, FC1_WEIGHTS, FC2_BIASES, FC2_WEIGHTS, FC3_BIASES, FC3_WEIGHTS,
        };

        let num_devices = members.len();

        let fast_indicators: Vec<AtomicBool> = (0..num_devices)
            .map(|_| AtomicBool::new(true)) // optimistic: assume fast initially
            .collect();

        let observers: Vec<Mutex<StrongObserver<BlockDeviceCompletionStats>>> = observers
            .into_iter()
            .map(Mutex::new)
            .collect();

        let fc1_weights: Vec<_> = (0..num_devices).map(|i| *FC1_WEIGHTS[i]).collect();
        let fc1_biases: Vec<_> = (0..num_devices).map(|i| *FC1_BIASES[i]).collect();
        let fc2_weights: Vec<_> = (0..num_devices).map(|i| *FC2_WEIGHTS[i]).collect();
        let fc2_biases: Vec<_> = (0..num_devices).map(|i| *FC2_BIASES[i]).collect();
        let fc3_weights: Vec<_> = (0..num_devices).map(|i| *FC3_WEIGHTS[i]).collect();
        let fc3_biases: Vec<_> = (0..num_devices).map(|i| FC3_BIASES[i]).collect();

        log::info!(
            "Heimdall created with {} devices",
            fast_indicators.len()
        );

        Ok(Arc::new(Self {
            members,
            observers,
            fast_indicators,
            fc1_weights,
            fc1_biases,
            fc2_weights,
            fc2_biases,
            fc3_weights,
            fc3_biases,
        }))
    }

    /// Returns whether device `idx` is currently classified as fast.
    pub fn is_device_fast(&self, idx: usize) -> bool {
        self.fast_indicators[idx].load(Ordering::Relaxed)
    }

    /// The number of member devices being monitored.
    pub fn num_devices(&self) -> usize {
        self.members.len()
    }

    /// Main monitoring loop. This should be spawned on a dedicated thread.
    ///
    /// For each device, drains completion records from its strong observer.
    /// Inference is triggered for a device when either condition is met first:
    ///   1. `BATCH_SIZE` (16) records have been drained, or
    ///   2. `INFERENCE_TIMEOUT_MS` (5 ms) have elapsed since the last inference.
    pub fn run(&self) {
        use ostd::timer::Jiffies;

        let num_devices = self.members.len();
        // TIMER_FREQ is 1000 Hz, so 1 jiffy = 1 ms. 5 ms = 5 jiffies.
        let timeout_jiffies = INFERENCE_TIMEOUT_MS * ostd::arch::timer::TIMER_FREQ / 1000;

        // Per-device batch buffers for accumulating stats between inferences.
        let mut batch_buffers: Vec<Vec<BlockDeviceCompletionStats>> = (0..num_devices)
            .map(|_| Vec::with_capacity(BATCH_SIZE))
            .collect();

        // Per-device jiffies timestamp of the last inference (or loop start).
        let now = Jiffies::elapsed().as_u64();
        let mut last_inference_jiffies = alloc::vec![now; num_devices];

        loop {
            for device_idx in 0..num_devices {
                let observer = self.observers[device_idx].lock();

                // Drain all available records (non-blocking).
                loop {
                    match observer.try_strong_observe() {
                        Ok(Some(stats)) => {
                            batch_buffers[device_idx].push(stats);

                            // Condition 1: batch is full.
                            // Do device inference, then break to give other devices a turn.
                            if batch_buffers[device_idx].len() >= BATCH_SIZE {
                                self.run_inference(device_idx, &mut batch_buffers[device_idx]);
                                last_inference_jiffies[device_idx] = Jiffies::elapsed().as_u64();
                                break;
                            }
                        }
                        Ok(None) => break,  // queue is empty, move on to timeout check
                        Err(OQueueError::Detached { .. }) => {
                            log::warn!(
                                "Heimdall: observer for device {} detached",
                                device_idx
                            );
                            break;
                        }
                        Err(e) => {
                            log::warn!(
                                "Heimdall: error observing device {}: {:?}",
                                device_idx, e
                            );
                            break;
                        }
                    }
                }

                drop(observer);

                // Condition 2: timeout elapsed and there is at least some data
                // (or even no data — we still re-evaluate so the device can
                // transition back to fast when IO pressure drops).
                let elapsed = Jiffies::elapsed().as_u64().wrapping_sub(last_inference_jiffies[device_idx]);
                if elapsed >= timeout_jiffies && !batch_buffers[device_idx].is_empty() {
                    self.run_inference(device_idx, &mut batch_buffers[device_idx]);
                    last_inference_jiffies[device_idx] = Jiffies::elapsed().as_u64();
                }
            }

            // Yield to avoid busy-spinning when all queues are empty.
            ostd::task::Task::yield_now();
        }
    }

    /// Run inference for a single device and update its fast indicator.
    fn run_inference(
        &self,
        device_idx: usize,
        batch: &mut Vec<BlockDeviceCompletionStats>,
    ) {
        // Model output: 1 → slow (reject IO), 0 → fast (accept IO).
        let is_slow = self.infer_device_speed(device_idx, batch);
        self.fast_indicators[device_idx].store(!is_slow, Ordering::Relaxed);
        log::info!(
            "Heimdall: labeling device {} to {} (by {} records)",
            device_idx,
            if is_slow { "slow" } else { "fast" },
            batch.len()
        );
        batch.clear();
    }

    /// Build the 11-element input feature vector from a batch of completion stats.
    ///
    /// Features (matching HeimdallNet training input):
    ///   [0]  queue_len_now      — queue_len of the most recent record
    ///   [1]  size_now           — request_size_pages of the most recent record
    ///   [2]  hist_que_len_t-1   — queue_len of the 2nd-most-recent record
    ///   [3]  hist_que_len_t-2   — queue_len of the 3rd-most-recent record
    ///   [4]  hist_que_len_t-3   — queue_len of the 4th-most-recent record
    ///   [5]  hist_lat_t-1       — latency_us of the 2nd-most-recent record
    ///   [6]  hist_lat_t-2       — latency_us of the 3rd-most-recent record
    ///   [7]  hist_lat_t-3       — latency_us of the 4th-most-recent record
    ///   [8]  hist_thpt_t-1      — request_size_pages / latency_us (2nd-most-recent)
    ///   [9]  hist_thpt_t-2      — request_size_pages / latency_us (3rd-most-recent)
    ///   [10] hist_thpt_t-3      — request_size_pages / latency_us (4th-most-recent)
    fn build_features(&self, batch: &[BlockDeviceCompletionStats]) -> [f32; INPUT_DIM] {
        let mut input = [0.0f32; INPUT_DIM];
        let n = batch.len();
        if n == 0 {
            return input;
        }

        // Most recent record: "now" features.
        let now = &batch[n - 1];
        input[0] = now.queue_len as f32;
        input[1] = now.request_size_pages as f32;

        // Historical records: t-1 = batch[n-2], t-2 = batch[n-3], t-3 = batch[n-4].
        // Missing history (batch too small) stays 0.0.
        for hist in 0..3usize {
            let idx = n.wrapping_sub(hist + 2);
            if idx < n {
                let rec = &batch[idx];
                input[2 + hist] = rec.queue_len as f32;
                input[5 + hist] = rec.latency_us as f32;
                input[8 + hist] = if rec.latency_us > 0 {
                    rec.request_size_pages as f32 / rec.latency_us as f32
                } else {
                    0.0
                };
            }
        }

        input
    }

    /// Run the HeimdallNet forward pass for `device_idx` on the given batch.
    ///
    /// Each device has its own model weights.
    /// Architecture:
    ///   Linear(INPUT_DIM, 128) -> ReLU -> Linear(128, 16) -> ReLU -> Linear(16, 1) -> Sigmoid
    ///
    /// Returns `true` if the device is predicted fast (sigmoid output >= 0.5).
    fn infer_device_speed(
        &self,
        device_idx: usize,
        batch: &[BlockDeviceCompletionStats],
    ) -> bool {
        let input = self.build_features(batch);

        // fc1: input (INPUT_DIM) x fc1_weights (INPUT_DIM x HIDDEN1_SIZE) + bias -> ReLU
        let w1 = &self.fc1_weights[device_idx];
        let b1 = &self.fc1_biases[device_idx];
        let mut h1 = [0.0f32; HIDDEN1_SIZE];
        for j in 0..HIDDEN1_SIZE {
            let mut sum = b1[j];
            for i in 0..INPUT_DIM {
                sum += input[i] * w1[i][j];
            }
            h1[j] = if sum > 0.0 { sum } else { 0.0 }; // ReLU
        }

        // fc2: h1 (HIDDEN1_SIZE) x fc2_weights (HIDDEN1_SIZE x HIDDEN2_SIZE) + bias -> ReLU
        let w2 = &self.fc2_weights[device_idx];
        let b2 = &self.fc2_biases[device_idx];
        let mut h2 = [0.0f32; HIDDEN2_SIZE];
        for j in 0..HIDDEN2_SIZE {
            let mut sum = b2[j];
            for i in 0..HIDDEN1_SIZE {
                sum += h1[i] * w2[i][j];
            }
            h2[j] = if sum > 0.0 { sum } else { 0.0 }; // ReLU
        }

        // fc3: h2 (HIDDEN2_SIZE) x fc3_weights (HIDDEN2_SIZE) + bias -> Sigmoid
        let w3 = &self.fc3_weights[device_idx];
        let b3 = self.fc3_biases[device_idx];
        let mut logit = b3;
        for j in 0..HIDDEN2_SIZE {
            logit += h2[j] * w3[j];
        }

        // Sigmoid: 1 / (1 + exp(-x)).  Equivalent to: logit >= 0.
        // We skip the actual sigmoid computation since we only need the
        // threshold comparison at 0.5.
        logit >= 0.0
    }
}
