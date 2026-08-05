// SPDX-License-Identifier: MPL-2.0

//! The 31-element LinnOS feature vector, built bit-identically to the kernel policies.
//!
//! Layout (matches `LinnOSPolicy::select_block_device` in the kernel):
//!   - `input[0..3]`  — current outstanding pages (3 digits: hundreds, tens, ones)
//!   - for each of 4 history steps `i` (`base = 3 + i*7`):
//!       - `input[base..base+3]`   outstanding pages of that completion (3 digits)
//!       - `input[base+3..base+7]` latency in microseconds (4 digits)
//!
//! The kernel observes the last 4 completions "most recent last": when fewer than 4 completions have
//! happened it fills the *oldest* slots (low `i`) with `None`, leaving those digits 0.0. We replicate
//! that by right-aligning the available history into the 4 slots.

/// Number of history steps in the feature vector; must match the kernel's `weak_observe_recent(4)`.
pub const HISTORY_LEN: usize = 4;

/// Total feature-vector length (3 current-outstanding_pages digits + 4 steps * 7 digits).
pub const FEATURE_LEN: usize = 31;

/// One completion sample kept in a member's history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockDeviceCompletionStats {
    /// Number of outstanding_pages 4KB pages recorded at this completion.
    pub outstanding_pages: u32,
    /// Request latency in microseconds.
    pub latency_us: u64,
}

/// Builds the 31 feature digits (each 0–9). `history` is oldest-first and holds at most
/// [`HISTORY_LEN`] entries; it is right-aligned into the 4 history slots so the newest completion
/// lands in the last slot, exactly as the kernel's `weak_observe_recent` ordering does.
fn feature_digits_raw(current_outstanding: u32, history: &[BlockDeviceCompletionStats]) -> [u8; FEATURE_LEN] {
    let mut input = [0u8; FEATURE_LEN];

    let co = current_outstanding as usize;
    input[0] = ((co / 100) % 10) as u8;
    input[1] = ((co / 10) % 10) as u8;
    input[2] = (co % 10) as u8;

    // Right-align: with `k` entries available, they occupy slots `HISTORY_LEN-k .. HISTORY_LEN`,
    // leaving the older (low-index) slots at 0 — matching the kernel's `None` history slots.
    let start = HISTORY_LEN.saturating_sub(history.len());
    for (i, completion) in history.iter().enumerate() {
        let slot = start + i;
        if slot >= HISTORY_LEN {
            break;
        }
        let base = 3 + slot * 7;
        let out = completion.outstanding_pages as usize;
        let lat = completion.latency_us as usize;

        input[base] = ((out / 100) % 10) as u8;
        input[base + 1] = ((out / 10) % 10) as u8;
        input[base + 2] = (out % 10) as u8;

        input[base + 3] = ((lat / 1000) % 10) as u8;
        input[base + 4] = ((lat / 100) % 10) as u8;
        input[base + 5] = ((lat / 10) % 10) as u8;
        input[base + 6] = (lat % 10) as u8;
    }

    input
}

/// The feature vector as `u8` digits (the decision-tree input).
pub fn feature_digits_u8(current_outstanding: u32, history: &[BlockDeviceCompletionStats]) -> [u8; FEATURE_LEN] {
    feature_digits_raw(current_outstanding, history)
}

/// The feature vector as `f32` digits (the neural-network input).
pub fn feature_digits_f32(current_outstanding: u32, history: &[BlockDeviceCompletionStats]) -> [f32; FEATURE_LEN] {
    let raw = feature_digits_raw(current_outstanding, history);
    let mut out = [0.0f32; FEATURE_LEN];
    for (dst, &src) in out.iter_mut().zip(raw.iter()) {
        *dst = src as f32;
    }
    out
}

/// Offline check that `feature_digits` reproduces the kernel's feature vector exactly, including the
/// fewer-than-4-history case where the kernel leaves the oldest slots at 0. The expected vectors are
/// hand-derived from the kernel algorithm, not from this module, so a divergence is caught.
pub fn feature_vector_parity_self_test() -> Result<(), String> {
    // Case A: current outstanding_pages 123 with a full 4-completion history (oldest first).
    let history_a = [
        BlockDeviceCompletionStats { outstanding_pages: 45, latency_us: 6789 },
        BlockDeviceCompletionStats { outstanding_pages: 8, latency_us: 100 },
        BlockDeviceCompletionStats { outstanding_pages: 250, latency_us: 9999 },
        BlockDeviceCompletionStats { outstanding_pages: 0, latency_us: 5 },
    ];
    #[rustfmt::skip]
    let expected_a: [u8; FEATURE_LEN] = [
        1, 2, 3, // current outstanding_pages 123
        0, 4, 5, 6, 7, 8, 9, // step0: out 45 -> 0,4,5 ; lat 6789 -> 6,7,8,9
        0, 0, 8, 0, 1, 0, 0, // step1: out 8 -> 0,0,8 ; lat 100 -> 0,1,0,0
        2, 5, 0, 9, 9, 9, 9, // step2: out 250 -> 2,5,0 ; lat 9999 -> 9,9,9,9
        0, 0, 0, 0, 0, 0, 5, // step3: out 0 -> 0,0,0 ; lat 5 -> 0,0,0,5
    ];
    let got_a = feature_digits_raw(123, &history_a);
    if got_a != expected_a {
        return Err(format!(
            "feature vector parity (full history) mismatch:\n got {got_a:?}\n exp {expected_a:?}"
        ));
    }

    // Case B: current outstanding_pages 7 with only 2 completions -> right-aligned into slots 2 and 3,
    // slots 0 and 1 left at 0 (the kernel's `None` history slots).
    let history_b = [
        BlockDeviceCompletionStats { outstanding_pages: 12, latency_us: 34 },
        BlockDeviceCompletionStats { outstanding_pages: 999, latency_us: 1234 },
    ];
    #[rustfmt::skip]
    let expected_b: [u8; FEATURE_LEN] = [
        0, 0, 7, // current outstanding_pages 7
        0, 0, 0, 0, 0, 0, 0, // step0: empty (older slot)
        0, 0, 0, 0, 0, 0, 0, // step1: empty (older slot)
        0, 1, 2, 0, 0, 3, 4, // step2: out 12 -> 0,1,2 ; lat 34 -> 0,0,3,4
        9, 9, 9, 1, 2, 3, 4, // step3: out 999 -> 9,9,9 ; lat 1234 -> 1,2,3,4
    ];
    let got_b = feature_digits_raw(7, &history_b);
    if got_b != expected_b {
        return Err(format!(
            "feature vector parity (short history) mismatch:\n got {got_b:?}\n exp {expected_b:?}"
        ));
    }

    // Case C: no history at all -> only the current-outstanding_pages digits are set.
    let got_c = feature_digits_raw(0, &[]);
    if got_c != [0u8; FEATURE_LEN] {
        return Err(format!("feature vector parity (empty history) mismatch: got {got_c:?}"));
    }

    Ok(())
}
