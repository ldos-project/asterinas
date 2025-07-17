//! Some simple utilities for running microbenchmarks (or potentially larger benchmarks) and
//! reporting the results.
//!
//! The basic use case is:
//!
//! * Use [`core::hint::black_box`] to block your inputs from the compiler.
//! * Create a timer: [`let timer = Timer::start();`](`Timer`)
//! * Run your code.
//! * Use `black_box` on the result.
//! * Use [`BenchmarkReporter`] to output your benchmark results.
//! 
//! *Note:* Make sure to include a warm-up, the kernel does take some time to get up to speed.
//! 
//! [`run_microbenchmark`] also provides a way to automatically compute an appropriate number of
//! iterations.

use alloc::{borrow::ToOwned, format, string::String, vec::Vec};
use core::{fmt::Debug, time::Duration};

use log::trace;
use num_traits::float::FloatCore;

use crate::{
    arch::{read_tsc, tsc_freq},
    prelude::*,
};

/// A time "mark" which represents a moment in time which is comparable only to near by `Mark`s.
#[derive(Debug, Copy, Clone)]
pub struct Mark {
    /// The value of TSC of this instance.
    tsc: u64,
}

impl Mark {
    /// Get the current time as accurately as practical while still being very fast.
    pub fn now() -> Mark {
        Mark { tsc: read_tsc() }
    }

    /// Return the time between `self` and `other`. This will only be correct if the two are closer
    /// together than the wrap-around period of the marks. On X86, this is 2^64 ticks at a processor
    /// specific number of ticks per second. This is generally quite large, on the order of year,
    /// but that is not guaranteed.
    pub fn difference(self, other: Mark) -> Duration {
        let diff = other.tsc.wrapping_sub(self.tsc);
        // We assume TSC frequency does not change during execution. This is kind of fundamental to
        // this code however.
        let ticks_per_second = tsc_freq();
        let diff = diff as f64 / ticks_per_second as f64;
        Duration::from_secs_f64(diff)
    }
}

/// A timer for a range of code.
pub struct Timer {
    start: Mark,
}

impl Timer {
    /// Start a timer.
    pub fn start() -> Self {
        let now = Mark::now();
        Self { start: now }
    }

    /// End a timer and return the amount of time since the start. This is only valid for times less
    /// than the roll-over time of the TSC. See [`Mark::difference`].
    pub fn end(self) -> Duration {
        let end = Mark::now();
        self.start.difference(end)
    }
}

/// A utility for collecting and exporting benchmark results. This makes sure the format is
/// consistent and that prints are not corrupted or mixed during printing. The results are not
/// printed until a print method is called.
///
/// Use this by creating and instance, calling [`BenchmarkReporter::report`] repeatedly, then
/// calling [`BenchmarkReporter::print_yaml`].
pub struct BenchmarkReporter {
    values: Vec<(String, String)>,
}

impl BenchmarkReporter {
    /// Create a new empty benchmark reporter.
    pub fn new() -> Self {
        Self {
            values: Default::default(),
        }
    }

    /// Report a value.
    pub fn report<T: Debug>(&mut self, name: &str, value: &T) {
        self.values.push((name.to_owned(), format!("{:?}", value)));
    }

    /// Print the benchmark results in YAML format. If the output of this is called repeatedly with
    /// no other text between, the result is a valid YAML list with an object as each element. This
    /// can be relatively easily converted to CSV.
    pub fn print_yaml(&self) {
        let s = self
            .values
            .iter()
            .map(|(name, value)| format!("    {}: {}", name, value))
            .reduce(|a, b| a + "\n" + &b)
            .unwrap();
        println!("\n-\n{}\n", s);
    }
}

impl Default for BenchmarkReporter {
    fn default() -> Self {
        Self::new()
    }
}

/// Run a benchmark function a few times to measure a rough iteration time, then run it enough to
/// take roughly `goal_runtime` and report the results of the longer run.
pub fn run_microbenchmark(
    f: impl Fn(usize) -> BenchmarkReporter,
    test_iterations: usize,
    goal_runtime: Duration,
) {
    let iterations = {
        let start = Timer::start();
        f(test_iterations);
        let time = start.end();
        trace!("Test run time: {:?}", time);
        goal_runtime.as_secs_f64() / (time.as_secs_f64() / test_iterations as f64)
    }
    .floor() as usize;
    trace!("Goal iterations: {}", iterations);
    let start = Timer::start();
    let mut reporter = f(iterations);
    let time = start.end();
    trace!("Full run time: {:?} (of goal {:?})", time, goal_runtime);
    reporter.report("iterations", &iterations);
    reporter.print_yaml();
}

#[cfg(ktest)]
mod tests {
    use core::hint::black_box;

    use super::*;

    #[ktest]
    fn test_timer() {
        for _ in 0..2 {
            let mut f = black_box(234.0);

            let t = Timer::start();
            for _ in 0..100000 {
                f += 1.0 / f;
            }
            let t = t.end();

            black_box(f);

            let mut reporter = BenchmarkReporter::new();
            reporter.report("timer", &(t / 100000));
            reporter.print_yaml();
        }
    }
}
