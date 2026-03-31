// SPDX-License-Identifier: MPL-2.0

//! PMU (Performance Monitoring Unit) hardware access.
//!
// TODO(tewaro, after SOSP) make generic for counters
// TODO(tewaro, after SOSP) support machines other than Broadwell

use x86::msr::{rdmsr, wrmsr};

// MSR Addresses
const IA32_PERFEVTSEL0: u32 = 0x186;
const IA32_PERFEVTSEL1: u32 = 0x187;
const IA32_PMC0: u32 = 0xC1;
const IA32_PMC1: u32 = 0xC2;
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;

// Icelake-Server event configurations
// EN=1 (bit 22), OS=1 (bit 17), USR=1 (bit 16), CMASK=0
// Event 0x08 = DTLB_LOAD_MISSES
/// Umask 0x20: Miss L1 DTLB, Hit STLB
const STLB_HIT_CONFIG: u64 = 0x00432008;
/// Umask 0x0E: Miss all TLBs, page walk completed
const STLB_MISS_CONFIG: u64 = 0x00430E08;

/// Disables all PMU counters globally and programs the dTLB event selectors.
pub fn pmu_reset() {
    // TODO(tewaro, after SOSP): check cpuid at startups and error if pmu is enabled on unsupported
    // CPUs.
    // SAFETY: Writing well-defined Intel PMU MSRs (IA32_PERF_GLOBAL_CTRL,
    // IA32_PERFEVTSEL0/1). These are valid MSRs on Icelake-Server and will not
    // cause UB — they only affect hardware performance counter state.
    unsafe {
        wrmsr(IA32_PERF_GLOBAL_CTRL, 0);
        wrmsr(IA32_PERFEVTSEL0, STLB_HIT_CONFIG);
        wrmsr(IA32_PERFEVTSEL1, STLB_MISS_CONFIG);
    }
}

/// Enables PMC0 and PMC1 globally to begin counting.
pub fn pmu_start() {
    // SAFETY: Writing IA32_PERF_GLOBAL_CTRL to enable PMC0 and PMC1.
    // This is a valid MSR write that only affects hardware counter state.
    unsafe {
        wrmsr(IA32_PERF_GLOBAL_CTRL, 0b11);
    }
}

/// Reads the current dTLB miss counters.
///
/// Returns `(miss_l1_tlb, miss_all_tlb)`.
pub fn pmu_read_dtlb() -> (u64, u64) {
    // SAFETY: Reading IA32_PMC0 and IA32_PMC1 are valid MSR reads that
    // return the current general-purpose performance counter values.
    unsafe { (rdmsr(IA32_PMC0), rdmsr(IA32_PMC1)) }
}
