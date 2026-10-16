# `tools/oqbench` — host driver for the OQFS round-trip microbenchmark

`run.py` boots the kernel with the OQFS round-trip microbenchmark enabled in a chosen scenario, waits
for the run to finish, and decodes the captured samples off the data capture image into JSON Lines
files. With `--scheduler` it also captures the kernel's scheduling events.

Run `tools/oqbench/run.py --help` for the options; that help text is the canonical reference.

See [`kernel/comps/mariposa_benchmark/README.md`](../../kernel/comps/mariposa_benchmark/README.md)
for what the benchmark measures and its caveats.

## Examples

```
# Quick local sanity run (writes result_oqbench.jsonl in the current directory):
tools/oqbench/run.py --iterations 50000

# Make the peer's own work non-trivial:
tools/oqbench/run.py --iterations 1000000 --peer-compute 5000

# Real-time kernel thread (mirroring the RAID worker) under scheduler contention built in release mode:
tools/oqbench/run.py --rt-prio 50 --busy-procs 16 --vcpus 4 --release

# Run with scheduler trace capture 5 busy procs in release mode:
tools/oqbench/run.py --scheduler --busy-procs 5 --release
```

## Output

Files are named `<PREFIX><base>` where `<PREFIX>` is `--output-prefix` (default `result_`, which is
what `howdone` copies into its run directories).

- `<PREFIX>oqbench.jsonl`: one JSON object per round trip, with a `timestamp` and the four
  TSC-cycle fields `roundtrip`, `kernel_to_user`, `compute` and `user_to_kernel`. Divide the
  TSC fields by the TSC frequency (printed in the console metadata block, which the script
  echoes to stderr) for seconds.
- `<PREFIX>scheduler_events.jsonl` (with `--scheduler`): one JSON object per scheduling event,
  also with a `timestamp`.

## Decoding a capture manually

`run.py` decodes for you, but the capture image is a normal Mariposa capture device, so you can also
read it directly:

```
python3 kernel/core/comps/mariposa_data_capture/python/decode_mariposa_data.py \
    test/initramfs/build/capture.img --output-dir .
```

