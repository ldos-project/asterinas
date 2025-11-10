import os

MPMCOQueue = "ostd::orpc::oqueue::ringbuffer::MPMCOQueue::<u64>::new(2 << 20, 0)"
RigtorpQueue = "ostd::orpc::oqueue::ringbuffer::mpmc::Rigtorp::<u64>::new(2 << 20)"


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
    "strong_obs_bench",
]

for i in range(1):
    for q in q_impls:
        for benchmark in benchmarks:
            for tc in thread_counts:
                if tc < 2 and benchmark == "mixed_bench":
                    continue
                if tc < 4 and benchmark in ["weak_obs_bench", "strong_obs_bench"]:
                    continue
                print(f"[RUN] {q=} {benchmark=} {tc=}")
                bench_extra_args = " ".join(
                    [
                        f"--kcmd-args='bench.n_threads={tc}'",
                        f"--kcmd-args='bench.q_type={q}'",
                        f"--kcmd-args='bench.benchmark={benchmark}'",
                    ]
                )
                cmd = f'RELEASE=1 BENCH_EXTRA_ARGS="{bench_extra_args}" make run 2>&1 | tee {q}_{benchmark}_throughput_{tc}_run_{i}.log'
                os.system(cmd)
