#![no_std]
#![deny(unsafe_code)]

use ostd::{
    arch::qemu::exit_qemu,
    prelude::*,
    tables::{locking, spsc},
    task::{Task, TaskOptions},
};

#[ostd::main]
fn kernel_main() {
    TaskOptions::new(|| {
        println!("Starting benchmarks.");
        locking::test::test_produce_weak_observe();
        spsc::benchmark::benchmark_streaming_produce_consume();
        spsc::benchmark::benchmark_call_return_wait_types();
        locking::benchmark::benchmark_streaming_produce_consume();
        locking::benchmark::benchmark_call_return();
        println!("Completed benchmarks successfully.");
        exit_qemu(ostd::arch::qemu::QemuExitCode::Success);
    })
    .spawn();
}
