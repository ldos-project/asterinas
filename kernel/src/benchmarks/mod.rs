// SPDX-License-Identifier: MPL-2.0
use alloc::{boxed::Box, sync::Arc};
use core::sync::atomic::{AtomicUsize, Ordering};

use super::{time, *};

mod fn_call;
mod oqueue;

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
    pub fn run(karg: &super::KCmdlineArg) {
        let mut bench = Self::new(karg);

        fn_call::register_benchmarks(&mut bench);
        oqueue::register_benchmarks(&mut bench);

        bench.main();
    }

    fn new(karg: &super::KCmdlineArg) -> Self {
        let n_threads = karg
            .get_module_arg_by_name::<usize>("bench", "n_threads")
            .unwrap_or(ostd::cpu::num_cpus());

        let n_repeat = karg
            .get_module_arg_by_name::<usize>("bench", "n_repeat")
            .unwrap_or(1);
        assert_ne!(n_repeat, 0);

        Self {
            n_threads,
            n_repeat,
            benchmark: karg
                .get_module_arg_by_name::<String>("bench", "benchmark")
                .unwrap(),

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
        let benchmark: &mut alloc::boxed::Box<dyn Benchmark> = match benchmark {
            Some(b) => b,
            None => panic!(
                "Could not find benchmark {}. Availible benchmarks {:?}",
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
