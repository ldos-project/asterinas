// SPDX-License-Identifier: MPL-2.0

#![cfg(not(baseline_asterinas))]

use alloc::{sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicBool, Ordering};

use aster_block::{BlockDevice, bio::BlockDeviceCompletionStats};
use ostd::{
    Error,
    orpc::{
        oqueue::{OQueueError, StrongObserver},
        sync::{BlockOnMany, Blocker, TimeoutBlocker},
    },
    task::disable_preempt,
    timer::Jiffies,
};

/// Heimdall: an asynchronous device performance monitor for RAID-1 arrays.
///
/// Heimdall maintains one ML model per member device. A dedicated background
/// thread ([`Heimdall::run`]) owns the per-device strong observers and
/// continuously drains completion stats from each device's OQueue. Every
/// `batch_size` completions, it runs an ML inference to update that device's
/// fast/slow indicator. Or the inference is triggered if `inference_timeout_ms`
/// elapses since the last inference for that device. Whichever condition is met
/// first.
///
/// Model architecture (per device):
///   Linear(INPUT_DIM, 16) -> ReLU -> Linear(16, 1) -> Sigmoid
///
/// The observers are deliberately *not* stored in this shared handle: they are
/// owned solely by the monitor thread, so this struct stays `Sync` and can be
/// shared with the RAID device via `Arc` without wrapping the (`!Sync`)
/// observers in a lock.
///
/// Selection policies can query `is_device_fast(idx)` to incorporate Heimdall's
/// classification into their scheduling decisions.
pub struct Heimdall {
    members: Vec<Arc<dyn BlockDevice>>,
    /// Per-device fast/slow indicator. `true` means the device is predicted fast.
    fast_indicators: Vec<AtomicBool>,
    /// Per-device fc1 weights: [INPUT_DIM][HIDDEN_SIZE]
    fc1_weights: Vec<[[f32; HIDDEN_SIZE]; INPUT_DIM]>,
    /// Per-device fc1 biases: [HIDDEN_SIZE]
    fc1_biases: Vec<[f32; HIDDEN_SIZE]>,
    /// Per-device fc3 weights: [HIDDEN_SIZE]
    fc3_weights: Vec<[f32; HIDDEN_SIZE]>,
    /// Per-device fc3 biases: scalar
    fc3_biases: Vec<f32>,
    /// Number of completion records to drain before running an inference.
    batch_size: usize,
    /// Inference timeout in milliseconds.
    inference_timeout_ms: u64,
}

impl core::fmt::Debug for Heimdall {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Heimdall")
            .field("members", &self.members)
            .field("fast_indicators", &self.fast_indicators)
            .finish()
    }
}

use crate::heimdall_weights::{HIDDEN_SIZE, INPUT_DIM};

/// Default number of completion records to drain before running an inference,
/// used when the `heimdall.batch_size` kernel parameter is not set.
pub const DEFAULT_BATCH_SIZE: usize = 6;

/// Default inference timeout in milliseconds, used when the
/// `heimdall.inference_timeout_ms` kernel parameter is not set.
pub const DEFAULT_INFERENCE_TIMEOUT_MS: u64 = 28;

impl Heimdall {
    /// Creates a new Heimdall monitor.
    ///
    /// `device_observers` — one `(member device, strong observer)` pair per
    /// RAID-1 member; the observer is attached to the member's bio completion
    /// OQueue.
    /// `batch_size` — completion records to drain before running an inference.
    /// `inference_timeout_ms` — inference timeout in milliseconds.
    ///
    /// Returns the shared handle together with the per-member observers. The
    /// caller must hand the observers to [`Self::run`] on the monitor thread.
    pub fn new(
        device_observers: Vec<(
            Arc<dyn BlockDevice>,
            StrongObserver<BlockDeviceCompletionStats>,
        )>,
        batch_size: usize,
        inference_timeout_ms: u64,
    ) -> Result<(Arc<Self>, Vec<StrongObserver<BlockDeviceCompletionStats>>), Error> {
        use crate::heimdall_weights::{FC1_BIASES, FC1_WEIGHTS, FC3_BIASES, FC3_WEIGHTS};

        let num_devices = device_observers.len();

        let (members, observers): (Vec<_>, Vec<_>) = device_observers.into_iter().unzip();

        let fast_indicators: Vec<AtomicBool> = (0..num_devices)
            .map(|_| AtomicBool::new(true)) // optimistic: assume fast initially
            .collect();

        let fc1_weights: Vec<_> = (0..num_devices).map(|i| *FC1_WEIGHTS[i]).collect();
        let fc1_biases: Vec<_> = (0..num_devices).map(|i| *FC1_BIASES[i]).collect();
        let fc3_weights: Vec<_> = (0..num_devices).map(|i| *FC3_WEIGHTS[i]).collect();
        let fc3_biases: Vec<_> = (0..num_devices).map(|i| FC3_BIASES[i]).collect();

        log::info!("Heimdall created with {} devices", fast_indicators.len());

        let heimdall = Arc::new(Self {
            members,
            fast_indicators,
            fc1_weights,
            fc1_biases,
            fc3_weights,
            fc3_biases,
            batch_size,
            inference_timeout_ms,
        });

        Ok((heimdall, observers))
    }

    /// Returns whether device `idx` is currently classified as fast.
    pub fn is_device_fast(&self, idx: usize) -> bool {
        self.fast_indicators[idx].load(Ordering::Relaxed)
    }

    /// Returns the fast indicator for device `idx`.
    ///
    /// `true` means the device is currently predicted fast; `false` means slow.
    pub fn check_device(&self, idx: usize) -> bool {
        self.fast_indicators[idx].load(Ordering::Relaxed)
    }

    /// The number of member devices being monitored.
    pub fn num_devices(&self) -> usize {
        self.members.len()
    }

    /// Main monitoring loop. This should be spawned on a dedicated thread, which
    /// takes ownership of `observers` (one per member, in member order).
    ///
    /// For each device, drains completion records from its strong observer.
    /// Inference is triggered for a device when either condition is met first:
    ///   1. `batch_size` records have been drained, or
    ///   2. `inference_timeout_ms` have elapsed since the last inference.
    pub fn run(&self, observers: Vec<StrongObserver<BlockDeviceCompletionStats>>) {
        let num_devices = self.members.len();
        // TIMER_FREQ is in Hz, so this converts the millisecond timeout to jiffies.
        let timeout_jiffies = self.inference_timeout_ms * ostd::timer::TIMER_FREQ / 1000;

        // Per-device batch buffers for accumulating stats between inferences.
        let mut batch_buffers: Vec<Vec<BlockDeviceCompletionStats>> = (0..num_devices)
            .map(|_| Vec::with_capacity(self.batch_size))
            .collect();

        // Per-device jiffies timestamp of the last inference (or loop start).
        let now = Jiffies::elapsed().as_u64();
        let mut last_inference_jiffies = alloc::vec![now; num_devices];

        let timeout = TimeoutBlocker::new();
        let mut block_on_many = BlockOnMany::new();

        loop {
            let mut earliest_deadline: Option<u64> = None;
            for device_idx in 0..num_devices {
                let observer = &observers[device_idx];

                // Drain all available records (non-blocking).
                loop {
                    match observer.try_strong_observe() {
                        Ok(Some(stats)) => {
                            batch_buffers[device_idx].push(stats);

                            // Condition 1: batch is full.
                            // Do device inference, then break to give other devices a turn.
                            if batch_buffers[device_idx].len() >= self.batch_size {
                                self.run_inference(device_idx, &mut batch_buffers[device_idx]);
                                last_inference_jiffies[device_idx] = Jiffies::elapsed().as_u64();
                                break;
                            }
                        }
                        Ok(None) => break, // queue is empty, move on to timeout check
                        Err(OQueueError::Revoked { .. }) => {
                            log::warn!("Heimdall: observer for device {} detached", device_idx);
                            break;
                        }
                        Err(e) => {
                            log::warn!("Heimdall: error observing device {}: {:?}", device_idx, e);
                            break;
                        }
                    }
                }

                // Condition 2: timeout elapsed and there is at least some data.
                let elapsed = Jiffies::elapsed()
                    .as_u64()
                    .wrapping_sub(last_inference_jiffies[device_idx]);
                if elapsed >= timeout_jiffies && !batch_buffers[device_idx].is_empty() {
                    self.run_inference(device_idx, &mut batch_buffers[device_idx]);
                    last_inference_jiffies[device_idx] = Jiffies::elapsed().as_u64();
                }

                if !batch_buffers[device_idx].is_empty() {
                    let deadline =
                        last_inference_jiffies[device_idx].saturating_add(timeout_jiffies);
                    earliest_deadline =
                        Some(earliest_deadline.map_or(deadline, |d| d.min(deadline)));
                }
            }

            // Arm the timeout only if at least one device has a pending partial batch;
            match earliest_deadline {
                Some(deadline) => timeout.arm_at(deadline),
                None => timeout.disarm(),
            }

            let blockers = observers
                .iter()
                .map(|observer| observer as &dyn Blocker)
                .chain(core::iter::once(&*timeout as &dyn Blocker));
            block_on_many.block_on(blockers);
        }
    }

    /// Run inference for a single device and update its fast indicator.
    fn run_inference(&self, device_idx: usize, batch: &mut Vec<BlockDeviceCompletionStats>) {
        // Model output: 1 → slow (reject IO), 0 → fast (accept IO).
        let is_slow = self.infer_device_speed(device_idx, batch);
        self.fast_indicators[device_idx].store(!is_slow, Ordering::Relaxed);
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
                let latency_us = rec.latency.as_micros() as u64;
                input[5 + hist] = latency_us as f32;
                input[8 + hist] = if latency_us > 0 {
                    rec.request_size_pages as f32 / latency_us as f32
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
    ///   Linear(INPUT_DIM, 16) -> ReLU -> Linear(16, 1) -> Sigmoid
    ///
    /// Returns `true` if the model output >= 0.5 (sigmoid(logit) >= 0.5 ⟺ logit >= 0).
    fn infer_device_speed(&self, device_idx: usize, batch: &[BlockDeviceCompletionStats]) -> bool {
        let _guard = disable_preempt();
        let input = self.build_features(batch);

        // fc1: input (INPUT_DIM) x fc1_weights (INPUT_DIM x HIDDEN_SIZE) + bias -> ReLU
        let w1 = &self.fc1_weights[device_idx];
        let b1 = &self.fc1_biases[device_idx];
        let mut h1 = [0.0f32; HIDDEN_SIZE];
        for j in 0..HIDDEN_SIZE {
            let mut sum = b1[j];
            for i in 0..INPUT_DIM {
                sum += input[i] * w1[i][j];
            }
            h1[j] = if sum > 0.0 { sum } else { 0.0 }; // ReLU
        }

        // fc3: h1 (HIDDEN_SIZE) x fc3_weights (HIDDEN_SIZE) + bias -> Sigmoid
        let w3 = &self.fc3_weights[device_idx];
        let b3 = self.fc3_biases[device_idx];
        let mut logit = b3;
        for j in 0..HIDDEN_SIZE {
            logit += h1[j] * w3[j];
        }

        // Sigmoid: 1 / (1 + exp(-x)).  Equivalent to: logit >= 0.
        // We skip the actual sigmoid computation since we only need the
        // threshold comparison at 0.5.
        logit >= 0.0
    }
}
