# `mariposa_benchmark`

Benchmarks for Mariposa. Each benchmark lives in its own module; `framework.rs` holds what they
share — collecting a sample per iteration, capturing the samples, reporting the run, and giving up on
it.

## `oqueue_roundtrip` — the OQFS kernel <-> user round trip

Measures the latency of the kernel-triggered / userspace-served hot path over OQFS: a kernel thread
produces a request into an OQueue, a userspace process observes it, computes, writes a reply into a
reply OQueue, and the kernel thread wakes on that reply.

### What it measures

Each iteration captures four timestamps:

```
t0  kernel: immediately before producing the request
t1  user:   immediately after the request read returns
t2  user:   immediately before writing the reply
t3  kernel: immediately after the reply is consumed
```

The reply carries `t1` and `t2` back to the kernel, so the round trip decomposes into four intervals,
in TSC cycles:

| field            | interval | meaning                                    |
|------------------|----------|--------------------------------------------|
| `roundtrip`      | `t3-t0`  | the full round trip                        |
| `kernel_to_user` | `t1-t0`  | the scheduler waking the userspace peer     |
| `compute`        | `t2-t1`  | the peer's own work                        |
| `user_to_kernel` | `t3-t2`  | the scheduler waking the kernel thread again |

Any anomaly (a reply timeout, an out-of-sequence reply, a reply arriving before its request) ends the
benchmark immediately.

When the peer is ready, it signals the kernel benchmarking thread via a control OQueue.

### Getting the results

Samples are buffered in memory during the run and written to the data capture device once it is over,
in the standard Mariposa capture format (see `kernel/comps/mariposa_data_capture`). See
[`tools/oqbench/README.md`](../../../tools/oqbench/README.md) for reading them back.

### Running

The host CLI is the intended interface; see [`tools/oqbench/README.md`](../../../tools/oqbench/README.md).

The always-on smoke test runs the whole pipeline at a small iteration count:

```
make run_kernel AUTO_TEST=oqbench
```

### Command-line parameters

Every parameter is `oqbench.<name>` on the kernel command line, and `oqbench.enable` is the master
switch — the benchmark is inert without it. `tools/oqbench/run.py --help` describes the rest, and a
run's own `MARIPOSA_BENCH|config` line reports the values it actually used.

### Caveats

- **Guest TSC under KVM is host-derived.** Both sides read the same guest TSC, so the four stamps are
  directly comparable, but the absolute frequency and host-side steal time are outside the guest's
  control.
