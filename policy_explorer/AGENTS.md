# Policy Explorer: guide for coding agents

`policy_explorer/` is an agentic search loop for kernel policies in Mariposa. Each turn, an LLM
writes the code of a policy, the harness builds the kernel with it, boots it in QEMU, runs a
benchmark, compares the result with a baseline, and shows the outcome to the next turn.

The loop is fixed. What differs per kernel subsystem (the benchmark, how the kernel is booted,
which code is evolved, what is measured, which kernel data the agent can see) is a **subsystem**:
a directory `subsystems/<name>/` implementing the interface in `orchestrator/subsystem.py`.

This document says how the code is organized and how to implement a subsystem. It is deliberately
incomplete: the parts marked **EXPECT USER TO PROVIDE** are decisions only the user can make. The
user writes them in a `SPEC.md` (template: `SPEC.md` beside this file). **When a user hands you a
SPEC.md and asks you to port the loop to their subsystem, follow "Porting a subsystem" below,
taking every EXPECT USER TO PROVIDE item from their SPEC.md.** If the SPEC leaves one of them out,
ask the user; do not invent it.

Each SPEC section has the user's **Your answer** and a **What to include** list. The list is an
instruction to the user: what to provide so that you have enough information to implement that
part. Implement from the answers. If an answer leaves out something its list asks for, ask the user.

## Layout

| Path | What | Change it for a new subsystem? |
|---|---|---|
| `run_experiment.py` | CLI, `Config` and `LoopConfig`, the turn loop, scoring a submitted policy, `should_stop` | No |
| `configs/loop.yaml` | The loop settings every search starts from (model, budgets, tool switches) | No; override them in a search config's `loop:` |
| `orchestrator/agent_loop.py` | One turn's tool-calling conversation | No |
| `orchestrator/llm_client.py` | OpenAI-compatible chat client | No |
| `orchestrator/tools.py` | Every tool the agent gets (`Toolbox`) | No; add subsystem tools via `extra_tools` |
| `orchestrator/database.py` | The per-experiment results directory (resumable) | No |
| `orchestrator/prompting.py` | System prompt: context doc + turn history | No |
| `context/loop.md` | Shared context for every subsystem: how a search works, the generic tools | No |
| `context/mariposa.md` | Shared context for every subsystem: OQueues, the registry, observing at runtime, data capture | Only to document a new kernel OQueue |
| `orchestrator/skills.py`, `skills/` | Skills: self-contained guides a search lets the agent load (`load_skill`) | Add a skill only if it is not specific to one subsystem |
| `orchestrator/policy_file.py` | Writing the agent's code into the policy file and putting the fallback back | No |
| `orchestrator/evaluation.py` | Build + repeated benchmark of one policy, with retry | No |
| `orchestrator/mariposa.py` | Shared helpers: `build_kernel`, `run_guest` (boot, markers, results, OQueue capture) | No |
| `orchestrator/stats.py` | `means`, one-sided `welch` | No |
| `orchestrator/subsystem.py` | **The interface a subsystem implements** | No |
| `subsystems/<name>/` | One subsystem: `subsystem.py`, `SPEC.md`, `context/`, `prompts/`, `configs/` | **Yes: this is the port** |

Kernel- and guest-side pieces live where the rest of the repository keeps them:
`test/initramfs/src/benchmark/<bench>/` (guest benchmark), `test/initramfs/nix/benchmark/`
(building it into the initramfs), and the policy's own kernel source file(s).

## How a turn runs

1. `run_experiment.py` builds the system prompt: the shared context (`context/loop.md`, then
   `context/mariposa.md`), the subsystem's context document, the names and descriptions of the
   search's skills, a line per past turn, the current best, and "This is turn N." The search's
   prompt file is the user message.
2. The agent calls tools (see "Tools") until it calls `submit_policy` with `description`,
   `pseudocode` and `code`.
3. The harness writes `code` into the policy file, calls `Subsystem.build`, then
   `Subsystem.benchmark` `benchmark_repeats` times on the `eval` workload (the last run captures the
   subsystem's `score_capture` OQueues), puts the original code back, and calls `Subsystem.metrics`
   on every run.
4. The baseline (`Subsystem.benchmark(baseline=True)`, with the policy file at its committed
   code) is benchmarked once per experiment and reused.
5. `Subsystem.criteria(candidate_runs, baseline_runs)` returns a `Verdict`; it is stored in
   `turn_NNNNNN/perf.json`. A `satisfied` verdict is a screen: the harness re-benchmarks that policy
   with `benchmark_repeats` new runs and as many new baseline runs (`turn_N/confirm/`) and
   applies `criteria` again. The search stops at `max_turns`, at `max_total_tokens`, or at the first
   turn whose verdict is `satisfied` *and* reproduced; an unreproduced one is reported in the turn's
   summary and the search goes on.

A run that fails (`BenchmarkError`) is retried once. A build failure or a second run failure is
recorded as a failed turn with its log, and the search goes on.

## The subsystem interface (`orchestrator/subsystem.py`)

`subsystems/<name>/subsystem.py` defines a `Subsystem` subclass and a module-level
`create(params) -> Subsystem`. `params` is the config's free-form `params:` block. Nothing outside
the subsystem reads it.

| Member | Contract |
|---|---|
| `baseline_name` | What the baseline is called, shown to the agent. |
| `policy_file` | Repo-relative path of the kernel source file the agent writes, whole. |
| `context_file` | The context document, relative to the subsystem directory (default `context/policy.md`). |
| `workloads` | `{name: one-line description}` the agent may benchmark on. Must contain `eval` (what turns are scored on). Can be set in `__init__` or `prepare`. |
| `oqueues` | Descriptions of OQueues worth documenting (see "OQueues and data"); `collect_data` can record any exported OQueue. |
| `score_capture` | Names of OQueues captured on the last run of every scored evaluation. |
| `higher_is_better` | Direction of `Verdict.score` (default: lower is better). |
| `label()` | A string telling runs with different params apart; part of the results directory name. |
| `prepare(work_dir)` | Once per start, before anything is built: generate workloads and other inputs into `work_dir`. Must be idempotent (it runs again on resume). Call `super().prepare(work_dir)` first. |
| `build(log_dir) -> BuildResult` | Build the kernel with whatever policy is in the tree, then boot it on a trivial task that exercises the policy. Return `ok` and a log the agent can act on (compiler errors, panic). |
| `benchmark(out_dir, *, baseline, workload, capture)` | One benchmark run: do the host-side preparation, boot the kernel with the policy (or the baseline when `baseline`), run the benchmark, and leave its files and one `<oqueue>.jsonl` per captured OQueue in `out_dir`. Raise `BenchmarkError` if it did not complete. |
| `metrics(run_dir) -> dict[str, float]` | Reduce one run's files to named numbers. |
| `criteria(candidate_runs, baseline_runs) -> Verdict` | Compare per-run metrics. `Verdict.satisfied` stops the search, `score` ranks turns, `summary` is the one line the agent sees for the turn, `details` is stored. Also called with the current best turn's runs in place of the baseline's (`perf.json`'s `vs_best`). |
| `extra_tools()` | Optional `(schema, handler)` pairs for subsystem-specific tools. |

### Benchmark (guest side)

A benchmark is a program that runs in Mariposa's userspace. It is built into the initramfs and
started as `init` by the kernel command line.

- Source: `test/initramfs/src/benchmark/<bench>/`, with an entry script (conventionally `run.sh`).
  Optional `host.sh` for a host-side peer, as in
  `test/initramfs/src/benchmark/README.md`.
- Built into the initramfs by a nix expression under `test/initramfs/nix/benchmark/`, wired into
  `test/initramfs/nix/default.nix` like the existing ones. Benchmarks are only included when
  `ENABLE_BENCHMARK_TEST=true`, which `mariposa.MAKE_VARS` always sets.
- **Guest protocol** (what `mariposa.run_guest` relies on):
  - Print `<MARKER>begin ...` when it starts, `<MARKER>end ...` when it finished successfully, and
    `<MARKER>error <reason>` and exit on failure. `<MARKER>` is a unique string per benchmark
    (e.g. `MY_BENCH|`). Progress lines (`<MARKER>progress ...`) are allowed.
  - Read inputs from, and write result files to, the virtio-fs share mounted at `/host`: inputs at
    the paths `run_guest(share_files=...)` put them; results into `/host/<guest_out_dir>/` (default
    `run`; pass the directory to the script as an argument). Write results only after the timed
    part, so their I/O does not disturb the measurement.
  - Return from the script when done; the guest then powers off.

**EXPECT USER TO PROVIDE (SPEC §3):** the benchmark's source folder, its nix wiring (or what it
needs built), the entry script and its arguments, its marker, and its output files with their
format. If the user hands you a folder, move it to `test/initramfs/src/benchmark/<bench>/`, add the
nix expression, make the entry script follow the guest protocol, and check the initramfs builds.

### Host side: prepare, build, benchmark

`orchestrator/mariposa.py` does the generic part; call it from the subsystem:

- `build_kernel()` runs `make kernel` with `MAKE_VARS` and returns `(ok, log)`, the log starting at
  the first compiler error.
- `run_guest(out_dir, marker=, init_args=, kernel_args=, share_files=, env=, capture=, smp=, mem=,
  guest_out_dir=, timeout_s=)` boots once with `make run_kernel`, stages `share_files`, starts
  virtiofsd, kills a guest that does not power off or exceeds `timeout_s`, moves the guest's result
  files and `qemu.log` into `out_dir`, decodes captured OQueues into `<name>.jsonl`, and raises
  `BenchmarkError` unless the guest printed `<marker>end`. `env` reaches `tools/qemu_args.sh`
  (e.g. a variable that attaches extra block devices).

The subsystem supplies everything specific: host-side preparation before each run (formatting or
zeroing disks, creating arrays, starting a host-side load generator), the kernel arguments that
select the policy vs. the baseline, the init command, and the trivial task `build` boots.

**EXPECT USER TO PROVIDE (SPEC §4):** pre-benchmark jobs and when they run (once in `prepare`, or
before every run in `benchmark`); the hardware or images involved; the kernel arguments that select
the policy and the baseline; the trivial task for `build`; SMP/memory; how long a run may take.

### Policy (what the agent evolves)

The agent writes one whole kernel source file, the **policy file** (`policy_file`), behind an
interface the engineer defines elsewhere in the kernel: the trait or functions the kernel calls, the
arguments it passes, and the hooks that call them. The agent finds the best policy behind that
interface and never changes it; a policy file that does fails to build.

- The file's committed content is the **fallback**: the code the agent starts from, what the file
  holds outside a build, and what the baseline runs with. Keep it compiling and behaving like the
  baseline (or a safe default). It is the single source of truth: write `{{policy_file}}` in the
  context document to show it to the agent.
- The harness writes the agent's code into the file for a build and its benchmark runs only, then
  puts the fallback back, even on an exception or SIGTERM. A backup
  (`results/.policy_file_backup.json`) lets the next start restore a file a killed process left
  holding the agent's code. Only one experiment can run per checkout (`results/.lock`).
- Keep the file to the policy itself: the module the kernel's hooks call into. Anything that
  decides whether the module is compiled at all (a `#[cfg(...)]`) goes on its `mod` line, outside
  the file. A policy object selected at boot by a kernel argument (e.g. `<subsystem>.policy=agent`)
  also gives the baseline for free: boot with the other selection.

**EXPECT USER TO PROVIDE (SPEC §2):** where the policy is, what interface it implements and what it
may call or observe, the fallback, and the constraints on the code (`no_std`, no floating point,
latency budget, ...). Move the policy into a file of its own if it is not in one, and describe the
interface in the context document.

### Baseline, metrics and criteria

**EXPECT USER TO PROVIDE (SPEC §5-6):** the baseline; how one run's output becomes numbers; how a
candidate's runs are compared with the baseline's; when the search counts as successful; which
number ranks turns and in which direction. `orchestrator/stats.py` has `means` and a one-sided
Welch t-test (`welch`) to build `criteria` on. A per-turn test at p < 0.05 checked every turn will
pass by chance now and then; the harness's confirmation re-run (step 5 of "How a turn runs") guards
against stopping on such a lucky draw, so `criteria` needs no multiple-comparison correction.

### OQueues and data

Mariposa components expose events on OQueues (`ostd/src/orpc/oqueue/mod.rs`). Every OQueue created
with a path is in the kernel's registry (`ostd/src/orpc/oqueue/registry.rs`); the ones registered
with `registry::register`/`register_with` are also *exported* to userspace through the OQueue
filesystem, `/oqueues/<a>/<b>/{metadata.yaml, strong_observe}` in the guest. The agent decides what
to observe and record; the harness gives it the means:

- **Browse.** Every boot runs the guest command inside `/benchmark/common/oqueue_capture.sh`, which
  lists the exported OQueues; `list_oqueues` returns that listing (path, Rust element type, whether
  it can be recorded) plus the subsystem's `oqueues` descriptions.
- **Record.** `collect_data(oqueues=[paths])` records any exported, observable OQueue: the wrapper
  reads its `strong_observe` stream during the run and the launcher decodes it into
  `<path>.jsonl` (one JSON object per line; the last line can be cut short). Recording runs a reader
  in the guest, a small perturbation of the run.
- **Observe from the policy.** The policy can attach a weak observer to any registered OQueue by
  path (`registry::lookup_by_path`) and read it while deciding. Where and when it may attach is the
  subsystem's to provide: an attach hook that runs once outside any lock (e.g. an `attach()` in the
  policy file that the kernel calls once after boot), documented in the context document with a
  compiling example.

An `OQueue` entry in the subsystem documents a stream: `name` is its path, `description` and
`fields` explain a record, `kernel_args` are arguments the stream needs to carry data (e.g. a flag
that turns recording on). A stream that is not exported but recorded by the kernel's data-capture device sets
`capture_path` (the `path!(...)` of its capture file in `kernel/core/src/init.rs`, e.g.
`io.block.completion` behind `io.capture_block_io`); the launcher then reads it from the capture
image instead.

To make a new stream available, create its OQueue with a path and export it
(`registry::register(&path, &oqueue.as_any_oqueue())`, element type `Copy + Serialize`).

**EXPECT USER TO PROVIDE (SPEC §7):** OQueues to add or export for this subsystem, and descriptions
of the ones worth documenting; which are recorded on every scored run; how the policy may attach to
OQueues (the hook).

### Workloads

`workloads["eval"]` is what turns are scored on. The goal is instance specialization: a policy tailored
to the evaluation workload, so `eval` alone is the norm. Offer other workloads only if the SPEC asks for
them (e.g. a `train` workload: the same generator with another seed).

**EXPECT USER TO PROVIDE (SPEC §3, §8):** the evaluation workload and any others.

### Context document and search prompts

- `context/policy.md`: everything the agent must know to write a policy: the interface, the
  constructor shape, what it may observe, framework requirements, the starting point
  (`{{policy_file}}`), what the benchmark output files contain, and how to use the tools
  for this subsystem. It follows the shared `policy_explorer/context/loop.md` (how a turn works,
  the generic tools, the layout of a turn's data) and `policy_explorer/context/mariposa.md`
  (OQueues, the registry, attaching weak observers at runtime, data capture), so do not repeat
  those; do state
  what is specific here: which OQueues exist when the policy is constructed, whether the policy may
  attach its own observers (and under which locking constraints), and any subsystem-only OQueues.
- `prompts/<search>.md`: the search condition: the task and goal, and any hints for this condition.
  Guidance that is not specific to this subsystem (e.g. how to embed a trained model in a kernel
  policy) belongs in a skill under `policy_explorer/skills/`, which the condition's config offers
  with `skills:`; the prompt adds only how it applies here.
- `configs/<search>.yaml`: one per search condition (fields below).

**EXPECT USER TO PROVIDE (SPEC §1):** the task statement ("You are searching for a better ...
policy for ...") and the goal.

## Tools

| Tool | When | What |
|---|---|---|
| `database_catalog` | always | One entry per past turn: id, first line of its description, built or not, result summary, score; plus the current turn id. |
| `query_database` | always | One past turn's full record by id. |
| `list_oqueues` | always | Every OQueue the kernel exports (path, element type, observable) and the subsystem's `oqueues` descriptions. The first call may boot once to list them. |
| `submit_policy` | always | Ends the turn. |
| `run_python` | always | Python in the turn's `turn_N/workspace/`; can read the whole results directory. Not sandboxed. |
| `load_skill` | `skills` | The full text of one of the search's skills. |
| `build_policy` | `enable_benchmark_tool` | `Subsystem.build` with the given code. |
| `run_benchmark` | `enable_benchmark_tool` | Build + `repeats` runs of the code or the baseline on a workload; returns metrics and the baseline mean. |
| `collect_data` | `enable_benchmark_tool` | Build + one run recording the given OQueues (by path) into `turn_N/collect_NNNN/`. |

`build_policy`, `run_benchmark` and `collect_data` share `max_benchmarks_per_turn`.

## Config fields

A search config (`subsystems/<name>/configs/*.yaml`) says which search it is, what its subsystem
needs, and where it departs from the shared loop settings:

| Field | Meaning |
|---|---|
| `search_name` | First part of the results directory name. |
| `subsystem` | Directory name under `subsystems/`. |
| `prompt_file` | The search prompt, relative to the subsystem directory. |
| `params` | The subsystem's own settings (its benchmark, disks, VM size, ...), free-form; passed to `create(params)`. The loop never reads them, so give defaults in `subsystem.py`. |
| `skills` | Names of skills under `skills/` the agent may load (default none). |
| `loop` | Overrides of the loop settings below, for this search only. |

The loop settings (`LoopConfig`) all come from `configs/loop.yaml` unless the search's `loop:`
overrides them:

| Field | Meaning |
|---|---|
| `backend`, `model` | `openai` (any OpenAI-compatible endpoint; `OPENAI_API_KEY`, optional `OPENAI_BASE_URL`) and the model id. |
| `reasoning_effort` | Passed to every API call as `reasoning_effort`. |
| `benchmark_repeats` | Runs per scored evaluation and for the baseline. |
| `max_turns`, `max_total_tokens` | Stop conditions. |
| `max_tokens_per_turn` | A turn that spends this much without submitting is recorded as failed. |
| `enable_benchmark_tool`, `max_benchmarks_per_turn` | The kernel-booting tools' switch and budget. |

## Results directory (`results/<search_name>_<label>_<model>/`)

- `config.yaml`: the resolved config (the search's file plus the loop defaults it did not override).
- `index.jsonl`: which turns exist. A turn appends `{"turn_id": N, "finished": false}` when it starts
  and its summary with `"finished": true` once recorded; only finished turns are read back, and a
  finished turn whose directory is missing is an error.
- `turn_NNNNNN/`: everything the turn used: `build_NNNN/`, `bench_NNNN/`, `collect_NNNN/`, `score/`
  and `confirm/` (each with one `run<i>/` per run; a failed attempt is kept as `run<i>_failed<k>/`),
  `workspace/` (its `run_python` working directory), `usage.json` (total and reasoning tokens so
  far, rewritten after every LLM call so a crashed turn's tokens still count against
  `max_total_tokens`), and its record: `description.md`, the submitted policy file (under its
  kernel file name), `perf.json`, `transcript.json`.
- `baseline/` and `baseline.json`: the baseline's runs and per-run metrics.

The subsystem may add its own.

## Porting a subsystem

Given a filled-in SPEC.md:

1. Read the SPEC.
2. Guest benchmark (SPEC §3): place the source under `test/initramfs/src/benchmark/<bench>/`, add
   the nix wiring, make the entry script follow the guest protocol. Check with
   `make kernel ENABLE_BENCHMARK_TEST=true RELEASE=1` and one manual `make run_kernel ...
   INITARGS=...` boot.
3. Policy (SPEC §2): put the policy code in a kernel source file of its own (`policy_file`), with a
   fallback that compiles; add a kernel argument that selects the baseline if the
   SPEC's baseline is a different policy.
4. OQueues (SPEC §7): add data-capture files and kernel flags for streams that have none.
5. `subsystems/<name>/subsystem.py`: implement the interface with `orchestrator/mariposa.py`;
   `metrics` and `criteria` per SPEC §6.
6. `context/policy.md` (with `{{policy_file}}`), one `prompts/<search>.md` (SPEC §1) and
   its `configs/<search>.yaml` (`params` for SPEC §3, §5; the budgets of `configs/loop.yaml`), and
   copy the SPEC to `subsystems/<name>/SPEC.md`. Add more search conditions only if the user asks.
7. Verify, in this order, and report what passed:
   - one `build` and one `benchmark` of the fallback and of the baseline from a Python shell
     (`create(params)`, `prepare(dir)`, `build(dir)`, `benchmark(dir/'run1', baseline=..., workload='eval', capture=[...])`),
     checking the output files and `metrics`;
   - a short real search (a config with `max_turns: 2`).

Do not change the fixed core for a port. If the SPEC needs something the interface cannot express,
stop and tell the user what and why.

## Building and running here

- `cargo-osdk` is installed per checkout; if the global one belongs to another worktree the build
  fails with a lockfile collision. Install a private copy (`OSDK_LOCAL_DEV=1 cargo install
  cargo-osdk --path osdk --root <dir> --locked`) and run with `PATH=<dir>/bin:$PATH` and
  `MAKEFLAGS=" -- CARGO_OSDK=<dir>/bin/cargo-osdk"`.
- Never point a search at hardware someone else is using: a subsystem's `benchmark` may
  repartition or wipe the disks its config names.
- Never commit agent-written code into the policy file: its committed content is the fallback.
