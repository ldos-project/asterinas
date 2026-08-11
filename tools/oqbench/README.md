# `tools/oqbench` — host driver for the OQFS round-trip microbenchmark

`run.sh` builds and boots the Asterinas kernel with the OQFS round-trip microbenchmark enabled in a
chosen scenario, waits for the run to complete, **fetches the results file from the guest over
`scp`**, verifies the sample count, and then shuts the guest down.

The samples are not in the console output: after the run the kernel streams every sample to the
userspace peer over an OQueue and the peer writes them to `/tmp/oqbench-samples.csv` in the guest;
the console carries only a small metadata block. This script's job is therefore boot → wait → fetch
→ shut down. It needs an SSH key (it generates `~/.ssh/id_ed25519` if none exists) because the guest
authorizes `~/.ssh/*.pub` and the fetch logs in over the initramfs's dropbear/sftp-server.

See [`kernel/comps/oqueue_roundtrip_bench/README.md`](../../kernel/comps/oqueue_roundtrip_bench/README.md)
for what the benchmark measures (the four timestamps, the wire format, the sample array, the dump)
and the caveats.

## Usage

```
tools/oqbench/run.sh [OPTIONS]
```

Run `tools/oqbench/run.sh --help` for the full option list. The common ones:

| flag                       | meaning                                                  | default   |
|----------------------------|----------------------------------------------------------|-----------|
| `--iterations <N>`         | measured iterations (must be > 0)                        | 1000000   |
| `--warmup <N>`             | warmup iterations, excluded from the samples             | 10000     |
| `--peer-compute <N>`       | userspace peer's synthetic work per request (iterations)  | 0         |
| `--timeout-ms <N>`         | per-reply timeout in ms; a timeout is fatal (stops guest) | 10000     |
| `--request-capacity <N>`   | request OQueue capacity (must be > 0)                    | 2         |
| `--reply-capacity <N>`     | reply OQueue capacity (must be > 0)                      | 2         |
| `--sched <normal\|realtime>` | driver-thread scheduling policy                        | normal    |
| `--rt-prio <N>`            | real-time priority (1..=99) for `--sched realtime`       | 50        |
| `--busy-procs <N>`         | competing busy-loop processes (0 = none)                | 0         |
| `--vcpus <N>`              | guest vCPU count / SMP (must be > 0)                     | 1         |
| `--kvm <on\|off>`          | KVM acceleration                                         | on        |
| `--output <FILE>`          | CSV sample file to write                                | `oqbench-samples.csv` |

There is **no flag for the userspace peer's scheduling**: it cannot be set from userspace on this
kernel. Only the kernel driver thread's scheduling is configurable, via `--sched` / `--rt-prio`.

The script **refuses nonsense arguments** up front and **exits non-zero with a clear message** if the
boot fails, the run did not complete, the fetch fails, or the fetched sample count does not match the
run's reported `measured`. It writes the output file only once that count matches, so it never leaves
a silently truncated result. The guest is always powered off on exit, including on failure.

The guest is reached on a forwarded SSH port (a random high port by default; override with the
`SSH_PORT` environment variable). Host key checking is disabled because the guest regenerates its
host keys on each boot.

## Examples

```
# Quick local sanity run (fast, KVM):
tools/oqbench/run.sh --iterations 50000 --warmup 5000 --output quick.csv

# Add userspace peer compute so the `compute` interval is non-trivial:
tools/oqbench/run.sh --iterations 1000000 --peer-compute 2000 --output compute.csv

# Real-time driver thread (mirroring the RAID worker) under scheduler contention:
tools/oqbench/run.sh --sched realtime --rt-prio 50 --busy-procs 16 --vcpus 4 --output loaded.csv
```

## Output

The output is **CSV**: one sample per line, four TSC-cycle fields
`roundtrip,kernel_to_user,compute,user_to_kernel`. Divide by the TSC frequency (printed in the
console metadata block, which the script echoes to stderr) for seconds. Any host-side aggregation
(binning, percentiles, log-scale views) is done from this complete data.

## Retrieving samples manually

`run.sh` fetches for you, but you can also copy the file yourself while a guest booted with
`oqbench.await_fetch` is still alive (the peer writes it to `/tmp/oqbench-samples.csv`):

```
scp -P <forwarded-port> root@localhost:/tmp/oqbench-samples.csv .
```

## Smoke test

The always-compiled smoke target exercises the whole pipeline at a small iteration count without the
CLI, and makes the build fail if the round trip does not complete:

```
make run_kernel AUTO_TEST=oqbench ENABLE_KVM=1
```
