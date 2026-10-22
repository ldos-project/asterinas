# SPDX-License-Identifier: MPL-2.0

"""Build and benchmark one policy, or the baseline, through the subsystem."""

# Shared by the end-of-turn scoring and the build_policy/run_benchmark/collect_data tools. The
# agent's code is in the policy file only inside `PolicyFile.containing`: for its build and its runs.

import shutil
from dataclasses import dataclass, field
from pathlib import Path

from orchestrator.policy_file import PolicyFile
from orchestrator.subsystem import BenchmarkError, BuildResult, OQueue, Subsystem


@dataclass
class Evaluation:
    """Outcome of building and benchmarking one policy."""

    build_ok: bool
    build_log: str = ""
    runs: list[dict] = field(default_factory=list)
    run_dirs: list[Path] = field(default_factory=list)
    # Set when the policy built but a run failed twice.
    error: str | None = None


class Runner:
    """Builds and benchmarks policies for one experiment."""

    def __init__(self, subsystem: Subsystem, policy: PolicyFile, log=print):
        """Bind the runner to a subsystem and its policy file."""
        self.subsystem = subsystem
        self.policy = policy
        self.log = log

    def build(self, code: str, log_dir: Path) -> BuildResult:
        """Build and smoke-boot a policy without benchmarking it."""
        log_dir.mkdir(parents=True, exist_ok=True)
        with self.policy.containing(code):
            result = self.subsystem.build(log_dir)
        (log_dir / "build.log").write_text(result.log)
        return result

    def _run_with_retry(self, out_dir: Path, **kwargs) -> None:
        """Run the benchmark once, retrying a failed run once."""
        # A guest occasionally hangs or dies without reporting completion. Each failed attempt's files
        # are kept as `<run>_failed<attempt>` for diagnosis.
        for attempt in (1, 2):
            try:
                self.subsystem.benchmark(out_dir, **kwargs)
                return
            except BenchmarkError as error:
                if out_dir.exists():
                    shutil.copytree(out_dir, out_dir.with_name(f"{out_dir.name}_failed{attempt}"), dirs_exist_ok=True)
                self.log(f"{out_dir} attempt {attempt} failed: {str(error)[-300:]}")
                if attempt == 2:
                    raise

    def _runs(self, out_root: Path, *, baseline: bool, workload: str, repeats: int, capture: list[OQueue], evaluation: Evaluation) -> None:
        """Run the benchmark `repeats` times and record each run's metrics."""
        for i in range(repeats):
            out_dir = out_root / f"run{i + 1}"
            last = i == repeats - 1
            try:
                # TODO(yingqi): making capturing a separate concern as running benchmark. 
                # Only the last run captures, so the others are not slowed by capturing.
                self._run_with_retry(out_dir, baseline=baseline, workload=workload, capture=list(capture) if last else [])
            except BenchmarkError as error:
                evaluation.error = str(error)[-2000:]
                return
            evaluation.runs.append(self.subsystem.metrics(out_dir))
            evaluation.run_dirs.append(out_dir)

    def evaluate(
        self,
        out_root: Path,
        code: str | None,
        *,
        workload: str = "eval",
        repeats: int = 1,
        capture: list[OQueue] = (),
    ) -> Evaluation:
        """Build and benchmark a policy (`code`), or the baseline (`code=None`).
        capture: a list of OQueues that will be captured during the evaluation
        """
        out_root.mkdir(parents=True, exist_ok=True)
        # The baseline runs with the policy file at its fallback; no build check needed.
        if code is None:
            evaluation = Evaluation(build_ok=True)
            self._runs(out_root, baseline=True, workload=workload, repeats=repeats, capture=capture, evaluation=evaluation)
            return evaluation
        # The agent's code stays in the policy file for the build and every run.
        with self.policy.containing(code):
            built = self.subsystem.build(out_root)
            (out_root / "build.log").write_text(built.log)
            evaluation = Evaluation(build_ok=built.ok, build_log=built.log)
            if built.ok:
                self._runs(out_root, baseline=False, workload=workload, repeats=repeats, capture=capture, evaluation=evaluation)
        return evaluation
