#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Run a policy search: `python run_experiment.py subsystems/<name>/configs/<search>.yaml`."""

# Resumes from `results/<search_name>_<label>_<model>/` if it already has turns. Calls the real LLM
# API and builds and boots real kernels; only run it when you mean to.

import argparse
import fcntl
import importlib
import os
import re
import signal
import sys
from dataclasses import asdict, dataclass, field, fields
from pathlib import Path

import openai
import yaml
from dotenv import load_dotenv

from orchestrator.agent_loop import run_turn
from orchestrator.database import Database, TurnRecord
from orchestrator.evaluation import Runner
from orchestrator.llm_client import LLMClient
from orchestrator.policy_file import PolicyFile
from orchestrator.prompting import build_system_prompt, render_context
from orchestrator.skills import Skill, catalog, load_skill
from orchestrator.stats import means
from orchestrator.subsystem import BenchmarkError, OQueue, Subsystem
from orchestrator.tools import Toolbox

REPO_ROOT = Path(__file__).resolve().parents[1]
POLICY_EXPLORER_ROOT = Path(__file__).resolve().parent
RESULTS_ROOT = POLICY_EXPLORER_ROOT / "results"
# The loop settings every search starts from; a search config's `loop:` block overrides them.
LOOP_DEFAULTS = POLICY_EXPLORER_ROOT / "configs" / "loop.yaml"
# Shared by every subsystem, in this order: how a search and its tools work; OQueues, the registry,
# observing at runtime, data capture.
SHARED_CONTEXT = (POLICY_EXPLORER_ROOT / "context" / "loop.md", POLICY_EXPLORER_ROOT / "context" / "mariposa.md")
# The skills a search config can offer the agent (`skills:`).
SKILLS_DIR = POLICY_EXPLORER_ROOT / "skills"
# One experiment per checkout: they would fight over the policy file and the guest share.
LOCK_FILE = RESULTS_ROOT / ".lock"
# The fallback of a policy file that holds the agent's code right now (policy_file.PolicyFile).
POLICY_BACKUP = RESULTS_ROOT / ".policy_file_backup.json"
# backend name -> (API key env var, base URL). A None base URL means the SDK's own default, which
# for OpenAI honors OPENAI_BASE_URL (so a token proxy needs no code change).
BACKENDS = {
    "openai": ("OPENAI_API_KEY", None),
}


@dataclass
class LoopConfig:
    """How the loop runs a search: the model, the budgets and the tools (`configs/loop.yaml`)."""

    backend: str
    model: str
    reasoning_effort: str
    benchmark_repeats: int
    max_turns: int
    max_total_tokens: int
    max_tokens_per_turn: int
    enable_benchmark_tool: bool
    max_benchmarks_per_turn: int


@dataclass
class Config:
    """A search config (`subsystems/<name>/configs/*.yaml`), with its loop settings resolved."""

    search_name: str
    subsystem: str
    prompt_file: str
    loop: LoopConfig
    # The subsystem's own settings, passed to its `create`; the loop never reads them.
    params: dict = field(default_factory=dict)
    # The skills (`skills/<name>.md`) the agent may load.
    skills: list[str] = field(default_factory=list)


def subsystem_dir(name: str) -> Path:
    """Directory of a subsystem."""
    return POLICY_EXPLORER_ROOT / "subsystems" / name


def load_config(path: Path) -> Config:
    """Load and check a search config, filling its loop settings from `configs/loop.yaml`."""
    data = yaml.safe_load(Path(path).read_text())
    misplaced = sorted(set(data) & {f.name for f in fields(LoopConfig)})
    if misplaced:
        raise ValueError(f"{misplaced} are loop settings; put them under `loop:`")
    loop = LoopConfig(**{**yaml.safe_load(LOOP_DEFAULTS.read_text()), **(data.pop("loop", None) or {})})
    config = Config(**data, loop=loop)
    if loop.backend not in BACKENDS:
        raise ValueError(f"unknown backend {loop.backend!r}; expected one of {sorted(BACKENDS)}")
    if not (subsystem_dir(config.subsystem) / "subsystem.py").exists():
        raise ValueError(f"no subsystem {config.subsystem!r} (expected {subsystem_dir(config.subsystem)}/subsystem.py)")
    return config


def make_client(config: Config) -> LLMClient:
    """The LLM client of a search's backend and model; exits if the backend's API key is not set."""
    load_dotenv(REPO_ROOT / ".env")
    load_dotenv(POLICY_EXPLORER_ROOT / ".env")
    key_var, base_url = BACKENDS[config.loop.backend]
    api_key = os.environ.get(key_var)
    if not api_key:
        sys.exit(f"{key_var} is not set (checked the environment and .env files)")
    return LLMClient(
        openai.OpenAI(base_url=base_url, api_key=api_key), model=config.loop.model, reasoning_effort=config.loop.reasoning_effort
    )


def load_skills(config: Config) -> list[Skill]:
    """The skills the config lets the agent load."""
    return [load_skill(SKILLS_DIR, name) for name in config.skills]


def load_context(config: Config, subsystem: Subsystem, fallback: str, skills: list[Skill]) -> str:
    """The context every turn's system prompt starts with: the shared context, the subsystem's, the skills."""
    parts = [path.read_text() for path in SHARED_CONTEXT]
    parts.append((subsystem_dir(config.subsystem) / subsystem.context_file).read_text())
    if skills:
        parts.append(catalog(skills))
    return render_context("\n\n".join(parts), fallback)


def load_subsystem(name: str, params: dict) -> Subsystem:
    """Create a subsystem from its params."""
    module = importlib.import_module(f"subsystems.{name}.subsystem")
    return module.create(params)


def results_dir(config: Config, label: str) -> Path:
    """Results directory of an experiment."""
    # `<search_name>_<label>_<model>`, so one search can run with different params and models side
    # by side. Characters a model id may contain but a directory name shouldn't (`/`, `:`) become `-`.
    model = re.sub(r"[^A-Za-z0-9._-]+", "-", config.loop.model)
    return RESULTS_ROOT / "_".join(part for part in (config.search_name, label, model) if part)


def should_stop(database: Database, config: Config) -> tuple[bool, str]:
    """Decide whether the search is over, and why."""
    turns = database.all_turns()

    if len(turns) >= config.loop.max_turns:
        return True, "max_turns"

    if database.total_tokens_used() >= config.loop.max_total_tokens:
        return True, "max_total_tokens"

    # Success needs the screening verdict and its independent confirmation (see `Experiment.confirm`).
    if any(turn.perf.get("build_ok") and turn.perf.get("satisfied") and (turn.perf.get("confirmation") or {}).get("confirmed")
           for turn in turns):
        return True, "success"

    return False, ""


def _usage_record(usages) -> dict:
    """A turn's `usage.json`: tokens spent, and how many of them were reasoning."""
    return {
        "total_tokens": sum(u.total_tokens for u in usages),
        "reasoning_tokens": sum(u.reasoning_tokens for u in usages),
    }


class Experiment:
    """One search: runs turns and scores submitted policies."""

    def __init__(self, config: Config, subsystem: Subsystem, database: Database, policy: PolicyFile, skills: list[Skill] = ()):
        """Wire the subsystem, policy file, database and tools together."""
        self.config = config
        self.subsystem = subsystem
        self.database = database
        self.policy = policy
        self.runner = Runner(subsystem, policy, log=self.log)
        self.toolbox = Toolbox(
            database=database,
            subsystem=subsystem,
            runner=self.runner,
            policy=policy,
            ensure_baseline=self.ensure_baseline,
            enable_benchmark_tool=config.loop.enable_benchmark_tool,
            max_benchmarks_per_turn=config.loop.max_benchmarks_per_turn,
            skills=skills,
            log=self.log,
        )

    def log(self, message: str) -> None:
        """Print a progress message."""
        print(f"[{self.config.search_name}] {message}", flush=True)

    def ensure_baseline(self) -> list[dict]:
        """The baseline's per-run metrics, benchmarking it on first use."""
        # Benchmarked once per experiment, with the policy file at its fallback.
        baseline = self.database.read_baseline()
        if baseline is not None:
            return baseline
        self.log(f"benchmarking the {self.subsystem.baseline_name} baseline ({self.config.loop.benchmark_repeats} runs)...")
        # Records the same OQueues on its last run as a scored candidate does, so both arms pay the same
        # recording cost.
        evaluation = self.runner.evaluate(
            self.database.baseline_dir(), None, repeats=self.config.loop.benchmark_repeats,
            capture=self.score_capture_oqueues(),
        )
        if evaluation.error:
            raise BenchmarkError(f"the baseline benchmark failed twice: {evaluation.error}")
        self.database.write_baseline(evaluation.runs)
        return evaluation.runs

    def score_capture_oqueues(self) -> list[OQueue]:
        """The OQueues recorded on the last run of every scored evaluation and of the baseline."""
        documented = {o.name: o for o in self.subsystem.oqueues}
        return [documented.get(name, OQueue(name)) for name in self.subsystem.score_capture]

    def confirm(self, turn_id: int, code: str) -> dict:
        """Re-benchmark a policy that passed screening, against fresh baseline runs."""
        # Independent of the runs it was selected on: `benchmark_repeats` new runs of the policy, then
        # as many new baseline runs, judged by the same criteria. Only a reproduced verdict stops the search.
        root = self.database.turn_dir(turn_id) / "confirm"
        repeats = self.config.loop.benchmark_repeats
        self.log(f"turn {turn_id} passed screening; confirming with {repeats} new runs of it and of the baseline")
        candidate = self.runner.evaluate(root / "candidate", code, repeats=repeats)
        baseline = self.runner.evaluate(root / "baseline", None, repeats=repeats) if candidate.build_ok and not candidate.error else None
        if baseline is None or baseline.error:
            error = candidate.error or candidate.build_log or (baseline.error if baseline else "")
            return {"confirmed": False, "summary": f"failed to run ({error[-300:]}); not confirmed."}
        verdict = self.subsystem.criteria(candidate.runs, baseline.runs)
        return {
            "confirmed": verdict.satisfied,
            "summary": ("reproduced. " if verdict.satisfied else "NOT reproduced. ") + verdict.summary,
            "details": verdict.details,
            "candidate_runs": candidate.runs,
            "baseline_runs": baseline.runs,
        }

    def score(self, turn_id: int, code: str) -> dict:
        """Score a submitted policy; return the turn's perf record."""
        # `benchmark_repeats` runs on 'eval', the last capturing `score_capture`; judged by `criteria`.
        capture = self.score_capture_oqueues()
        evaluation = self.runner.evaluate(
            self.database.turn_dir(turn_id) / "score", code, repeats=self.config.loop.benchmark_repeats, capture=capture
        )
        if not evaluation.build_ok:
            return {"build_ok": False, "error": evaluation.build_log[-4000:]}
        if evaluation.error:
            return {"build_ok": False, "error": f"the policy built, but its benchmark run failed twice: {evaluation.error}"}
        baseline_runs = self.ensure_baseline()
        best = self.database.current_best()
        verdict = self.subsystem.criteria(evaluation.runs, baseline_runs)
        confirmation = self.confirm(turn_id, code) if verdict.satisfied else None
        summary = verdict.summary + (f" Confirmation re-run: {confirmation['summary']}" if confirmation else "")
        return {
            "build_ok": True,
            "error": None,
            "satisfied": verdict.satisfied,
            "confirmation": confirmation,
            "score": verdict.score,
            "summary": summary,
            "details": verdict.details,
            "candidate_runs": evaluation.runs,
            "candidate_mean": means(evaluation.runs),
            "baseline_mean": means(baseline_runs),
            # The same criteria against the current best turn instead of the baseline.
            "vs_best": self.subsystem.criteria(evaluation.runs, best.perf["candidate_runs"]).details if best else None,
            "params": self.config.params,
        }

    def run_one_turn(self, client: LLMClient, context: str, user_prompt: str) -> None:
        """Run one turn: let the agent work, score its policy, record the turn."""
        turn_id = self.database.start_turn()
        self.toolbox.start_turn(turn_id)
        messages: list = []
        self.log(f"turn {turn_id} starting")
        try:
            submitted, usages = run_turn(
                client,
                self.toolbox,
                build_system_prompt(context, self.database, turn_id),
                max_tokens_per_turn=self.config.loop.max_tokens_per_turn,
                messages_out=messages,
                user_prompt=user_prompt,
                on_usage=lambda usages: self.database.write_usage(turn_id, _usage_record(usages)),
            )
        except RuntimeError as error:
            # The turn hit its token budget without submitting: record the spend (it counts against
            # max_total_tokens) as a failed turn, and let should_stop decide what's next.
            if not hasattr(error, "usages"):
                raise
            self.log(f"turn {turn_id}: {error}")
            self.database.write_turn(
                TurnRecord(turn_id, "(turn ended without submitting a policy)", "", {"build_ok": False, "error": str(error)},
                           _usage_record(error.usages)),
                transcript=messages,
            )
            return

        self.log(f"turn {turn_id}: scoring the submitted policy")
        # Scoring the policy generated in this turn. 
        perf = self.score(turn_id, submitted.code)
        self.log(f"turn {turn_id}: " + (perf["summary"] if perf["build_ok"] else "build or benchmark failed"))
        # Add the turn running info to the database
        self.database.write_turn(
            TurnRecord(turn_id, submitted.description, submitted.code, perf, _usage_record(usages)),
            transcript=messages,
        )


def _lock_checkout():
    """Make sure no other experiment runs in this checkout."""
    # Held until the process exits; the caller keeps the returned file open.
    RESULTS_ROOT.mkdir(parents=True, exist_ok=True)
    lock = open(LOCK_FILE, "w")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        sys.exit(f"another experiment is running in this checkout ({LOCK_FILE} is locked)")
    return lock


def main(argv=None) -> int:
    """Run a search from a config file until it stops."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("config", type=Path, help="Path to a search config YAML file")
    args = parser.parse_args(argv)

    config = load_config(args.config)
    skills = load_skills(config)
    client = make_client(config)

    _lock = _lock_checkout()
    # SIGTERM unwinds like Ctrl-C, so `PolicyFile.containing` puts the fallback back on the way out.
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))

    # Load the subsystem, its policy file (repairing one a killed run left holding the agent's code)
    # and the results directory, then let the subsystem prepare its inputs.
    subsystem = load_subsystem(config.subsystem, config.params)
    root = results_dir(config, subsystem.label())
    policy = PolicyFile(REPO_ROOT, subsystem.policy_file, POLICY_BACKUP)
    database = Database(root, Path(subsystem.policy_file).name, subsystem.higher_is_better)
    # The resolved config: the search's file with the loop defaults it did not override.
    (database.root / "config.yaml").write_text(yaml.safe_dump(asdict(config), sort_keys=False))
    subsystem.prepare(database.root)

    context = load_context(config, subsystem, policy.fallback, skills)
    user_prompt = (subsystem_dir(config.subsystem) / config.prompt_file).read_text()

    experiment = Experiment(config, subsystem, database, policy, skills)

    while True:
        stop, reason = should_stop(database, config)
        if stop:
            experiment.log(f"stopped ({reason}) after {len(database.all_turns())} turns.")
            return 0
        experiment.run_one_turn(client, context, user_prompt)


if __name__ == "__main__":
    sys.exit(main())
