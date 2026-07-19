// SPDX-License-Identifier: MPL-2.0

//! The assembler crate for the Asterinas kernel.

#![no_std]
#![no_main]
#![deny(unsafe_code)]

#[ostd::main]
fn main() {
    aster_core::boot();
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
        time::init();
        vm::vmar::init_in_first_kthread();
    });
}
