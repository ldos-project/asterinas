// SPDX-License-Identifier: MPL-2.0

// PLACEHOLDER per-device decision-tree prediction functions.
//
// This is GENERATED CODE. Regenerate real trees (which MUST NOT be committed) from trained sklearn
// checkpoints with, per device N:
//
//   python kernel/comps/raid/python/generate_decision_tree.py \
//       --model results/dt_deviceN.pkl \
//       --format rust --fn_name predict_deviceN \
//       --out test/initramfs/src/raid_policies/decision_tree/src/dt_deviceN.rs
//
// then paste the emitted functions here. (A dummy-checkpoint generator,
// kernel/comps/raid/python/generate_dummy_checkpoints.py, produces throwaway checkpoints so the
// pipeline can be exercised end to end.)
//
// The placeholder for each device is the trivial trained-nothing tree: a single leaf predicting
// "slow" (0), so the policy falls through to its round-robin fallback — expected for CI, which does
// not need an accurate policy.
//
// Input: &[u8; 31] — one byte per feature digit (0–9), the same layout as the LinnOS feature vector
// (`input[0..3]` current outstanding pages, then 4 history steps of 3 outstanding_pages + 4 latency digits).
// Returns: 0 (slow) or 1 (fast).

/// Predict fast (1) or slow (0) for device 0. Placeholder: always slow.
#[inline]
pub fn predict_device0(_x: &[u8; 31]) -> u8 {
    0
}

/// Predict fast (1) or slow (0) for device 1. Placeholder: always slow.
#[inline]
pub fn predict_device1(_x: &[u8; 31]) -> u8 {
    0
}

/// Predict fast (1) or slow (0) for device 2. Placeholder: always slow.
#[inline]
pub fn predict_device2(_x: &[u8; 31]) -> u8 {
    0
}
