// SPDX-License-Identifier: MPL-2.0

//! Aster-nix is the Asterinas kernel, a safe, efficient unix-like
//! operating system kernel built on top of OSTD and OSDK.

#![no_std]
#![no_main]
#![deny(unsafe_code)]
#![feature(associated_type_defaults)]
#![feature(btree_cursors)]
#![feature(debug_closure_helpers)]
#![feature(format_args_nl)]
#![feature(linked_list_cursors)]
#![feature(linked_list_retain)]
#![feature(panic_can_unwind)]
#![feature(register_tool)]
#![feature(min_specialization)]
#![feature(thin_box)]
#![register_tool(component_access_control)]

extern crate alloc;
extern crate lru;
#[macro_use]
extern crate controlled;
#[macro_use]
extern crate getset;
#[macro_use]
extern crate ostd_pod;

// Set this crate's log prefix for `ostd::log`.
macro_rules! __log_prefix {
    () => {
        ""
    };
}

#[cfg_attr(target_arch = "x86_64", path = "arch/x86/mod.rs")]
#[cfg_attr(target_arch = "riscv64", path = "arch/riscv/mod.rs")]
#[cfg_attr(target_arch = "loongarch64", path = "arch/loongarch/mod.rs")]
mod arch;

mod context;
mod cpu;
#[cfg(not(baseline_asterinas))]
mod data_capture;
mod device;
mod driver;
mod error;
#[cfg(not(baseline_asterinas))]
mod event;
mod events;
mod fs;
mod init;
mod ipc;
mod net;
#[cfg(not(baseline_asterinas))]
mod orpc_utils;
mod prelude;
mod process;
mod sched;
mod security;
mod syscall;
mod thread;
mod time;
mod util;
// TODO: Add vDSO support for other architectures.
mod benchmarks;
pub mod kcmdline;
#[cfg(any(target_arch = "x86_64", target_arch = "riscv64"))]
mod vdso;
mod vm;

#[controlled]
#[ostd::main]
fn main() {
    init::main();
}

#[cfg(ktest)]
/// Initialize kernel subsystems for ktests. This is not a complete initialization, but does load
/// all components.
///
/// This is idempotent and should be called in *every* test that needs it. This avoids at least some
/// test order dependence.
///
/// TODO(arthurp, https://github.com/ldos-project/asterinas/issues/221): Something less ad-hoc.
pub fn init_for_ktest() {
    use spin::Once;

    pub static INITIALIZED: Once<()> = Once::new();
    INITIALIZED.call_once(|| {
        component::init_all(
            component::InitStage::Bootstrap,
            component::parse_metadata!(),
        )
        .unwrap();
        crate::time::init();
        crate::vm::vmar::init_in_first_kthread();
    });
}
