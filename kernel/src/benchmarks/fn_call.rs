use alloc::{boxed::Box, sync::Arc};
use core::{
    any::Any,
    sync::atomic::{AtomicUsize, Ordering},
};

use super::{Benchmark, BenchmarkHarness};
use crate::time::{Clock, clocks::RealTimeClock};

trait MyTrait: Any {
    fn do_work(&mut self);
}

struct MyStruct {
    calls: usize,
}

impl MyTrait for MyStruct {
    #[inline(never)]
    fn do_work(&mut self) {
        self.calls += 1;
    }
}

struct MyStruct2 {
    calls: usize,
}

impl MyTrait for MyStruct2 {
    #[inline(never)]
    fn do_work(&mut self) {
        self.calls += 2;
    }
}

#[inline(never)]
fn bench_call_overhead(s: &mut Box<MyStruct>) {
    let now = RealTimeClock::get().read_time();
    for _ in 0..1000000 {
        s.do_work();
    }
    let end = RealTimeClock::get().read_time();
    crate::prelude::println!("1M direct calls in {:?}", end - now);
}

struct FnCallOverheadBenchmark {}
impl Benchmark for FnCallOverheadBenchmark {
    fn init(&mut self, n_threads: usize, _n_repeat: usize, _iter: usize) {
        assert_eq!(n_threads, 1);
    }

    fn run(&self, completed: Arc<AtomicUsize>) {
        let mut s = Box::new(MyStruct { calls: 0 });
        bench_call_overhead(&mut s);
        completed.fetch_add(1, Ordering::Relaxed);
    }

    fn name(&self) -> &str {
        "fn_call::bench_call_overhead"
    }
}

#[inline(never)]
fn bench_dyn_call_overhead(s: &mut Box<dyn MyTrait>) {
    let now = RealTimeClock::get().read_time();
    for _ in 0..1000000 {
        s.do_work();
    }
    let end = RealTimeClock::get().read_time();
    crate::prelude::println!("1M dyn calls in in {:?}", end - now);
}

struct DynFnCallOverheadBenchmark {}
impl Benchmark for DynFnCallOverheadBenchmark {
    fn init(&mut self, n_threads: usize, _n_repeat: usize, _iter: usize) {
        assert_eq!(n_threads, 1);
    }

    fn run(&self, completed: Arc<AtomicUsize>) {
        let mut s: Box<dyn MyTrait> = Box::new(MyStruct { calls: 0 });
        bench_dyn_call_overhead(&mut s);
        let mut s: Box<dyn MyTrait> = Box::new(MyStruct2 { calls: 0 });
        bench_dyn_call_overhead(&mut s);

        completed.fetch_add(1, Ordering::Relaxed);
    }

    fn name(&self) -> &str {
        "fn_call::bench_dyn_call_overhead"
    }
}

pub fn register_benchmarks(bc: &mut BenchmarkHarness) {
    bc.register_benchmark(Box::new(FnCallOverheadBenchmark {}));
    bc.register_benchmark(Box::new(DynFnCallOverheadBenchmark {}));
}
