// SPDX-License-Identifier: MPL-2.0
//
use alloc::sync::Arc;
use core::{
    any::Any,
    sync::atomic::{AtomicUsize, Ordering},
};

use ostd::orpc::oqueue::OQueue;

use super::time;
use crate::{benchmark_consts, prelude::*, thread::kernel_thread::ThreadOptions};
#[allow(dead_code)]
pub mod test_bench_overhead {
    use super::*;
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
        let now = time::clocks::RealTimeClock::get().read_time();
        for _ in 0..1000000 {
            s.do_work();
        }
        let end = time::clocks::RealTimeClock::get().read_time();
        println!("1M direct calls in {:?}", end - now);
    }

    #[inline(never)]
    fn bench_dyn_call_overhead(s: &mut Box<dyn MyTrait>) {
        let now = time::clocks::RealTimeClock::get().read_time();
        for _ in 0..1000000 {
            s.do_work();
        }
        let end = time::clocks::RealTimeClock::get().read_time();
        println!("1M dyn calls in in {:?}", end - now);
    }
}

#[allow(dead_code)]
pub fn produce_bench(
    q: &Arc<dyn OQueue<u64>>,
    completed: &Arc<AtomicUsize>,
    completed_wq: &Arc<ostd::sync::WaitQueue>,
) {
    println!("Starting producers");
    // Start all producers
    for tid in 0..benchmark_consts::N_THREADS {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(tid + 1).unwrap());
        ThreadOptions::new({
            let completed = completed.clone();
            let completed_wq = completed_wq.clone();
            let producer = q.attach_producer().unwrap();
            move || {
                let now = time::clocks::RealTimeClock::get().read_time();
                for _ in 0..benchmark_consts::N_MESSAGES_PER_THREAD {
                    producer.produce(0);
                }
                let end = time::clocks::RealTimeClock::get().read_time();
                println!(
                    "[producer-{}-{:?}] sent msg in {:?}",
                    tid,
                    ostd::cpu::CpuId::current_racy(),
                    end - now
                );
                completed.fetch_add(1, Ordering::Relaxed);
                completed_wq.wake_all();
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    }
}

#[allow(dead_code)]
pub fn consume_bench(
    q: &Arc<dyn OQueue<u64>>,
    completed: &Arc<AtomicUsize>,
    completed_wq: &Arc<ostd::sync::WaitQueue>,
) {
    println!("Populating queue");
    for tid in 0..benchmark_consts::N_THREADS {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(tid + 1).unwrap());
        ThreadOptions::new({
            let completed = completed.clone();
            let completed_wq = completed_wq.clone();
            let producer = q.attach_producer().unwrap();
            move || {
                for _ in 0..benchmark_consts::N_MESSAGES_PER_THREAD {
                    producer.produce(0);
                }
                completed.fetch_add(1, Ordering::Relaxed);
                completed_wq.wake_all();
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    }
    completed_wq.wait_until(|| {
        (completed.load(Ordering::Relaxed) == benchmark_consts::N_THREADS).then_some(())
    });
    completed.store(0, Ordering::Relaxed);

    // Start all consumers
    for tid in 0..benchmark_consts::N_THREADS {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(tid + 1).unwrap());
        ThreadOptions::new({
            let completed = completed.clone();
            let completed_wq = completed_wq.clone();
            let consumer = q.attach_consumer().unwrap();
            move || {
                let now = time::clocks::RealTimeClock::get().read_time();
                for _ in 0..benchmark_consts::N_MESSAGES_PER_THREAD {
                    let _ = consumer.consume();
                }
                let end = time::clocks::RealTimeClock::get().read_time();
                println!(
                    "[consumer-{}-{:?}] recv msg in {:?}",
                    tid,
                    ostd::cpu::CpuId::current_racy(),
                    end - now
                );
                completed.fetch_add(1, Ordering::Relaxed);
                completed_wq.wake_all();
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    }
}

#[allow(dead_code)]
pub fn mixed_bench(
    q: &Arc<dyn OQueue<u64>>,
    completed: &Arc<AtomicUsize>,
    completed_wq: &Arc<ostd::sync::WaitQueue>,
) {
    // Start all producers
    for tid in 0..(benchmark_consts::N_THREADS / 2) {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(tid + 1).unwrap());
        ThreadOptions::new({
            let completed = completed.clone();
            let completed_wq = completed_wq.clone();
            let producer = q.attach_producer().unwrap();
            move || {
                for _ in 0..benchmark_consts::N_MESSAGES_PER_THREAD {
                    producer.produce(0);
                }
                completed.fetch_add(1, Ordering::Relaxed);
                completed_wq.wake_one();
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    }

    // Start all consumers
    for tid in 0..(benchmark_consts::N_THREADS / 2) {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(tid + 1).unwrap());
        ThreadOptions::new({
            let completed = completed.clone();
            let completed_wq = completed_wq.clone();
            let consumer = q.attach_consumer().unwrap();
            move || {
                for _ in 0..benchmark_consts::N_MESSAGES_PER_THREAD {
                    let _ = consumer.consume();
                }
                completed.fetch_add(1, Ordering::Relaxed);
                completed_wq.wake_one();
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    }
}
