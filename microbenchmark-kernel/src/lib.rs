#![no_std]
#![deny(unsafe_code)]

use ostd::{
    arch::qemu::exit_qemu,
    prelude::*,
    tables::spsc,
    task::{Task, TaskOptions},
};

#[ostd::main]
fn kernel_main() {
    TaskOptions::new(|| {
        println!("Starting benchmarks.");
        spsc::benchmark::benchmark_streaming_produce_consume();
        spsc::benchmark::benchmark_call_return_wait_types();
        println!("Completed benchmarks successfully.");
        exit_qemu(ostd::arch::qemu::QemuExitCode::Success);
    })
    .spawn();
}
