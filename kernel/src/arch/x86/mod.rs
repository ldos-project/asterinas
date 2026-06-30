// SPDX-License-Identifier: MPL-2.0

pub mod cpu;
#[cfg(not(baseline_asterinas))]
pub mod pmu;
mod power;
pub mod signal;

pub fn init() {
    power::init();
}
