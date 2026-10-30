# Policy Explorer: quick start

Policy Explorer lets an LLM search for a better kernel policy in Mariposa. Each turn the model
writes the policy's Rust code; the harness builds the kernel with it, boots it in QEMU, runs your
benchmark, and compares the result with a baseline. The model sees every earlier result, can run
its own experiments and read kernel data within a turn, and tries again. The search stops when a
policy meets your success criterion or the budget runs out.

You can use this loop with an AI Agent to find better policy on any subsystem as long as:
(1) you have a benchmark inside Mariposa, 
(2) you can boot the kernel with the policy or the baseline, (3) you split the policy's code cleanly

## How the work is split

To speed through the process of deploying the policy exploration for your subsystem, we designed the development process in a way you can collaborate with a Coding Agent: 

| You (the human) | A coding agent (Claude Code, Codex, ...) |
|---|---|
| Write a `SPEC.md`: what the policy decides, your benchmark, how to boot the kernel, what to measure, what data matters. | Reads `policy_explorer/AGENTS.md` and your SPEC, and implements the subsystem adapter, wiring, and configs. |
| Point at your benchmark's source folder and the kernel code to evolve. | Moves the benchmark into the initramfs build, puts the policy in a file of its own, exposes the data. |
| Review the diff and the short test search. | Runs a build/boot check and a two-turn search. |

`AGENTS.md` describes the code and marks each decision it needs from you as **EXPECT USER TO
PROVIDE**; `SPEC.md` (in this directory) is the template with one section per decision. 

In short, you specify what subsystem you want to optimize for what benchmark in `SPEC.md`, and an coding agent implement the harness according to how should them being implemented in `AGENTS.md`. 

## Setup

Work inside the project's Docker container (see the repository's `AGENTS.md`), then:

```bash
python3 -m venv .venv && .venv/bin/pip install -r policy_explorer/requirements.txt
echo "OPENAI_API_KEY=..." >> .env        # any OpenAI-compatible endpoint; OPENAI_BASE_URL for a proxy
```

## Running a search

A search is one config of a subsystem:

```bash
cd policy_explorer
../.venv/bin/python3 run_experiment.py subsystems/<name>/configs/<search>.yaml
```

Results go to `results/<search>_<label>_<model>/`: one `turn_NNNNNN/` per turn with
everything it used: the model's description, its code under `policy/`, `perf.json`, the full
`transcript.json`, every benchmark's raw files, and its `run_python` `workspace/`. Re-running the same command resumes.

## Porting to your subsystem

Copy `SPEC.md`, fill it in, and ask a coding agent, for example:

> Read policy_explorer/AGENTS.md. Port the policy exploration loop to my subsystem as specified in
> path/to/SPEC.md. My benchmark is in path/to/my_bench/.

What you provide maps to four parts.

### 1. The benchmarking application

A program that runs inside Mariposa and measures what you care about. It prints a start marker, an
end marker (or an error marker), and writes its result files to the shared folder `/host/run/`.
(SPEC §3, §8.)

### 2. Kernel launching

What has to happen on the host before a run, the kernel arguments that pick your policy or the
baseline, and the devices the VM needs. The harness does the booting, timeouts, and result
collection. 
(SPEC §4, §5.)

### 3. Isolating the policy to evolve

Put the policy in a kernel source file of its own, behind an interface you define: the trait or
functions the kernel calls, and the hooks that call them. The model rewrites that whole file; the version you commit is its baseline and the fallback.

The harness swaps the model's file in only while it builds and benchmarks, then puts yours back,
including after a crash. Describe the interface in your SPEC. (SPEC §2.)

### 4. OQueues and data collection

OQueues are how Mariposa components expose events. Say which ones matter, in two roles: 
1. what the policy observes while it runs (part of its interface) (the OQueue containing the data your policy depends on when making a decision), and 
2. what the model may record and analyze (what data you think is relevant and your agent should look into while searching for good policy). 
Every search's prompt already explains OQueues, the registry and how a policy attaches
observers at runtime (`context/mariposa.md`), so your SPEC only adds what is the OQueue that you think is relevant to your subsystem. The model lists the OQueues the kernel exports with `list_oqueues`, records any of them
with the `collect_data` tool, and analyzes the records with `run_python`. OQueues are recorded through the OQueue filesystem (`/oqueues`); if a stream you need is not exported yet, the agent exports it. (SPEC §7.)

Your SPEC also says how runs are scored and when the search has succeeded (§6), and gives the task
statement (§1).

## What the model can do in a turn

`database_catalog` / `query_database` (past turns), `list_oqueues`, `collect_data`,
`run_python` (analysis, training), `build_policy`, `run_benchmark` (try a policy
or the baseline without ending the turn), `load_skill` (a shared guide the search offers, such as
`skills/deploying_a_model.md`), and `submit_policy` (ends the turn; the policy is scored). 

The loop settings in `configs/loop.yaml` control which are offered and how many builds/benchmarks a turn may
spend; a search config overrides them in its `loop:` block and lists its skills in `skills:`.

## Things to know

- `run_python` runs model-written code on the host (actually, in the `devcontainer` that Mariposa is built);
 the prompt scopes it to the experiment's directories.
- One experiment runs per checkout at a time (the policy code lives in the shared source tree).
