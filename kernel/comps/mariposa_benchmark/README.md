# `mariposa_benchmark`

Benchmarks for Mariposa. Each benchmark lives in its own module; `framework.rs` holds what they
share. Every benchmark is compiled into every kernel build and does nothing unless enabled on the
kernel command line.

## `oqueue_roundtrip` — the OQFS kernel ↔ user round trip

Measures the latency of the kernel-triggered / userspace-served hot path over OQFS: a kernel thread
produces a request into an OQueue, a userspace process observes it, computes, writes a reply into a
second OQueue, and the kernel thread wakes on that reply. It is the same shape as the RAID-1
`raid.selection=userspace` policy, reduced to a bare ping-pong so the numbers are about the transport
and the scheduler rather than about RAID.

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

Every round trip is measured, however long: a sample is never discarded, capped, or substituted
because it is large. There is no warmup parameter — the early iterations are captured like any other,
so the analysis can see the warmup and decide how much of it to drop.

Any anomaly (a reply timeout, an out-of-sequence reply, a reply arriving before its request) ends the
run rather than being smoothed over. The kernel side never stops the machine, not even then: it writes
the reason to the console and tells the peer the run failed, and the peer's exit status is what `init`
acts on. Starting and stopping both belong to userspace; only the parameters come from the kernel
command line.

### Getting the results

Samples are buffered in memory during the run and written to the data capture device once it is over,
in the standard Mariposa capture format (see `kernel/comps/mariposa_data_capture`). Read them on the
host with `kernel/comps/mariposa_data_capture/python/mariposa_data_reader.py`. Convert cycles to
seconds with the TSC frequency printed in the console metadata block.

### Running

The host CLI is the intended interface; see [`tools/oqbench/README.md`](../../../tools/oqbench/README.md).

```
tools/oqbench/run.sh --iterations 2000000 --peer-compute 5000 --output result.csv
```

The always-on smoke test runs the whole pipeline at a small iteration count:

```
make run_kernel AUTO_TEST=oqbench ENABLE_KVM=1
```

### Command-line parameters

| parameter                  | meaning                                                     | default      |
|----------------------------|-------------------------------------------------------------|--------------|
| `oqbench.enable`           | master switch; inert unless present                          | off          |
| `oqbench.iterations`       | measured iterations                                          | 1000000      |
| `oqbench.timeout_ms`       | per-reply timeout (ms); a timeout ends the run as failed      | 10000        |
| `oqbench.request_capacity` | request OQueue capacity                                      | 2            |
| `oqbench.reply_capacity`   | reply OQueue capacity                                        | 2            |
| `oqbench.realtime`         | run the kernel thread under real-time scheduling              | off (normal) |
| `oqbench.rt_prio`          | real-time priority (`1..=99`) when `oqbench.realtime` is set  | 50           |
| `oqbench.peer_compute`     | the peer's synthetic work per request, in TSC cycles          | 0            |
| `oqbench.busy_procs`       | competing busy-loop processes, as scheduler contention        | 0            |

`oqbench.peer_compute` and `oqbench.busy_procs` are acted on in userspace by `init` and the peer; the
kernel registers them only so they are recognised parameters and appear in the reported
configuration.

- **`oqbench.peer_compute`** makes the peer spin for a fixed number of cycles between reading a
  request and writing its reply, inflating `compute`. The default of `0` isolates pure transport and
  scheduler cost.
- **`oqbench.busy_procs`** adds competing processes for the duration of the run. The default of `0`
  measures the idle best case; raise it to see how much of the wakeup latency is contention rather
  than fixed cost.
- **`oqbench.realtime` / `oqbench.rt_prio`** run the kernel thread under real-time scheduling,
  matching the RAID worker this hot path mirrors. The userspace peer's scheduling cannot be set from
  userspace on this kernel, so there is no knob for it.

### Caveats

- **Guest TSC under KVM is host-derived.** Both sides read the same guest TSC, so the four stamps are
  directly comparable, but the absolute frequency and host-side steal time are outside the guest's
  control.
- **The wakeup latencies are scheduler-dependent — that is the point.** `kernel_to_user` and
  `user_to_kernel` move with vCPU count, the kernel thread's scheduling policy, and competing load.
