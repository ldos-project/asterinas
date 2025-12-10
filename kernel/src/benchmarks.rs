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
    bc: &crate::benchmark_consts::BenchConsts,
    q: &Arc<dyn OQueue<u64>>,
    completed: &Arc<AtomicUsize>,
    completed_wq: &Arc<ostd::sync::WaitQueue>,
) -> usize {
    println!("Starting producers");
    let barrier = Arc::new(AtomicUsize::new(bc.n_threads));
    // Start all producers
    for tid in 0..bc.n_threads {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(tid + 1).unwrap());
        ThreadOptions::new({
            let barrier = barrier.clone();
            let completed = completed.clone();
            let completed_wq = completed_wq.clone();
            let producer = q.attach_producer().unwrap();
            move || {
                barrier.fetch_sub(1, Ordering::Acquire);
                while barrier.load(Ordering::Relaxed) > 0 {}
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

    bc.n_threads
}

#[allow(dead_code)]
pub fn consume_bench(
    bc: &crate::benchmark_consts::BenchConsts,
    q: &Arc<dyn OQueue<u64>>,
    completed: &Arc<AtomicUsize>,
    completed_wq: &Arc<ostd::sync::WaitQueue>,
) -> usize {
    // Initialize consumer so that the producer knows to retain values
    let consumer = q.attach_consumer().unwrap();
    println!("Populating queue");
    for tid in 0..bc.n_threads {
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
    completed_wq.wait_until(|| (completed.load(Ordering::Relaxed) == bc.n_threads).then_some(()));
    completed.store(0, Ordering::Relaxed);

    let barrier = Arc::new(AtomicUsize::new(bc.n_threads));
    // Start all consumers
    for tid in 0..bc.n_threads {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(tid + 1).unwrap());
        ThreadOptions::new({
            let completed = completed.clone();
            let completed_wq = completed_wq.clone();
            let consumer = q.attach_consumer().unwrap();
            let barrier = barrier.clone();
            move || {
                barrier.fetch_sub(1, Ordering::Acquire);
                while barrier.load(Ordering::Relaxed) > 0 {}
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
    drop(consumer);
    bc.n_threads
}
#[allow(dead_code)]
pub fn mixed_bench(
    bc: &crate::benchmark_consts::BenchConsts,
    q: &Arc<dyn OQueue<u64>>,
    completed: &Arc<AtomicUsize>,
    completed_wq: &Arc<ostd::sync::WaitQueue>,
) -> usize {
    let n_threads_per_type: usize = bc.n_threads / 2;
    let barrier = Arc::new(AtomicUsize::new(bc.n_threads));

    // Start all producers
    for tid in 0..n_threads_per_type {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(tid + 1).unwrap());
        ThreadOptions::new({
            let completed = completed.clone();
            let completed_wq = completed_wq.clone();
            let producer = q.attach_producer().unwrap();
            let barrier = barrier.clone();
            move || {
                barrier.fetch_sub(1, Ordering::Acquire);
                while barrier.load(Ordering::Relaxed) > 0 {}
                for _ in 0..(2 * benchmark_consts::N_MESSAGES_PER_THREAD) {
                    producer.produce(0);
                }
                crate::prelude::println!("finished producer {}", tid);
                completed.fetch_add(1, Ordering::Relaxed);
                completed_wq.wake_one();
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    }

    // Start all consumers
    for tid in 0..n_threads_per_type {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(n_threads_per_type + tid + 1).unwrap());
        ThreadOptions::new({
            let completed = completed.clone();
            let completed_wq = completed_wq.clone();
            let consumer = q.attach_consumer().unwrap();
            let barrier = barrier.clone();
            move || {
                barrier.fetch_sub(1, Ordering::Acquire);
                while barrier.load(Ordering::Relaxed) > 0 {}
                for _ in 0..(2 * benchmark_consts::N_MESSAGES_PER_THREAD) {
                    let _ = consumer.consume();
                }
                crate::prelude::println!("finished consumer {}", tid);
                completed.fetch_add(1, Ordering::Relaxed);
                completed_wq.wake_one();
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    }
    bc.n_threads
}

#[allow(dead_code)]
pub fn weak_obs_bench(
    bc: &crate::benchmark_consts::BenchConsts,
    q: &Arc<dyn OQueue<u64>>,
    completed: &Arc<AtomicUsize>,
    completed_wq: &Arc<ostd::sync::WaitQueue>,
) -> usize {
    let n_threads_per_type: usize = bc.n_threads / 2;
    let barrier = Arc::new(AtomicUsize::new(bc.n_threads));

    // Start all producers
    for tid in 0..n_threads_per_type {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(tid + 1).unwrap());
        ThreadOptions::new({
            let completed = completed.clone();
            let completed_wq = completed_wq.clone();
            let producer = q.attach_producer().unwrap();
            let barrier = barrier.clone();
            move || {
                barrier.fetch_sub(1, Ordering::Acquire);
                while barrier.load(Ordering::Relaxed) > 0 {}
                for _ in 0..(2 * benchmark_consts::N_MESSAGES_PER_THREAD) {
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
    let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
    cpu_set.add(ostd::cpu::CpuId::try_from(n_threads_per_type + 1).unwrap());
    ThreadOptions::new({
        let completed = completed.clone();
        let completed_wq = completed_wq.clone();
        let consumer = q.attach_consumer().unwrap();
        let barrier = barrier.clone();
        move || {
            barrier.fetch_sub(1, Ordering::Acquire);
            while barrier.load(Ordering::Relaxed) > 0 {}
            for _ in 0..(2 * benchmark_consts::N_MESSAGES_PER_THREAD) {
                let _ = consumer.consume();
            }
            completed.fetch_add(1, Ordering::Relaxed);
            completed_wq.wake_one();
        }
    })
    .cpu_affinity(cpu_set)
    .spawn();

    if bc.q_type == "mpmc_oq" || bc.q_type == "locking" {
        // Start all weak observers
        for tid in 0..(n_threads_per_type.wrapping_sub(1)) {
            let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
            cpu_set.add(ostd::cpu::CpuId::try_from(n_threads_per_type + tid + 2).unwrap());
            ThreadOptions::new({
                let completed = completed.clone();
                let completed_wq = completed_wq.clone();
                let weak_observer = q.attach_weak_observer().unwrap();
                let barrier = barrier.clone();
                move || {
                    barrier.fetch_sub(1, Ordering::Acquire);
                    while barrier.load(Ordering::Relaxed) > 0 {}
                    let mut cnt = 0;
                    for _ in 0..(2 * benchmark_consts::N_MESSAGES_PER_THREAD) {
                        cnt += weak_observer.weak_observe_recent(1).len();
                    }
                    crate::prelude::println!("weak observed {} values", cnt);
                    completed.fetch_add(1, Ordering::Relaxed);
                    completed_wq.wake_one();
                }
            })
            .cpu_affinity(cpu_set)
            .spawn();
        }
    } else {
        for tid in 0..(n_threads_per_type.wrapping_sub(1)) {
            let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
            cpu_set.add(ostd::cpu::CpuId::try_from(n_threads_per_type + tid + 2).unwrap());
            ThreadOptions::new({
                let completed = completed.clone();
                let completed_wq = completed_wq.clone();
                let weak_observer = q.attach_consumer().unwrap();
                let barrier = barrier.clone();
                move || {
                    barrier.fetch_sub(1, Ordering::Acquire);
                    while barrier.load(Ordering::Relaxed) > 0 {}
                    let mut cnt = 0;
                    for _ in 0..(2 * benchmark_consts::N_MESSAGES_PER_THREAD) {
                        cnt += weak_observer.consume();
                    }
                    crate::prelude::println!("weak observed {} values", cnt);
                    completed.fetch_add(1, Ordering::Relaxed);
                    completed_wq.wake_one();
                }
            })
            .cpu_affinity(cpu_set)
            .spawn();
        }
    }

    bc.n_threads
}

#[allow(dead_code)]
pub fn strong_obs_bench(
    bc: &crate::benchmark_consts::BenchConsts,
    q: &Arc<dyn OQueue<u64>>,
    completed: &Arc<AtomicUsize>,
    completed_wq: &Arc<ostd::sync::WaitQueue>,
) -> usize {
    let n_threads_per_type: usize = bc.n_threads / 2;
    let barrier = Arc::new(AtomicUsize::new(bc.n_threads));

    // Start all producers
    for tid in 0..n_threads_per_type {
        let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
        cpu_set.add(ostd::cpu::CpuId::try_from(tid + 1).unwrap());
        ThreadOptions::new({
            let completed = completed.clone();
            let completed_wq = completed_wq.clone();
            let producer = q.attach_producer().unwrap();
            let barrier = barrier.clone();
            move || {
                barrier.fetch_sub(1, Ordering::Acquire);
                while barrier.load(Ordering::Relaxed) > 0 {}
                crate::prelude::println!("producer start");
                for _ in 0..(2 * benchmark_consts::N_MESSAGES_PER_THREAD) {
                    producer.produce(0);
                }
                crate::prelude::println!("producer stop");
                completed.fetch_add(1, Ordering::Relaxed);
                completed_wq.wake_one();
            }
        })
        .cpu_affinity(cpu_set)
        .spawn();
    }

    // Start all consumers
    let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
    cpu_set.add(ostd::cpu::CpuId::try_from(n_threads_per_type + 1).unwrap());
    ThreadOptions::new({
        let completed = completed.clone();
        let completed_wq = completed_wq.clone();
        let consumer = q.attach_consumer().unwrap();
        let barrier = barrier.clone();
        move || {
            barrier.fetch_sub(1, Ordering::Acquire);
            // ostd::task::Task::yield_now();
            while barrier.load(Ordering::Relaxed) > 0 {}
            crate::prelude::println!("consumer start");
            for _ in 0..(2 * benchmark_consts::N_MESSAGES_PER_THREAD) {
                let _ = consumer.consume();
            }
            crate::prelude::println!("consumer stop");
            completed.fetch_add(1, Ordering::Relaxed);
            completed_wq.wake_one();
        }
    })
    .cpu_affinity(cpu_set)
    .spawn();

    if bc.q_type == "mpmc_oq" || bc.q_type == "locking" {
        // Start all consumers
        for tid in 0..(n_threads_per_type.wrapping_sub(1)) {
            let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
            cpu_set.add(ostd::cpu::CpuId::try_from(n_threads_per_type + 2 + tid).unwrap());
            ThreadOptions::new({
                let completed = completed.clone();
                let completed_wq = completed_wq.clone();
                let strong_observer = q.attach_strong_observer().unwrap();
                let barrier = barrier.clone();
                move || {
                    barrier.fetch_sub(1, Ordering::Acquire);
                    while barrier.load(Ordering::Relaxed) > 0 {}
                    crate::prelude::println!("observer start");
                    let mut cnt = 0;
                    for _ in 0..(2 * benchmark_consts::N_MESSAGES_PER_THREAD) {
                        strong_observer.strong_observe();
                        cnt += 1;
                    }
                    crate::prelude::println!("strong observed {} values", cnt);
                    completed.fetch_add(1, Ordering::Relaxed);
                    completed_wq.wake_one();
                }
            })
            .cpu_affinity(cpu_set)
            .spawn();
        }
    } else {
        for tid in 0..(n_threads_per_type.wrapping_sub(1)) {
            let mut cpu_set = ostd::cpu::set::CpuSet::new_empty();
            cpu_set.add(ostd::cpu::CpuId::try_from(n_threads_per_type + 2 + tid).unwrap());
            ThreadOptions::new({
                let completed = completed.clone();
                let completed_wq = completed_wq.clone();
                let strong_observer = q.attach_consumer().unwrap();
                let barrier = barrier.clone();
                move || {
                    barrier.fetch_sub(1, Ordering::Acquire);
                    while barrier.load(Ordering::Relaxed) > 0 {}
                    let mut cnt = 0;
                    for _ in 0..(2 * benchmark_consts::N_MESSAGES_PER_THREAD) {
                        cnt += strong_observer.consume();
                    }
                    crate::prelude::println!("strong observed {} values", cnt);
                    completed.fetch_add(1, Ordering::Relaxed);
                    completed_wq.wake_one();
                }
            })
            .cpu_affinity(cpu_set)
            .spawn();
        }
    }
    bc.n_threads
}
