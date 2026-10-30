# SPEC: <subsystem name>

Fill in the **Your answer** part of every section, in prose, code snippets or file paths, then give
this file to a coding agent together with `policy_explorer/AGENTS.md`. Write "none" where a section
does not apply. Each section answers one "EXPECT USER TO PROVIDE" item of AGENTS.md.

## 1. Task statement

What the agent is asked to do, as you would brief a colleague. It becomes the opening of every
search prompt.

**Your answer:**

<!-- Write here. -->

**What to include:**

- The subsystem and the decision the policy makes ("You are exploring <decision> policies for
  <component> ...").
- The goal, in terms of the metrics in §6 ("make the P99 of <metric> as low as possible, beating
  the <baseline> baseline").

## 2. The policy to evolve

The kernel code the agent rewrites each turn, and what that code may rely on.

**Your answer:**

<!-- Write here. -->

**What to include:**

- **Where:** the kernel source file holding the policy, which the agent rewrites whole. If the
  policy is part of a larger file, say so; the coding agent moves it into a file of its own.
- **Interface:** the trait or function signature the code implements, and the constructor or
  inputs it receives.
- **What it may observe at runtime:** OQueue observers, statistics, or other state passed in, with
  their types and units.
- **Fallback:** the starting code (usually equivalent to the baseline). It must compile.
- **Constraints:** `no_std`, floating point, allocation, locking, a per-call time budget, anything
  that must stay true for the kernel to keep working.
- **How it is selected:** a kernel argument, a feature, or "always on".

## 3. Benchmark (runs in the guest)

The program that runs in the guest and measures the policy. It is the fixed part of every run: the
same code, built once into the initramfs. What it is fed (the trace, request mix or seed) is the
workload, in §8.

**Your answer:**

<!-- Write here. -->

**What to include:**

- **Source:** the folder holding the benchmark (it goes to `test/initramfs/src/benchmark/<bench>/`),
  or the existing one.
- **Build:** how it is built into the initramfs (an existing nix expression, or what it needs).
- **Entry point:** the script `init` runs and its arguments.
- **Marker and outputs:** the marker string it prints (`<MARKER>begin/end/error`), the files it
  writes to `/host/<out dir>/`, and their format.
- **How it takes a workload:** the argument that names it, or the file it reads from the share, and
  that file's format. Which workloads exist goes in §8.
- **Host peer (optional):** a `host.sh` or load generator that runs on the host during the run.

## 4. Host side and kernel launching

What the host does around each run, and how the kernel is booted for it.

**Your answer:**

<!-- Write here. -->

**What to include:**

- **Pre-benchmark jobs:** what must happen on the host before each run (zero or repartition disks,
  create images, drop caches, start a server) and what happens once per experiment (generate
  workloads).
- **Hardware:** devices or images the guest needs, and how they reach QEMU (`tools/qemu_args.sh`
  variables).
- **Kernel arguments:** for the policy, for the baseline, and always.
- **Build check:** the trivial task `build` boots to prove the policy compiles and runs.
- **VM size and time:** SMP, memory, and how long a run normally takes (for the timeout).

## 5. Baseline

What the agent's policies are compared against.

**Your answer:**

<!-- Write here. -->

**What to include:**

- The policy it is (e.g. the kernel's current default).
- How to run it: a kernel argument, the fallback code, ...

## 6. Metrics and success criterion

How runs are measured, compared, and judged.

**Your answer:**

<!-- Write here. -->

**What to include:**

- **Per-run metrics:** how one run's output files become named numbers.
- **Comparison:** how a candidate's runs are compared with the baseline's (statistical test,
  threshold, number of repeats).
- **Success:** when the search stops as successful.
- **Ranking:** the single number that ranks turns, and whether lower or higher is better.

## 7. OQueues and data

The runtime data the agent needs beyond what the kernel already exports. The agent can already
browse the kernel's exported OQueues, record any of them, and choose which ones its policy observes.

**Your answer:**

<!-- Write here. -->

**What to include:**

- **OQueues to add or export:** streams that do not exist yet or are not exported, with what one
  record means (fields, units) and any kernel argument needed for them to carry data.
- **Documented OQueues:** descriptions of existing streams worth explaining to the agent.
- **Recorded on every scored run:** which OQueues.
- **Observing from the policy:** where the policy may attach to OQueues (a hook outside any lock)
  and what it may do there.

## 8. Workloads

The inputs the benchmark (§3) runs on. A workload is the part of a run that can change while the
benchmark stays the same: a trace to replay, a request mix, a seed, a data size. Each one has a
name the agent passes to `run_benchmark`. The policy is tailored to `eval`, the one turns are
scored on.

**Your answer:**

<!-- Write here. -->

**What to include:**

- **`eval`:** its parameters, and how it is produced (a fixed file, a generator run on the host
  once per experiment, ...).
- **Others (optional):** workloads the agent may also run, e.g. `train`, the same generator with
  another seed, so the agent can study one and be scored on the other. The same details for each.
- If the benchmark takes no input, say so; `eval` is then the benchmark as is.

## 9. Extra tools (optional)

Subsystem-specific tools the agent should have.

**Your answer:**

<!-- Write here. -->

**What to include:**

- Each tool's name and purpose.
- What it takes and what it returns.
