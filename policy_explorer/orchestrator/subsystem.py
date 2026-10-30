# SPDX-License-Identifier: MPL-2.0

"""The interface a kernel subsystem implements to plug into the search loop."""

# A subsystem lives in `subsystems/<name>/subsystem.py`, subclasses `Subsystem`, and exposes
# `create(params)`. The loop only calls the methods below; AGENTS.md says how to implement each.

from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable


class BenchmarkError(RuntimeError):
    """A benchmark run that did not complete."""

    # The loop retries a failed run once before giving up.


@dataclass(frozen=True)
class OQueue:
    """A stream of kernel records the agent can capture with `collect_data`."""

    # `name` is the OQueue's registry path (e.g. `scheduler.events`). By default it is read from the
    # OQueue filesystem (`/oqueues`), which needs the OQueue to be exported. A stream that is only
    # recorded by the kernel's data-capture device sets `capture_path` instead: the `path!(...)` its
    # capture file is registered under. `kernel_args` are kernel arguments the stream needs to
    # carry data (e.g. a flag that turns recording on).
    name: str
    description: str = ""
    fields: str = ""
    kernel_args: tuple[str, ...] = ()
    capture_path: str | None = None


@dataclass
class BuildResult:
    """Outcome of building and smoke-booting a policy."""

    ok: bool
    log: str


@dataclass
class Verdict:
    """How a candidate compares with the baseline."""

    # Stops the search when true.
    satisfied: bool
    # Ranks turns for "current best"; lower is better unless `higher_is_better`.
    score: float
    # The one line the agent sees for this turn in its history and in `database_catalog`.
    summary: str
    details: dict = field(default_factory=dict)


class Subsystem:
    """Base class for a subsystem adapter."""

    # Shown to the agent, e.g. "default".
    baseline_name: str = "baseline"
    # Repo-relative path of the kernel source file the agent writes, whole, behind the policy interface.
    policy_file: str
    # Starts every turn's system prompt; relative to the subsystem directory.
    context_file: str = "context/policy.md"
    higher_is_better: bool = False
    # name -> one-line description; "eval" is what turns are scored on and must exist.
    workloads: dict[str, str] = {"eval": "the workload submitted policies are scored on"}
    # Descriptions of OQueues the subsystem knows about. The agent can also capture any other OQueue
    # the kernel exports (`list_oqueues` shows them).
    oqueues: tuple[OQueue, ...] = ()
    # OQueue names captured on the last run of every scored evaluation, so later turns can read them
    # without spending a benchmark.
    score_capture: tuple[str, ...] = ()

    def __init__(self, params: dict):
        """Create the subsystem from the config's `params:` block."""
        # The loop never reads `params` itself.
        self.params = params
        self.work_dir: Path | None = None

    def label(self) -> str:
        """Name the params in the results directory."""
        # Tells runs with different params apart, e.g. "<workload>_<size>".
        return ""

    def prepare(self, work_dir: Path) -> None:
        """Prepare inputs every run needs, once per experiment start."""
        # E.g. generate workloads into `work_dir` (the results directory). Runs again on resume, so
        # it must be idempotent. Overrides call this first to set `self.work_dir`.
        self.work_dir = Path(work_dir)

    def build(self, log_dir: Path) -> BuildResult:
        """Build the kernel with the current policy and check that it boots."""
        # Boot it on a trivial task that exercises the policy, so a compile error or a panic on first
        # use costs a short boot instead of a benchmark. The log is shown to the agent.
        raise NotImplementedError

    def benchmark(self, out_dir: Path, *, baseline: bool, workload: str, capture: list[OQueue]) -> None:
        """Run the benchmark once."""
        # Includes any per-run host preparation. Leaves the run's files, and `<name>.jsonl` per
        # OQueue in `capture`, in `out_dir`. `baseline` runs the baseline instead of the policy in the
        # tree. Raise BenchmarkError if the run did not complete.
        raise NotImplementedError

    def metrics(self, run_dir: Path) -> dict[str, float]:
        """Reduce one run's output to named numbers."""
        raise NotImplementedError

    def criteria(self, candidate_runs: list[dict], baseline_runs: list[dict]) -> Verdict:
        """Judge a candidate's runs against the baseline's."""
        # Also called with the current best turn's runs in place of the baseline's (`vs_best`).
        raise NotImplementedError

    def extra_tools(self) -> list[tuple[dict, Callable[[dict], dict]]]:
        """Offer subsystem-specific tools to the agent."""
        # (OpenAI function schema, handler) pairs. A handler takes the call's arguments and returns a
        # JSON-serializable dict, or raises `orchestrator.tools.ToolError` with a message for the agent.
        return []
