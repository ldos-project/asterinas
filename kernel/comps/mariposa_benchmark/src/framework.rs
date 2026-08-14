// SPDX-License-Identifier: MPL-2.0

//! The parts of a benchmark run that are not specific to any one benchmark.

use core::fmt::Debug;

use aster_logger::println;
use ostd::{
    arch::tsc_freq,
    power::{ExitCode, poweroff},
};

/// Prefix on every line this module prints, so a host tool can pick the block out of a console log
/// that also carries ordinary kernel output.
const PREFIX: &str = "MARIPOSA_BENCH";

/// Emits the metadata needed to interpret a completed run: the benchmark's name, the TSC frequency
/// its cycle counts are relative to, the configuration it ran under, and how many samples it
/// captured.
///
/// The configuration is printed through its `Debug` representation, so it stays complete as a
/// benchmark gains parameters. Call this only once the samples are safely written, since the `end`
/// line is what a harness waits for.
pub fn report(name: &str, config: &dyn Debug, samples: usize) {
    println!("{PREFIX}|begin {name}");
    println!("{PREFIX}|tsc_freq_hz {}", tsc_freq());
    println!("{PREFIX}|config {config:?}");
    println!("{PREFIX}|samples {samples}");
    println!("{PREFIX}|end {name}");
}

/// Reports an anomaly and powers the machine off with a failure exit code.
///
/// Benchmarks stop the machine rather than panicking: a panic is only a per-thread oops here, which
/// would leave the guest hanging instead of failing.
pub fn fatal(message: core::fmt::Arguments<'_>) -> ! {
    println!("{PREFIX}|error {message}");
    poweroff(ExitCode::Failure)
}
