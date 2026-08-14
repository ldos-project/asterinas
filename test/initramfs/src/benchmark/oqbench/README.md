# `oqbench` — the OQFS kernel ↔ user round-trip microbenchmark

Guest side of the benchmark implemented in
[`kernel/comps/mariposa_benchmark`](../../../../../kernel/comps/mariposa_benchmark/README.md). The
kernel drives the measurement; `run.sh` only starts the userspace peer (and any competing load) and
returns when the kernel has captured every sample.

Run it through the host driver, which sets the `oqbench.*` kernel parameters and decodes the results:

```
tools/oqbench/run.sh --iterations 1000000 --output samples.jsonl
```

## Why there is no `bench_result.yaml`

Unlike the other suites here, this job is **not** drivable by `bench_linux_and_aster.sh`:

- It is Mariposa-only. OQueues do not exist on Linux, so there is no Linux side to compare against.
- It produces a distribution — one record per round trip, written to the data capture device — rather
  than a single scalar on stdout for `result_extraction` to match with a regex.

It follows the rest of the convention (a job directory with a `run.sh` that the guest runner invokes
via `BENCHMARK=oqbench/roundtrip`), so it is driven with `make run_kernel` directly.
