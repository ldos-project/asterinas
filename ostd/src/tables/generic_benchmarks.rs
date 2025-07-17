//! A set of benchmarks for use with any table implementation.

use alloc::{sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    benchmark::{BenchmarkReporter, Timer},
    sync::WaitQueue,
    tables::{Consumer as _, Producer as _, Table},
    task::{Task, TaskOptions},
};

pub(crate) fn benchmark_streaming_produce_consume<T: Table<usize> + Sync + Send + 'static>(
    n_messages: usize,
    tables: &[Arc<T>],
) -> BenchmarkReporter {
    let done_count = Arc::new(AtomicUsize::new(0));
    let wait_queue = Arc::new(WaitQueue::new());
    let tasks: Vec<_> = tables
        .iter()
        .map(|table| {
            let producer_task = Arc::new(
                TaskOptions::new({
                    let table = table.clone();
                    let done_count = done_count.clone();
                    let wait_queue = wait_queue.clone();
                    move || {
                        let producer = table.attach_producer().unwrap();
                        for i in 0..n_messages {
                            producer.put(i);
                        }
                        done_count.fetch_add(1, Ordering::Relaxed);
                        wait_queue.wake_all();
                    }
                })
                .build()
                .unwrap(),
            );

            let consumer_task = Arc::new(
                TaskOptions::new({
                    let table = table.clone();
                    let done_count = done_count.clone();
                    let wait_queue = wait_queue.clone();
                    move || {
                        let consumer = table.attach_consumer().unwrap();
                        for expected_i in 0..n_messages {
                            let i = consumer.take();
                            assert_eq!(i, expected_i);
                        }
                        done_count.fetch_add(1, Ordering::Relaxed);
                        wait_queue.wake_all();
                    }
                })
                .build()
                .unwrap(),
            );
            [producer_task, consumer_task]
        })
        .flatten()
        .collect();

    let timer = Timer::start();

    tasks.iter().for_each(Task::run);

    wait_queue.wait_until(|| {
        let done_count = done_count.load(Ordering::Relaxed);
        if done_count == tasks.len() {
            Some(())
        } else {
            None
        }
    });

    let time = timer.end();

    let mut reporter = BenchmarkReporter::new();
    reporter.report("time", &(time / n_messages as u32).as_nanos());
    reporter
}

pub(crate) fn benchmark_call_return<T: Table<usize> + Sync + Send + 'static>(
    n_messages: usize,
    n_pairs: usize,
    new_table: impl Fn() -> Arc<T>,
) -> BenchmarkReporter {
    let done_count = Arc::new(AtomicUsize::new(0));
    let wait_queue = Arc::new(WaitQueue::new());
    let tasks: Vec<_> = (0..n_pairs)
        .map(|_| {
            let call_table = new_table();
            let return_table = new_table();

            let caller_task = Arc::new(
                TaskOptions::new({
                    let call_table = call_table.clone();
                    let return_table = return_table.clone();
                    let done_count = done_count.clone();
                    let wait_queue = wait_queue.clone();
                    move || {
                        let call_handle = call_table.attach_producer().unwrap();
                        let return_handle = return_table.attach_consumer().unwrap();
                        for i in 0..n_messages {
                            call_handle.put(i);
                            let res = return_handle.take();
                            assert_eq!(res, i + 1);
                        }
                        done_count.fetch_add(1, Ordering::Relaxed);
                        wait_queue.wake_all();
                    }
                })
                .build()
                .unwrap(),
            );

            let callee_task = Arc::new(
                TaskOptions::new({
                    let call_table = call_table.clone();
                    let return_table = return_table.clone();
                    let done_count = done_count.clone();
                    let wait_queue = wait_queue.clone();
                    move || {
                        let call_handle = call_table.attach_consumer().unwrap();
                        let return_handle = return_table.attach_producer().unwrap();
                        for expected_i in 0..n_messages {
                            let i = call_handle.take();
                            assert_eq!(i, expected_i);
                            return_handle.put(i + 1);
                        }
                        done_count.fetch_add(1, Ordering::Relaxed);
                        wait_queue.wake_all();
                    }
                })
                .build()
                .unwrap(),
            );
            [caller_task, callee_task]
        })
        .flatten()
        .collect();

    let timer = Timer::start();

    tasks.iter().for_each(Task::run);

    wait_queue.wait_until(|| {
        let done_count = done_count.load(Ordering::Relaxed);
        if done_count == tasks.len() {
            Some(())
        } else {
            None
        }
    });

    let time = timer.end();

    let mut reporter = BenchmarkReporter::new();
    reporter.report("n_pairs", &n_pairs);
    reporter.report("time", &(time / n_messages as u32).as_nanos());
    reporter
}
