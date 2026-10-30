# Mariposa: observing the kernel

You are writing a policy for Mariposa, a research kernel built on Asterinas (a Linux-compatible
kernel in Rust, `no_std`). Mariposa components publish their events and internal state on
**OQueues** (observable queues). A policy can observe OQueues **at runtime** to make its decisions,
and you can **capture** OQueues during a benchmark run to analyze them offline in your turn. This
document covers both, for every subsystem; the subsystem context after it says what this policy's
interface provides directly.

## OQueues

An OQueue is a typed ring of events with a path name (e.g. `scheduler.events`); the API is in
`ostd::orpc::oqueue`. Producers publish events; observers read them through a **query**, a function
that projects each event to a small `Copy + Send` value. Two kinds of observers:

- **Weak observer** (use this in policies): keeps the most recent `history_len` projected values.
  Reading never blocks or slows the producer; a value overwritten before you read it shows as `None`.
- **Strong observer**: receives every value, but a slow strong observer makes producers wait or drop
  events. Do not use one from a policy.

### Finding an OQueue: the registry

Every named OQueue is registered when it is created. `ostd::orpc::oqueue::registry` looks them up:

- `lookup_by_path::<D>(&path!(scheduler.events))` -> `Option<GenericAnyOQueueRef<D>>`
- `lookup_by_path_pattern::<D>(&pattern)` -> all OQueues whose path matches
- `lookup_by_type::<D>()` -> all OQueues of an element type (e.g. one per block device)

`D` is the element's descriptor, `<T as Element>::Descriptor`; for a `Copy` element type `T` that is
`LifetimelessElementDescriptor<T>`. A lookup only finds OQueues that already exist, so it can fail if
your code runs before the producing component is initialized.

### Observing at runtime

```rust
use ostd::{orpc::oqueue::{registry, LifetimelessElementDescriptor, ObservationQuery, OQueueBase, WeakObserver}, path};
use aster_block::bio::BlockDeviceCompletionStats;

// Once, where allocation is allowed (e.g. in your constructor):
let observers: Vec<WeakObserver<(u32, u64)>> = registry::lookup_by_type::<LifetimelessElementDescriptor<BlockDeviceCompletionStats>>()
    .into_iter()
    .filter_map(|q| q.attach_weak_observer(16, ObservationQuery::new(|s: &BlockDeviceCompletionStats| {
        (s.device_id, s.latency.as_micros() as u64)
    })).ok())
    .collect();

// Then, on each decision (no allocation):
let mut recent = [None; 8];
observers[0].weak_observe_recent_into(&mut recent)?;   // oldest first; None where unavailable
```

Rules:

- **Attach once, read often.** `attach_weak_observer` allocates; do it in a constructor or other
  setup code, never on a hot path. Reading with `weak_observe_recent_into(&mut buf)` does not
  allocate; `weak_observe_recent(n)` allocates a `Vec`.
- **Match the locking to where your code runs.** If the policy runs with interrupts disabled or a
  spinlock held (e.g. inside the scheduler), keep the observer in a `SpinLock<_, LocalIrqDisabled>`
  and never use a sleeping `Mutex` there. Code that may sleep can use `ostd::sync::Mutex`.
- Reads return `Err(OQueueError::Revoked)` if the observer was revoked or the OQueue removed; fall
  back to a decision that does not need the data.

### OQueues in the kernel

| Path | Element | What it carries |
|---|---|---|
| `scheduler.events` | `ostd::task::scheduler::SchedulingEvent` | Every context switch: `task` (an `Arc<Task>`; project it, e.g. to `crate::event::TaskId::new(&e.task)`) and `kind` (`Schedule`/`Deschedule`). Very high volume. |
| `io.block_io.bio_completion[<n>]` | `aster_block::bio::BlockDeviceCompletionStats` | One per block device: each completed I/O's `latency`, `outstanding_pages`, `queue_len`, `request_size_pages`, `device_id`. Created when the device is initialized. |
| `syscall.socket_oqueue` | `crate::syscall::oqueue::SocketOQueueMessage` | Socket accepts and closes: `fd`, `is_close`, `timestamp`. |
| `pmu.dtlb_miss_count` | `DtlbMisses` (x86 PMU) | Data-TLB miss counts; only with the `pmu.dtlb_enabled` kernel argument. |

The subsystem context lists any OQueues specific to this subsystem.

## Data capture: inspecting runtime traces in your turn

The kernel exports OQueues to userspace through the OQueue filesystem (`/oqueues` in the guest);
the harness can record any exported OQueue during a benchmark run and decodes it into one JSON
object per line, with the same values the OQueue carried. Recording runs a reader in the guest,
which perturbs the run slightly.

- `list_oqueues` lists every OQueue the kernel exports (path, Rust element type, whether it can be
  recorded) and describes the ones this subsystem documents.
- `collect_data` runs one benchmark (your policy or the baseline) while recording the OQueues you
  name by path, and returns each file's path and record count.
- `run_python` reads the recordings, one JSON object per line at the returned paths, for any
  analysis or training. Every scored turn also records this subsystem's default OQueues, in
  `turn_NNNNNN/score/run<i>/<oqueue>.jsonl`, so past turns' data is readable without spending a
  benchmark. Busy OQueues give large files: stream them rather than loading them whole. Durations
  and timestamps appear as `{secs, nanos}`. Ids (of tasks, CPUs, devices) are labels to group by,
  not quantities to average.

Use recordings to see what your policy would observe at runtime and how the system behaved under a
given policy, then encode what you learn in the policy.
