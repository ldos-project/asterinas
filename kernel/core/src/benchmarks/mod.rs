// SPDX-License-Identifier: MPL-2.0
use alloc::{boxed::Box, string::String, sync::Arc, vec, vec::Vec};
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use aster_logger::println;
use spin::Once;

use super::{time, *};
use crate::time::Clock as _;

/// Returns true if any benchmark parameters have been set.
pub fn is_benchmark_enabled() -> bool {
    BENCH_NAME.get().is_some()
}

mod fn_call;
#[cfg(not(baseline_asterinas))]
mod legacy_oqueue;
mod oqueue;
#[cfg(not(baseline_asterinas))]
pub(super) mod oqueue_roundtrip;

pub trait Benchmark {
    fn init(&mut self, _n_threads: usize, _n_repeat: usize, _iter: usize) {}
    fn run(&self, completed: Arc<AtomicUsize>);
    fn finalize(&self) {}

    fn name(&self) -> &str;
}

pub struct BenchmarkHarness {
    pub n_threads: usize,
    pub n_repeat: usize,
    pub benchmark: String,

    benchmarks: Vec<Box<dyn Benchmark>>,
}

impl BenchmarkHarness {
    pub fn run() {
        let mut bench = Self::new();

        fn_call::register_benchmarks(&mut bench);
        #[cfg(not(baseline_asterinas))]
        oqueue::register_benchmarks(&mut bench);

        bench.main();
    }

    fn new() -> Self {
        let n_threads = BENCH_N_THREADS.load(Ordering::Relaxed) as usize;

        let n_repeat = BENCH_N_REPEAT.load(Ordering::Relaxed) as usize;
        assert_ne!(n_repeat, 0);

        Self {
            n_threads,
            n_repeat,
            benchmark: BENCH_NAME
                .get()
                .expect("missing bench.benchmark=... on kernel command line")
                .clone(),

            benchmarks: vec![],
        }
    }

    pub fn register_benchmark(&mut self, bench: Box<dyn Benchmark>) {
        let name = bench.name();
        if self.benchmarks.iter().any(|b| b.name() == name) {
            panic!("Duplicate benchmark {} registered!", bench.name());
        }

        self.benchmarks.push(bench);
    }

    pub fn main(&mut self) {
        let benchmark = self
            .benchmarks
            .iter_mut()
            .find(|b| b.name() == self.benchmark.as_str());
        let benchmark: &mut Box<dyn Benchmark> = match benchmark {
            Some(b) => b,
            None => panic!(
                "Could not find benchmark {}. Available benchmarks {:?}",
                self.benchmark,
                self.benchmarks.iter().map(|b| b.name()).collect::<Vec<_>>()
            ),
        };

        for i in 0..self.n_repeat {
            benchmark.init(self.n_threads, self.n_repeat, i);

            let now = time::clocks::RealTimeClock::get().read_time();

            let completed = Arc::new(AtomicUsize::new(0));

            benchmark.run(completed.clone());

            println!("Waiting for benchmark to complete");
            // Exit after benchmark completes
            while completed.load(Ordering::Relaxed) != self.n_threads {
                core::hint::spin_loop();
            }
            let end = time::clocks::RealTimeClock::get().read_time();

            benchmark.finalize();

            println!("[total] {:?}", end - now);
        }
    }
}

static BENCH_N_THREADS: AtomicU32 = AtomicU32::new(0);
aster_cmdline::define_kv_param!("bench.n_threads", BENCH_N_THREADS);

static BENCH_N_REPEAT: AtomicU32 = AtomicU32::new(1);
aster_cmdline::define_kv_param!("bench.n_repeat", BENCH_N_REPEAT);

static BENCH_NAME: Once<String> = Once::new();
aster_cmdline::define_kv_param!("bench.benchmark", BENCH_NAME);

static BENCH_Q_TYPE: Once<String> = Once::new();
aster_cmdline::define_kv_param!("bench.q_type", BENCH_Q_TYPE);
