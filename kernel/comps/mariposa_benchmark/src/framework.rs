// SPDX-License-Identifier: MPL-2.0

//! The parts of a benchmark run that are not specific to any one benchmark.
//!
//! A benchmark supplies a closure that measures one iteration; [`run`] collects a sample from it per
//! iteration and captures them, and [`report`] emits the metadata needed to interpret the result.
//! What is left in the benchmark is only what actually distinguishes it.

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::fmt::Debug;

use aster_logger::println;
// Capture to a block device is built on OQueues, so it does not exist in baseline mode.
#[cfg(not(baseline_asterinas))]
use mariposa_data_capture::DataCaptureFile;
use ostd::{arch::tsc_freq, orpc::errors::RPCError, ostd_error};
use serde::Serialize;
use snafu::{ResultExt as _, Snafu};

/// Prefix on every line a benchmark prints, so a host tool can pick the block out of a console log
/// that also carries ordinary kernel output. A benchmark reporting why it gave up prints
/// `{PREFIX}|error <name>: <reason>`.
pub(crate) const PREFIX: &str = "MARIPOSA_BENCH";

/// The ways a run can end early that are the framework's own doing rather than the benchmark's.
///
/// A benchmark's error type absorbs these through a transparent variant, so that one type describes
/// every way its run can end.
#[ostd_error]
#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("cannot reserve room for {iterations} samples ({context})"))]
    OutOfMemory { iterations: usize },
    #[snafu(display("could not write the samples: {source}"))]
    CaptureWrite { source: RPCError },
    #[snafu(display("could not sync the samples: {source}"))]
    CaptureSync { source: RPCError },
}

/// Collects one sample per iteration for `iterations` iterations and captures them, returning how
/// many were captured.
///
/// `benchmark_fn` is given the iteration's sequence number and returns that iteration's sample. Its
/// error type is the benchmark's own, and has to be able to absorb an [`Error`] so that both kinds
/// of failure reach the caller as one type.
///
/// Samples are held in memory until measurement is over, so writing them is never part of what is
/// being timed. That bounds a run by memory: `iterations` samples must fit.
///
/// This returns only once the samples are on the device, so its `Ok` is what a benchmark should wait
/// for before calling [`report`].
#[cfg(not(baseline_asterinas))]
pub fn run<S, E>(
    iterations: u32,
    capture_file: Arc<dyn DataCaptureFile<S>>,
    mut benchmark_fn: impl FnMut(u64) -> Result<S, E>,
) -> Result<usize, E>
where
    S: Copy + Send + Serialize + 'static,
    E: From<Error>,
{
    // Reserve the whole sample array up front so recording never allocates on the hot path.
    let mut samples: Vec<S> = Vec::new();
    if samples.try_reserve_exact(iterations as usize).is_err() {
        return Err(OutOfMemorySnafu {
            iterations: iterations as usize,
        }
        .build()
        .into());
    }

    for seq in 0..iterations as u64 {
        samples.push(benchmark_fn(seq)?);
    }

    let measured = samples.len();
    capture_file
        .write_values(Box::new(samples.into_iter()))
        .context(CaptureWriteSnafu)?;
    // Stopping the capture file syncs it and only returns once the server thread is done, so the
    // samples are on the device before this returns and the caller reports success.
    capture_file.stop().context(CaptureSyncSnafu)?;

    Ok(measured)
}

/// Emits the metadata needed to interpret a completed run: the benchmark's name, the TSC frequency
/// its cycle counts are relative to, the configuration it ran under, and how many samples it
/// captured.
///
/// The configuration is only printed, so it is taken as something printable rather than at a type:
/// what a benchmark is configured with is its own business, and echoing it verbatim through `Debug`
/// is what lets a results file be matched back to the run that produced it -- and keeps it complete
/// as a benchmark gains parameters.
///
/// Call this only once the samples are safely written, since the `end` line is what a harness waits
/// for.
pub fn report(name: &str, config: &dyn Debug, samples: usize) {
    println!("{PREFIX}|begin {name}");
    println!("{PREFIX}|tsc_freq_hz {}", tsc_freq());
    println!("{PREFIX}|config {config:?}");
    println!("{PREFIX}|samples {samples}");
    println!("{PREFIX}|end {name}");
}
