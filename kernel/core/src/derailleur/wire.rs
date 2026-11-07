// SPDX-License-Identifier: MPL-2.0

//! The cassette's wire format: `include/uapi/linux/derailleur_cassette.h` in
//! the derailleur kernel, as offsets into the shared mailbox.  Everything is
//! little-endian and fixed width.  The whole layout is described, used or not.
#![allow(dead_code)]

/// The session's offset in the mailbox (`CAS_OFF`).
pub const CAS_OFF: usize = 2 << 20;
pub const CAS_MAGIC: u32 = 0x4341_5353;
pub const CAS_VERSION: u32 = 1;
pub const CAS_NREQ: usize = 64;
pub const CAS_KEY: usize = 48;
pub const CAS_HEAP: usize = 5 << 20;

pub const CAS_GET: u32 = 1;
pub const CAS_ACT: u32 = 2;

/// `struct cas_session` field offsets (relative to `CAS_OFF`).
pub mod session {
    pub const MAGIC: usize = 0;
    pub const VERSION: usize = 4;
    pub const PID: usize = 8;
    pub const SESSION: usize = 12;
    pub const NREQ: usize = 16;
    pub const BATCH: usize = 64;
    pub const DONE: usize = 128;
    pub const REQ: usize = 192;
    pub const RESP: usize = 4288;
    pub const HEAP: usize = 6336;
    /// `struct cas_req`: op u32, pad u32, arg u64, key[48].
    pub const REQ_SIZE: usize = 64;
    /// `struct cas_resp`: status s64, off u64, len u64, pad u64.
    pub const RESP_SIZE: usize = 32;
}

/// Page-request slots (`struct cas_page_slot`).
pub mod slot {
    pub const SLOTS_OFF: usize = 4096;
    /// Each slot is 4 KiB aligned and 4296 bytes long.
    pub const STRIDE: usize = 8192;
    pub const PAGES: usize = 512;
    pub const REQ_SEQ: usize = 0;
    pub const RESP_SEQ: usize = 64;
    pub const ADDR: usize = 128;
    pub const NPAGES: usize = 136;
    pub const WRITE: usize = 140;
    pub const STATUS: usize = 144;
    pub const OP: usize = 176;
    pub const OBJ: usize = 180;
    pub const GPA: usize = 200;

    pub const OP_VA: u32 = 0;
    pub const RW_FORCE: u32 = 2;

    pub const GPA_NONE: u64 = !0;
    pub const GPA_ZERO: u64 = 1 << 58;
    pub const GPA_FLAGS: u64 = (1 << 62) | (1 << 61) | (1 << 60) | (1 << 59) | (1 << 58) | (1 << 57);
}

/// Record types (`enum cas_rec_type`) and the record header (`struct cas_rec`).
pub mod rec {
    pub const T_PROC: u32 = 1;
    pub const T_LAYOUT: u32 = 2;
    pub const T_TASK: u32 = 3;
    pub const T_THREADS: u32 = 4;
    pub const T_CLOCK: u32 = 7;
    pub const T_VDSO: u32 = 8;
    /// type u32, version u16, pad u16, len u64.
    pub const HDR: usize = 16;
}

/// `struct cas_layout` / `struct cas_vma`.
pub mod layout {
    pub const GEN: usize = 16;
    pub const NR: usize = 24;
    pub const VMAS: usize = 32;
    pub const VMA_SIZE: usize = 48;
    pub const V_START: usize = 0;
    pub const V_END: usize = 8;
    pub const V_PROT: usize = 28;
    pub const V_KIND: usize = 32;

    pub const PROT_READ: u32 = 1;
    pub const PROT_WRITE: u32 = 2;
    pub const PROT_EXEC: u32 = 4;
    pub const KIND_SHARED: u32 = 3;
}

/// `struct cas_vdso`.
pub mod vdso {
    pub const TEXT: usize = 16;
    pub const SIZE: usize = 24;
    pub const IMAGE: usize = 32;
}

/// `struct cas_task`.
pub mod task {
    pub const TGID: usize = 16;
    pub const UMASK: usize = 20;
    pub const START_CODE: usize = 24;
    pub const END_CODE: usize = 32;
    pub const START_DATA: usize = 40;
    pub const END_DATA: usize = 48;
    pub const START_BRK: usize = 56;
    pub const BRK: usize = 64;
    pub const START_STACK: usize = 72;
    pub const ARG_START: usize = 80;
    pub const ARG_END: usize = 88;
    pub const ENV_START: usize = 96;
    pub const ENV_END: usize = 104;
    pub const VDSO_TEXT: usize = 112;
    pub const SIGPENDING: usize = 120;
    pub const SIGACT: usize = 128;
    pub const SIGACT_SIZE: usize = 32;
    pub const RLIM: usize = 128 + 64 * 32;
    pub const CWD: usize = 128 + 64 * 32 + 16 * 16;
    pub const SIZE: usize = CWD + 256;
}

/// `struct cas_threads` / `struct cas_thread`.
pub mod thread {
    /// The record header plus nr and padding: entries start here.
    pub const ENTRIES: usize = 64;
    pub const NR: usize = 16;
    pub const LEN: usize = 0;
    pub const TID: usize = 4;
    /// `struct cas_x86_regs`: r15 r14 r13 r12 bp bx r11 r10 r9 r8 ax cx dx si di ip flags sp fsbase gsbase.
    pub const REGS: usize = 8;
    pub const BLOCKED: usize = 168;
    pub const SIGPENDING: usize = 176;
    pub const CLEAR_CHILD_TID: usize = 184;
    pub const ROBUST_LIST: usize = 192;
    pub const RSEQ_PTR: usize = 200;
    pub const RSEQ_LEN: usize = 208;
    pub const SS_SP: usize = 216;
    pub const SS_SIZE: usize = 224;
    pub const SS_FLAGS: usize = 232;
    pub const XSTATE_SIZE: usize = 236;
    pub const COMM: usize = 240;
    pub const XSTATE: usize = 256;
}

/// Little-endian field readers over a byte slice (records are copied out of
/// the heap first).
pub fn u32_at(b: &[u8], off: usize) -> u32 {
    let mut v = [0u8; 4];
    v.copy_from_slice(&b[off..off + 4]);
    u32::from_le_bytes(v)
}

pub fn u64_at(b: &[u8], off: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[off..off + 8]);
    u64::from_le_bytes(v)
}
