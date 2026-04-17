# Kernel Benchmarks

Benchmarks are configured via `KCMDARGS`.

## General Arguments

| Argument | Description |
|---|---|
| `bench.benchmark` | Benchmark to run |
| `bench.n_threads` | Number of threads |
| `bench.n_repeat` | Number of repetitions |
| `bench.q_type` | Queue type: `mpmc_oq`, `locking`, or `rigtorp` (default) |

> **SMP**: Number of processors to use — always pass `32`.

## Legacy OQueue Benchmarks

| Benchmark | Arguments | Notes |
|---|---|---|
| `oqueue::produce_bench` | `n_threads`, `n_repeat`, `q_type` | |
| `oqueue::consume_bench` | `n_threads`, `n_repeat`, `q_type` | `n_threads` must be less than SMP |
| `oqueue::mixed_bench` | `n_threads`, `n_repeat`, `q_type` | `n_threads` must be even |
| `oqueue::weak_obs_bench` | `n_threads`, `n_repeat`, `q_type` | `n_threads` must be even |
| `oqueue::strong_obs_bench` | `n_threads`, `n_repeat`, `q_type` | `n_threads` must be even |

## Scaling Benchmarks

> `q_type` is not required for scaling benchmarks.

| Benchmark | Arguments |
|---|---|
| `oqueue_scaling::consumer` | `n_threads`, `n_repeat` |
| `oqueue_scaling::strong_obs` | `n_threads`, `n_repeat` |
| `oqueue_scaling::weak_obs` | `n_threads`, `n_repeat` |

## New OQueue Benchmarks

| Benchmark | Arguments |
|---|---|
| `produce_bench_new` | `n_threads`, `n_repeat` |
| `consume_bench_new` | `n_threads`, `n_repeat` |
| `mixed_bench_new` | `n_threads`, `n_repeat` |
| `weak_obs_bench_new` | `n_threads`, `n_repeat` |