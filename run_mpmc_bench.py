import os

MPMCOQueue = "ostd::orpc::oqueue::ringbuffer::MPMCOQueue::<u64>::new(2 << 20, 0)"
RigtorpQueue = "ostd::orpc::oqueue::ringbuffer::mpmc::Rigtorp::<u64>::new(2 << 20)"


def setup(n_threads: int, queue: str, benchmark: str):
    benchmark_consts_rs = f"""\
// SPDX-License-Identifier: MPL-2.0
// DO NOT EDIT BY HAND! See run_mpmc_bench.py
use alloc::sync::Arc;

use ostd::orpc::oqueue::OQueue;
pub use super::benchmarks::{benchmark} as benchfn;

pub const N_THREADS: usize = {n_threads};
pub const N_MESSAGES_PER_THREAD: usize = 2 << 15;
pub const N_MESSAGES: usize = N_MESSAGES_PER_THREAD * N_THREADS;

pub fn get_oq() -> Arc<dyn OQueue<u64>> {{
    let q = {queue};
    assert!(q.capacity() >= N_MESSAGES);
    let q: Arc<dyn OQueue<u64>> = q;
    q
}}
"""

    with open("kernel/src/benchmark_consts.rs", "w") as f:
        f.write(benchmark_consts_rs)


thread_counts = [1, 2, 4, 8, 16, 32]
q_impls = {
    "mpmc_oq": MPMCOQueue,
    "rigtorp": RigtorpQueue,
}
benchmarks = [
    "mixed_bench",
    "consume_bench",
    "produce_bench",
    "weak_obs_bench",
]

for i in range(10):
    for q in q_impls:
        for benchmark in benchmarks:
            for tc in thread_counts:
                if tc < 2 and benchmark == "mixed_bench":
                    continue
                if tc < 4 and benchmark == "weak_obs_bench":
                    continue
                print(f"[RUN] {q=} {benchmark=} {tc=}")
                setup(tc, q_impls[q], benchmark)
                os.system(
                    f"RELEASE=1 make run 2>&1 | tee {q}_{benchmark}_throughput_{tc}_run_{i}.log"
                )
