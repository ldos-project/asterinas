# `tools/oqbench` — host driver for the OQFS round-trip microbenchmark

`run.sh` boots the kernel with the OQFS round-trip microbenchmark enabled in a chosen scenario, waits
for the run to finish, and decodes the captured samples off the data capture image into a JSON Lines
file. Like the other in-kernel microbenchmarks, the run is configured entirely from the kernel
command line and the kernel stops the machine when it is done.

Run `tools/oqbench/run.sh --help` for the options; that help text is the canonical reference.

See [`kernel/comps/mariposa_benchmark/README.md`](../../kernel/comps/mariposa_benchmark/README.md)
for what the benchmark measures and its caveats.

## Examples

```
# Quick local sanity run:
tools/oqbench/run.sh --iterations 50000 --output quick.jsonl

# Make the peer's own work non-trivial:
tools/oqbench/run.sh --iterations 1000000 --peer-compute 5000 --output compute.jsonl

# Real-time kernel thread (mirroring the RAID worker) under scheduler contention:
tools/oqbench/run.sh --rt-prio 50 --busy-procs 16 --vcpus 4 --output loaded.jsonl
```

## Output

One JSON object per round trip, with the four TSC-cycle fields `roundtrip`, `kernel_to_user`,
`compute` and `user_to_kernel`. Divide by the TSC frequency (printed in the console metadata block,
which the script echoes to stderr) for seconds. All host-side aggregation — binning, percentiles,
discarding warmup iterations — is done from this complete data.

## Decoding a capture manually

`run.sh` decodes for you, but the capture image is a normal Mariposa capture device, so you can also
read it directly:

```
python3 kernel/comps/mariposa_data_capture/python/decode_mariposa_data.py \
    test/initramfs/build/capture.img --output-dir .
```

## Smoke test

The always-compiled smoke target exercises the whole pipeline at a small iteration count without the
CLI, and makes the build fail if the round trip does not complete:

```
make run_kernel AUTO_TEST=oqbench ENABLE_KVM=1
```
