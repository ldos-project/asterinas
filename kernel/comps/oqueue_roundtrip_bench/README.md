# OQFS round-trip microbenchmark (`aster-oqueue-roundtrip-bench`)

This component measures the latency of the kernel-triggered / userspace-served hot path over OQFS:
a kernel thread produces a request into an OQueue, a userspace process observes it, computes, writes
a reply into a second OQueue, and the kernel thread wakes on that reply. It is the same shape as the
RAID-1 `raid.selection=userspace` policy, reduced to a bare ping-pong so the numbers are about the
transport and the scheduler rather than about RAID.

It is compiled into **every** kernel build and does nothing unless enabled on the kernel command
line with `oqbench.enable`.

Every measured sample is stored verbatim and, after the run, streamed to the userspace peer over an
OQueue (no in-kernel bucketing or percentiles); the peer writes them to a plain-text results file in
the guest. Any anomaly is fatal: it prints a distinctive `OQBENCH|error` line and powers the machine
off with a failure exit code (a plain `panic!` would only be a per-thread oops that hangs the boot).

## The four timestamps

Each iteration captures four timestamps on a single shared TSC time base (the guest TSC, read by
`ostd::arch::read_tsc()` in the kernel and `rdtsc` in userspace):

```
t0  kernel: immediately before producing the request
t1  user:   immediately after the request read returns
t2  user:   immediately before writing the reply
t3  kernel: immediately after the reply is consumed
```

The reply payload carries `t1` and `t2` (and the sequence number) back to the kernel, so the round
trip decomposes into:

```
t3-t0  full round trip
t1-t0  kernel -> user wakeup latency   (the scheduler waking the userspace peer)
t2-t1  userspace compute
t3-t2  user -> kernel wakeup latency   (the scheduler waking the kernel driver)
```

## Wire format

Three OQueues live on a dedicated OQFS subtree, created and exported by this component:

- `/oqueues/oqbench/request/` — kernel → user, registered with `registry::register` so it surfaces
  as `strong_observe`. Each request is a single CBOR unsigned integer: the sequence number. A final
  request of `u64::MAX` (the sentinel) marks the end of measurement and hands over to the dump.
- `/oqueues/oqbench/reply/` — user → kernel, registered with `registry::register_producible` so it
  surfaces as `produce`. Each reply is a fixed 3-element CBOR array `[seq, t1, t2]` (matching the
  kernel's `[u64; 3]` reply type). After measurement the peer reuses it for dump acks `[consumed, 0, 0]`.
- `/oqueues/oqbench/dump/` — kernel → user, `strong_observe` again. Used only after the run to carry
  a header `[count, 0, 0, 0]` and then `count` sample records `[u64; 4]`.

## Sample storage and the dump

Before measuring, the driver preallocates a single array holding **every** measured (post-warmup)
sample — four `u64` = 32 bytes each. Recording a sample on the hot path is a single indexed store.
The requested byte size is printed at startup (`OQBENCH|alloc sample_bytes=… iterations=…`); a failed
allocation stops the machine.

After the measurement loop finishes — **strictly after** — the driver streams the array to the peer
over the dump OQueue: the sentinel breaks the peer out of its reply loop, then the header and every
sample record flow to the peer, which writes each as one line to its results file. The `strong_observe`
export drops a reader that falls behind, so the peer paces the kernel with an ack `[consumed, 0, 0]`
every 1024 samples; the kernel never runs further than one dump window ahead and the reader is never
dropped. The peer's **final** ack is sent only after it has written and flushed the whole file, so it
certifies a complete transfer. A missing or short ack (kernel side) or an early end of the dump
stream (peer side) is fatal — a truncated dump is never mistaken for a whole one.

The transfer time is reported as `dump_ms` in the console block. Post-processing (binning,
percentiles, log-scale views) happens on the **host**.

### Retrieving the samples

The peer writes the results to `/tmp/oqbench-samples.csv` in the guest, one sample per line as four
TSC-cycle fields `roundtrip,kernel_to_user,compute,user_to_kernel` (the field order is authoritative
in the `Interval` enum in `lib.rs`). Divide by `tsc_freq_hz` from the console block for seconds.
`tools/oqbench/run.sh` boots the guest, waits for the run, and fetches that file over `scp`.

## Console output

When the run finishes the kernel driver prints one self-delimiting **metadata** block to the console
(and hence `qemu.log`) — no sample data, only what is needed to interpret the results file. Every
line is prefixed with `OQBENCH|` so the block survives interleaved kernel logging. The block
contains:

```
OQBENCH|begin v1
OQBENCH|tsc_freq_hz <hz>                     # convert cycles -> ns/s with this
OQBENCH|config iterations=.. warmup=.. timeout_ms=.. request_capacity=.. reply_capacity=.. peer_compute=.. sched=<normal|realtime> rt_prio=..
OQBENCH|scenario busy_procs=..
OQBENCH|counts measured=.. warmup=..
OQBENCH|samples count=.. fields=roundtrip,kernel_to_user,compute,user_to_kernel dump_ms=..
OQBENCH|end
OQBENCH: run complete (<n> iterations)       # the last line; the smoke test greps for this
```

Reaching the end of the loop **is** success: every failure mode instead stops the machine with a
failure exit code (printing an `OQBENCH|error` line first), so the success marker is emitted
unconditionally once the loop finishes.

## Running

The host CLI is the intended interface; see [`tools/oqbench/README.md`](../../../tools/oqbench/README.md):

```
tools/oqbench/run.sh --iterations 2000000 --peer-compute 500 --output result.csv
tools/oqbench/run.sh --sched realtime --rt-prio 50 --busy-procs 16 --vcpus 4 --output loaded.csv
```

The always-on smoke test runs the whole pipeline at a small iteration count:

```
make run_kernel AUTO_TEST=oqbench ENABLE_KVM=1
```

### Command-line parameters

| parameter                  | meaning                                                    | default   |
|----------------------------|------------------------------------------------------------|-----------|
| `oqbench.enable`           | master switch; inert unless present                        | off       |
| `oqbench.iterations`       | measured iterations                                        | 1000000   |
| `oqbench.warmup`           | warmup iterations, excluded from the samples              | 10000     |
| `oqbench.timeout_ms`       | per-reply timeout (ms); a timeout is fatal (stops machine) | 10000     |
| `oqbench.request_capacity` | request OQueue capacity                                    | 2         |
| `oqbench.reply_capacity`   | reply OQueue capacity                                      | 2         |
| `oqbench.realtime`         | run the driver thread under real-time scheduling          | off (normal) |
| `oqbench.rt_prio`          | real-time priority (`1..=99`) when `oqbench.realtime` set | 50        |
| `oqbench.peer_compute`     | userspace peer's synthetic work per request (iterations)  | 0         |
| `oqbench.busy_procs`       | number of competing busy-loop processes                  | 0         |

### The two workload knobs

Neither `oqbench.peer_compute` nor `oqbench.busy_procs` is acted on by the **kernel**: both are
consumed in userspace — `init` reads them from `/proc/cmdline` (and passes `peer_compute` on to the
peer) — and the kernel registers them only so they are recognised parameters and can be echoed into
the result block for provenance.

- **`oqbench.peer_compute`** makes the userspace peer burn a controlled amount of work between reading
  a request and writing its reply (inflating the `compute` interval, `t2-t1`). Its default of `0`
  isolates pure transport + scheduler cost; raise it to model a peer that computes something.
- **`oqbench.busy_procs`** starts N busy-loop processes for the duration of the run. Its default of
  `0` measures the idle best case; raise it to add scheduler contention and see how much of the
  wakeup latency is contention rather than fixed cost.

`oqbench.realtime` / `oqbench.rt_prio` run the driver thread under real-time scheduling, matching the
RAID worker this hot path mirrors (`kernel/src/device/registry/raid.rs`), so a run can measure both
the normal and real-time combinations. **The userspace peer's scheduling cannot be set from userspace
on this kernel**, so there is no knob for it.

## Anomalies are fatal

Each of these prints a distinctive `OQBENCH|error` line and powers the machine off with a failure
exit code:

- **Timeout.** A per-reply timeout is a deadlock: the run stops naming the sequence number and how
  long it waited.
- **Out-of-sequence reply.** A reply whose sequence number is not the one just sent means the
  transport or peer is broken: the run stops naming the expected and received sequence numbers.
- **Stale reply.** Anything consumable before the request is even produced means something is broken:
  the run stops.
- **Every round trip is measured, however long.** A sample is never discarded, capped, or substituted
  because it is large.

## Caveats

- **Guest TSC under KVM is host-derived.** Both sides read the same guest TSC, so the four stamps are
  directly comparable, but the absolute frequency and host-side steal time are outside the guest's
  control. `tsc_freq_hz` is reported for ns conversion.
- **The wakeup latencies are scheduler-dependent — that is the point.** `t1-t0` and `t3-t2` move with
  vCPU count, the driver-thread scheduling policy (`--sched`), and competing load (`--busy-procs`).
