// SPDX-License-Identifier: MPL-2.0

//! PMU (Performance Monitoring Unit) hardware access.
//!
// TODO(tewaro, after SOSP) make generic for counters
// TODO(tewaro, after SOSP) support machines other than Broadwell

use core::{
    arch::asm,
    sync::atomic::{AtomicU8, Ordering},
};

use x86::msr::{rdmsr, wrmsr};

use super::cpu::cpuid::cpuid;

// MSR Addresses
const IA32_PERFEVTSEL0: u32 = 0x186;
const IA32_PERFEVTSEL1: u32 = 0x187;
const IA32_PMC0: u32 = 0xC1;
const IA32_PMC1: u32 = 0xC2;
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;
const IA32_PERFEVTSEL2: u32 = 0x188;
const IA32_PERFEVTSEL3: u32 = 0x189;
const IA32_PMC2: u32 = 0xC3;
const IA32_PMC3: u32 = 0xC4;
const IA32_FIXED_CTR0: u32 = 0x309;
const IA32_FIXED_CTR1: u32 = 0x30A;
const IA32_FIXED_CTR_CTRL: u32 = 0x38D;

/// The `IA32_PERF_GLOBAL_CTRL` bits of the dTLB counters, PMC0 and PMC1.
const DTLB_GLOBAL_CTRL_BITS: u64 = 0b11;

// Icelake-Server event configurations
// EN=1 (bit 22), OS=1 (bit 17), USR=1 (bit 16), CMASK=0
// Event 0x08 = DTLB_LOAD_MISSES
/// Umask 0x20: Miss L1 DTLB, Hit STLB
const STLB_HIT_CONFIG: u64 = 0x00432008;
/// Umask 0x0E: Miss all TLBs, page walk completed
const STLB_MISS_CONFIG: u64 = 0x00430E08;

/// Disables the dTLB counters and programs their event selectors.
///
/// Other counters, such as the task counters, are left running.
pub fn pmu_reset() {
    // TODO(tewaro, after SOSP): check cpuid at startups and error if pmu is enabled on unsupported
    // CPUs.
    // SAFETY: Writing well-defined Intel PMU MSRs (IA32_PERF_GLOBAL_CTRL,
    // IA32_PERFEVTSEL0/1). These are valid MSRs on Icelake-Server and will not
    // cause UB — they only affect hardware performance counter state.
    unsafe {
        let global_ctrl = rdmsr(IA32_PERF_GLOBAL_CTRL);
        wrmsr(IA32_PERF_GLOBAL_CTRL, global_ctrl & !DTLB_GLOBAL_CTRL_BITS);
        wrmsr(IA32_PERFEVTSEL0, STLB_HIT_CONFIG);
        wrmsr(IA32_PERFEVTSEL1, STLB_MISS_CONFIG);
    }
}

/// Enables PMC0 and PMC1 globally to begin counting, leaving other counters as they are.
pub fn pmu_start() {
    // SAFETY: Writing IA32_PERF_GLOBAL_CTRL to enable PMC0 and PMC1.
    // This is a valid MSR write that only affects hardware counter state.
    unsafe {
        let global_ctrl = rdmsr(IA32_PERF_GLOBAL_CTRL);
        wrmsr(IA32_PERF_GLOBAL_CTRL, global_ctrl | DTLB_GLOBAL_CTRL_BITS);
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

// Task counters
//
// The task counters are sampled at every context switch to attribute hardware events to tasks.
// They use the two fixed counters for cycles and instructions and the general-purpose counters
// PMC2 and PMC3, so that they can run alongside the dTLB counters on PMC0 and PMC1.

/// LONGEST_LAT_CACHE.MISS, an architectural event: event 0x2E, umask 0x41, EN, OS and USR.
const LLC_MISS_CONFIG: u64 = 0x0043_412E;
/// CYCLE_ACTIVITY.STALLS_L3_MISS: event 0xA3, umask 0x06, CMASK 6, EN, OS and USR.
///
/// This event is model-specific. It has this encoding from Skylake through Golden Cove (e.g., the
/// P-cores of Alder Lake and Raptor Lake), but is meaningless on other microarchitectures.
const L3_MISS_STALLS_CONFIG: u64 = 0x0643_06A3;
/// Counts both OS and USR for fixed counters 0 (instructions) and 1 (cycles).
const FIXED_CTR_CTRL_BITS: u64 = 0x33;
const FIXED_CTR_CTRL_MASK: u64 = 0xFF;
/// The `IA32_PERF_GLOBAL_CTRL` bits of PMC2, PMC3, and fixed counters 0 and 1.
const TASK_COUNTERS_GLOBAL_CTRL_BITS: u64 = (0b11 << 2) | (0b11 << 32);

/// The bit widths of the general-purpose and fixed counters, set by
/// [`task_counters_supported`].
static GP_COUNTER_WIDTH: AtomicU8 = AtomicU8::new(0);
static FIXED_COUNTER_WIDTH: AtomicU8 = AtomicU8::new(0);

/// Hardware event counts, either the raw counter values or the events between two samples.
#[derive(Debug, Clone, Copy, Default)]
pub struct TaskCounters {
    /// Unhalted core cycles.
    pub cycles: u64,
    /// Instructions retired.
    pub instructions: u64,
    /// Last-level cache misses.
    pub llc_misses: u64,
    /// Cycles stalled with an outstanding L3 miss. This event is model-specific; see
    /// `L3_MISS_STALLS_CONFIG`.
    pub l3_miss_stall_cycles: u64,
}

impl TaskCounters {
    /// Returns the events counted between `earlier` and `self`, allowing for counter wraparound.
    pub fn since(&self, earlier: &TaskCounters) -> TaskCounters {
        let gp_mask = width_mask(GP_COUNTER_WIDTH.load(Ordering::Relaxed));
        let fixed_mask = width_mask(FIXED_COUNTER_WIDTH.load(Ordering::Relaxed));
        TaskCounters {
            cycles: self.cycles.wrapping_sub(earlier.cycles) & fixed_mask,
            instructions: self.instructions.wrapping_sub(earlier.instructions) & fixed_mask,
            llc_misses: self.llc_misses.wrapping_sub(earlier.llc_misses) & gp_mask,
            l3_miss_stall_cycles: self
                .l3_miss_stall_cycles
                .wrapping_sub(earlier.l3_miss_stall_cycles)
                & gp_mask,
        }
    }
}

fn width_mask(width: u8) -> u64 {
    if width >= 64 {
        u64::MAX
    } else {
        (1 << width) - 1
    }
}

/// Returns whether the CPU supports the task counters, according to CPUID leaf 0xA.
///
/// This requires architectural performance monitoring version 2 or later with at least four
/// general-purpose counters, two fixed counters, and the core cycle, instruction retired and LLC
/// miss events. It does not check the model-specific L3 miss stall event.
pub fn task_counters_supported() -> bool {
    let Some(leaf) = cpuid(0xA, 0) else {
        return false;
    };
    let version = leaf.eax & 0xFF;
    let num_gp_counters = (leaf.eax >> 8) & 0xFF;
    let gp_width = (leaf.eax >> 16) & 0xFF;
    let ebx_len = (leaf.eax >> 24) & 0xFF;
    let num_fixed_counters = leaf.edx & 0x1F;
    let fixed_width = (leaf.edx >> 5) & 0xFF;

    // A set EBX bit means that the event is *not* available. Bit 0 is core cycles, bit 1 is
    // instructions retired, and bit 4 is LLC misses.
    let required_events = 0b1_0011;
    let supported = version >= 2
        && num_gp_counters >= 4
        && num_fixed_counters >= 2
        && ebx_len >= 5
        && leaf.ebx & required_events == 0
        && gp_width > 0
        && fixed_width > 0;
    if supported {
        GP_COUNTER_WIDTH.store(gp_width as u8, Ordering::Relaxed);
        FIXED_COUNTER_WIDTH.store(fixed_width as u8, Ordering::Relaxed);
    }
    supported
}

/// Programs and starts the task counters on the current CPU.
///
/// The caller must have checked [`task_counters_supported`].
pub fn task_counters_start() {
    // SAFETY: These are architectural PMU MSRs, which exist because the caller checked
    // `task_counters_supported`. Writing them only affects hardware counter state, and the
    // read-modify-writes leave the dTLB counters alone.
    unsafe {
        let global_ctrl = rdmsr(IA32_PERF_GLOBAL_CTRL);
        wrmsr(
            IA32_PERF_GLOBAL_CTRL,
            global_ctrl & !TASK_COUNTERS_GLOBAL_CTRL_BITS,
        );

        wrmsr(IA32_PERFEVTSEL2, LLC_MISS_CONFIG);
        wrmsr(IA32_PERFEVTSEL3, L3_MISS_STALLS_CONFIG);
        let fixed_ctrl = rdmsr(IA32_FIXED_CTR_CTRL);
        wrmsr(
            IA32_FIXED_CTR_CTRL,
            (fixed_ctrl & !FIXED_CTR_CTRL_MASK) | FIXED_CTR_CTRL_BITS,
        );
        for counter in [IA32_PMC2, IA32_PMC3, IA32_FIXED_CTR0, IA32_FIXED_CTR1] {
            wrmsr(counter, 0);
        }

        wrmsr(
            IA32_PERF_GLOBAL_CTRL,
            global_ctrl | TASK_COUNTERS_GLOBAL_CTRL_BITS,
        );
    }
}

/// Reads the task counters on the current CPU.
///
/// The caller must have started them with [`task_counters_start`] on this CPU.
pub fn task_counters_read() -> TaskCounters {
    /// Selects a fixed counter rather than a general-purpose one in `rdpmc`.
    const FIXED: u32 = 1 << 30;

    // SAFETY: `rdpmc` is allowed in kernel mode, and the counters exist because they were
    // started on this CPU.
    unsafe {
        TaskCounters {
            cycles: rdpmc(FIXED | 1),
            instructions: rdpmc(FIXED),
            llc_misses: rdpmc(2),
            l3_miss_stall_cycles: rdpmc(3),
        }
    }
}

/// Reads the performance counter selected by `counter`.
///
/// # Safety
///
/// The counter must exist.
unsafe fn rdpmc(counter: u32) -> u64 {
    let (low, high): (u32, u32);
    // SAFETY: The caller guarantees that the counter exists, so `rdpmc` does not fault.
    unsafe {
        asm!(
            "rdpmc",
            in("ecx") counter,
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((high as u64) << 32) | low as u64
}
