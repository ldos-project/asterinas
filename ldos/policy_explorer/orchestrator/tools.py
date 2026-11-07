# SPDX-License-Identifier: MPL-2.0

"""The tools the agent can call during a turn.

Always offered: read the database, list OQueues, `run_python` (the agent's way to analyze captured
data and everything else on disk), submit. The kernel-booting tools (`build_policy`,
`run_benchmark`, `collect_data`) come with `enable_benchmark_tool` and share a per-turn budget.
`load_skill` comes with a search's `skills`.
"""

import inspect
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Annotated, Callable

from pydantic import ConfigDict, Field, TypeAdapter, ValidationError, validate_call

from orchestrator import mariposa
from orchestrator.database import Database
from orchestrator.evaluation import Runner
from orchestrator.policy_file import PolicyFile
from orchestrator.skills import Skill
from orchestrator.stats import means
from orchestrator.subsystem import OQueue, Subsystem

BUILD_LOG_TAIL = 4000
# The packages the agent can import in `run_python` (installed in the harness's own environment).
REQUIREMENTS = Path(__file__).resolve().parents[1] / "requirements-agent.txt"

# The tools' argument types shared by several tools. `{field}`s in their descriptions are filled per
# experiment, like those in a tool's docstring.
Code = Annotated[str, Field(min_length=1, description="The complete new content of the policy file, {policy_path}.")]
Workload = Annotated[str, Field(description="{workloads} (default 'eval').")]


class ToolError(Exception):
    """A tool call that failed; its message goes back to the agent."""


@dataclass
class SubmittedPolicy:
    """A policy the agent submitted to end its turn."""

    description: str
    pseudocode: str
    code: str


# Every `@tool` method's name and `when`, in the order they are defined (the order the agent sees).
_TOOLS: dict[str, str | None] = {}


def tool(when: str | None = None):
    """Offer a `Toolbox` method to the agent as a tool named after the method.

    The method's docstring is the tool's description and its parameters are the tool's arguments,
    checked before each call. `{field}`s in the docstring and in parameter descriptions are filled
    per experiment (`Toolbox._fill`). With `when`, the tool is offered only while that `Toolbox`
    attribute is truthy.
    """
    def register(method):
        _TOOLS[method.__name__] = when
        return method

    return register


def _schema_and_handler(method: Callable, fill: dict) -> tuple[dict, Callable[[object], object]]:
    """The OpenAI function tool of a bound `@tool` method, and the handler of a call's raw arguments."""
    name = method.__name__
    # pydantic builds the schema of a function's parameters; its type stubs only accept types.
    parameters = TypeAdapter(method).json_schema()  # pyright: ignore[reportArgumentType]
    for argument in parameters["properties"].values():
        if "description" in argument:
            argument["description"] = argument["description"].format(**fill)
    # The docstring's line breaks are only source formatting.
    description = " ".join((inspect.getdoc(method) or "").split()).format(**fill)
    # Arguments the tool does not take are ignored.
    checked = validate_call(config=ConfigDict(extra="ignore"))(method)

    def call(raw: object):
        """Check a call's raw arguments and run the tool with them."""
        if not isinstance(raw, dict):
            raise ToolError(f"{name}'s arguments must be a JSON object")
        try:
            return checked(**raw)
        except ValidationError as error:
            raise ToolError("; ".join(f"{name}'s '{'.'.join(map(str, e['loc']))}': {e['msg']}" for e in error.errors())) from None

    schema = {"type": "function", "function": {"name": name, "description": description, "parameters": parameters}}
    return schema, call


def _truncate(text: str, limit: int = 4_000) -> str:
    """Cut long tool output to `limit` characters."""
    if len(text) <= limit:
        return text
    return text[:limit] + f"\n...[truncated, {len(text) - limit} more characters]"


def _run_python(code: str, workspace_dir: Path, run=subprocess.run, timeout: float = 1800) -> dict:
    """Run the agent's Python code in its workspace.

    Not sandboxed: the tool description scopes the agent to its workspace and the database.
    """
    workspace_dir.mkdir(parents=True, exist_ok=True)
    try:
        process = run(
            [sys.executable, "-c", code], cwd=workspace_dir, capture_output=True, text=True, timeout=timeout
        )
    except subprocess.TimeoutExpired:
        return {"stdout": "", "stderr": f"timed out after {timeout}s", "returncode": None}

    return {
        "stdout": _truncate(process.stdout),
        "stderr": _truncate(process.stderr),
        "returncode": process.returncode,
    }


def _first_line(text: str, limit: int = 160) -> str:
    """First non-empty line of a text, shortened to `limit`."""
    line = next((l.strip() for l in text.splitlines() if l.strip()), "")
    return line if len(line) <= limit else line[: limit - 3] + "..."


class Toolbox:
    """Every tool of one experiment: schemas, dispatch, and per-turn state."""

    def __init__(
        self,
        *,
        database: Database,
        subsystem: Subsystem,
        runner: Runner,
        policy: PolicyFile,
        ensure_baseline: Callable[[], list[dict]],
        enable_benchmark_tool: bool = False,
        max_benchmarks_per_turn: int = 8,
        skills: list[Skill] = (),
        log=print,
    ):
        """Set up the tools for one experiment."""
        self.database = database
        self.subsystem = subsystem
        self.runner = runner
        self.policy = policy
        self.ensure_baseline = ensure_baseline
        self.enable_benchmark_tool = enable_benchmark_tool
        self.max_benchmarks_per_turn = max_benchmarks_per_turn
        self.skills = {skill.name: skill for skill in skills}
        self.log = log
        self.oqueues = {oqueue.name: oqueue for oqueue in subsystem.oqueues}
        self.extra = subsystem.extra_tools()
        self.turn_id = 0
        self.benchmarks_used = 0

    @property
    def workspace_dir(self) -> Path:
        """This turn's `run_python` workspace."""
        return self.database.workspace_dir(self.turn_id)

    def start_turn(self, turn_id: int) -> None:
        """Reset the per-turn state for a new turn.

        The data tools default to this turn id; the benchmark budget starts over.
        """
        self.turn_id = turn_id
        self.benchmarks_used = 0

    # --- tools ---------------------------------------------------------------------------------

    def _fill(self) -> dict:
        """The per-experiment values of the `{field}`s in the tools' descriptions."""
        workloads = sorted(self.subsystem.workloads)
        return {
            "policy_path": self.policy.path,
            "baseline_name": self.subsystem.baseline_name,
            "workloads": "; ".join(f"'{w}': {self.subsystem.workloads[w]}" for w in workloads),
            "skills": ", ".join(sorted(self.skills)),
            "budget": f"build_policy, run_benchmark and collect_data share a budget of {self.max_benchmarks_per_turn} calls per turn.",
            "requirements": REQUIREMENTS,
            "workspace": self.workspace_dir,
            "database": self.database.root,
        }

    def _tools(self) -> dict[str, tuple[dict, Callable[[dict], object]]]:
        """The tools this experiment offers, by name: (schema, handler of a call's raw arguments)."""
        # Built on every use: run_python's description names the current turn's workspace.
        fill = self._fill()
        tools = [
            _schema_and_handler(getattr(self, name), fill)
            for name, when in _TOOLS.items()
            if when is None or getattr(self, when)
        ]
        tools += self.extra
        return {schema["function"]["name"]: (schema, handler) for schema, handler in tools}

    def schemas(self) -> list[dict]:
        """Schemas of the tools this experiment offers."""
        return [schema for schema, _ in self._tools().values()]

    def dispatch(self, name: str, arguments: dict) -> dict:
        """Run a tool call (any tool but submit_policy)."""
        tools = self._tools()
        if name not in tools or name == "submit_policy":
            raise ToolError(f"'{name}' is not an available tool (submit_policy ends the turn instead)")
        return tools[name][1](arguments)

    def validate_submit(self, arguments: dict) -> SubmittedPolicy:
        """Check a submit_policy call and turn it into a SubmittedPolicy."""
        return self._tools()["submit_policy"][1](arguments)

    # --- database tools ------------------------------------------------------------------------

    @tool()
    def database_catalog(self) -> dict:
        """One entry per past turn of this search: its id, a one-line description of its policy,
        whether it built, and its result. Use query_database for a turn's full record."""
        return {
            "current_turn": self.turn_id,
            "turns": [
                {
                    "turn_id": t.turn_id,
                    "policy": _first_line(t.description),
                    "build_ok": bool(t.perf.get("build_ok")),
                    "result": t.perf.get("summary") or _first_line(t.perf.get("error") or "", 300),
                    "score": t.perf.get("score"),
                }
                for t in self.database.all_turns()
            ],
        }

    @tool()
    def query_database(self, turn_id: int) -> dict:
        """A past turn's full record: description, submitted code, and performance details."""
        if not self.database.has_turn(turn_id):
            raise ToolError(f"no turn {turn_id!r} in the database")
        turn = self.database.read_turn(turn_id)
        return {"turn_id": turn.turn_id, "description": turn.description, "code": turn.code, "perf": turn.perf}

    # --- OQueue tools --------------------------------------------------------------------------

    def _exports(self, boot: bool) -> list[dict] | None:
        """The OQueues the kernel exports, from the newest run's listing; with `boot`, boot once to list them."""
        root = self.database.root
        # Any run's listing counts, but not a file the agent wrote into a workspace.
        listings = sorted(
            (p for p in root.glob(f"**/{mariposa.EXPORTS_FILE}") if "workspace" not in p.relative_to(root).parts),
            key=lambda p: p.stat().st_mtime,
        )
        if not listings and boot:
            # Any boot lists them (the guest wrapper runs on every boot): use a build check of the fallback.
            out = root / "oqueue_listing"
            self.log(f"[turn {self.turn_id}] list_oqueues: booting once to list the kernel's OQueues")
            self.runner.build(self.policy.fallback, out)
            listings = sorted(out.glob(f"**/{mariposa.EXPORTS_FILE}"))
        return mariposa.read_exports(listings[-1]) if listings else None

    @tool()
    def list_oqueues(self) -> dict:
        """The OQueues (streams of kernel records) in this kernel: every OQueue it exports (path,
        Rust element type, whether it can be recorded), plus descriptions of the ones this subsystem
        documents (what one record means, its fields). collect_data can record any exported,
        observable OQueue by its path. The first call may boot the kernel once to list them."""
        exports = self._exports(boot=True) or []
        return {
            "exported": exports,
            "documented": [
                {
                    "path": o.name,
                    "description": o.description,
                    "fields": o.fields,
                    "kernel_args": list(o.kernel_args),
                    "captured_while_scoring": o.name in self.subsystem.score_capture,
                }
                for o in self.subsystem.oqueues
            ],
        }

    def _oqueue(self, name: str) -> OQueue:
        """The OQueue to capture for a path: the subsystem's description, or a plain exported one."""
        if name in self.oqueues:
            return self.oqueues[name]
        exports = self._exports(boot=False)
        if exports is not None and not any(e["path"] == name and e["observable"] for e in exports):
            raise ToolError(f"{name!r} is not an exported, observable OQueue; list_oqueues shows them")
        return OQueue(name)

    # --- submitting, analysis and skill tools --------------------------------------------------

    @tool()
    def submit_policy(
        self,
        description: Annotated[str, Field(min_length=1, description="What the policy does and why; the first line is its catalog entry.")],
        pseudocode: Annotated[str, Field(min_length=1)],
        code: Code,
    ) -> SubmittedPolicy:
        """Submit this turn's policy. Ends the turn; the policy is then built and scored."""
        return SubmittedPolicy(description, pseudocode, code)

    @tool()
    def run_python(self, code: Annotated[str, Field(min_length=1, description="Python source to execute.")]) -> dict:
        """Run arbitrary Python code in the harness's own Python environment; the packages it can
        import (e.g. pandas/scikit-learn/pytorch) are the ones listed in {requirements}, which you may
        read. Use it to collect and analyze data, train models, and save intermediate artifacts —
        datasets, correlation results, model weights, dumping scripts. This turn's writable scratch
        workspace is at {workspace} (the working directory). The full policy database is readable at
        {database}: each turn's turn_NNNNNN/ holds its description, code, perf.json, every
        benchmark's and collection's files, and its workspace/, so artifacts an earlier turn saved are
        in that turn's workspace/. These two paths and that requirements file are your entire scope:
        do not read, list, or explore anything outside them (e.g. other directories elsewhere on this
        machine, including other, unrelated experiments' results) — that data is not relevant to this
        task and exploring it only spends your budget for this turn. A call may run for up to 30
        minutes. Returns stdout, stderr, and the exit code."""
        return _run_python(code, self.workspace_dir)

    @tool(when="skills")
    def load_skill(self, name: Annotated[str, Field(description="One of: {skills}.")]) -> dict:
        """Returns the full text of one of the skills listed under Skills in your context."""
        if name not in self.skills:
            raise ToolError(f"no skill {name!r}; one of: {', '.join(sorted(self.skills))}")
        return {"name": name, "text": self.skills[name].text}

    # --- benchmarking tools --------------------------------------------------------------------

    def _spend_benchmark(self) -> None:
        """Take one call from the turn's benchmark budget."""
        if self.benchmarks_used >= self.max_benchmarks_per_turn:
            raise ToolError(f"at most {self.max_benchmarks_per_turn} build/benchmark/collect calls per turn; submit_policy to score your policy")
        self.benchmarks_used += 1

    def _candidate(self, code: str, baseline: bool) -> str | None:
        """The policy code to run, or None for the baseline."""
        if baseline:
            return None
        if not code:
            raise ToolError(f"`code` must be the complete content of {self.policy.path}, unless `baseline` is set")
        return code

    def _check_workload(self, workload: str) -> None:
        """Reject a workload the subsystem does not have."""
        if workload not in self.subsystem.workloads:
            raise ToolError(f"no workload {workload!r}; one of: {', '.join(sorted(self.subsystem.workloads))}")

    def _next_dir(self, prefix: str) -> Path:
        """Next free `<prefix>_NNNN` directory in this turn's directory."""
        turn_dir = self.database.turn_dir(self.turn_id)
        number = 1
        while (turn_dir / f"{prefix}_{number:04d}").exists():
            number += 1
        return turn_dir / f"{prefix}_{number:04d}"

    @tool(when="enable_benchmark_tool")
    def build_policy(self, code: Code) -> dict:
        """Builds the kernel with your policy and boots it on a trivial task, without benchmarking.
        Returns whether it built and booted, and the build/boot log's tail. Cheaper than
        run_benchmark for checking that code compiles. {budget}"""
        self._spend_benchmark()
        out = self._next_dir("build")
        self.log(f"[turn {self.turn_id}] build_policy -> {out.name}")
        result = self.runner.build(code, out)
        return {"build_ok": result.ok, "log_tail": result.log[-BUILD_LOG_TAIL:], "calls_left_this_turn": self._left()}

    def _left(self) -> int:
        """Calls left in the turn's benchmark budget."""
        return self.max_benchmarks_per_turn - self.benchmarks_used

    @tool(when="enable_benchmark_tool")
    def run_benchmark(
        self,
        code: Code = "",
        baseline: Annotated[bool, Field(description="Benchmark the baseline ({baseline_name}) instead of `code`.")] = False,
        workload: Workload = "eval",
        repeats: Annotated[int, Field(ge=1, le=3)] = 1,
    ) -> dict:
        """Builds (if you pass code) and benchmarks a policy without ending your turn, so you can try
        policies before you submit. Returns each run's metrics, their mean, the baseline's mean on
        'eval', and where each run's files are saved (the benchmark's raw output, readable with
        run_python). Not your turn's score: only submit_policy scores. {budget}"""
        candidate = self._candidate(code, baseline)
        self._check_workload(workload)
        self._spend_benchmark()
        baseline_runs = self.ensure_baseline()  # reported alongside, for comparison
        out = self._next_dir("bench")
        self.log(f"[turn {self.turn_id}] run_benchmark -> {out.name}: {'baseline' if candidate is None else 'policy'} on {workload}, {repeats} run(s)")
        evaluation = self.runner.evaluate(out, candidate, workload=workload, repeats=repeats)
        if not evaluation.build_ok:
            return {"build_ok": False, "log_tail": evaluation.build_log[-BUILD_LOG_TAIL:], "calls_left_this_turn": self._left()}
        if evaluation.error:
            raise ToolError(f"the benchmark failed twice: {evaluation.error[-1500:]}")
        return {
            "build_ok": True,
            "bench_id": out.name,
            "baseline": candidate is None,
            "workload": workload,
            "runs": evaluation.runs,
            "mean": means(evaluation.runs),
            f"{self.subsystem.baseline_name}_baseline_mean_on_eval": means(baseline_runs),
            "saved_under": str(out),
            "calls_left_this_turn": self._left(),
        }

    @tool(when="enable_benchmark_tool")
    def collect_data(
        self,
        oqueues: Annotated[list[str], Field(
            min_length=1,
            description="OQueue paths from list_oqueues (exported and observable, or documented by the subsystem).",
        )],
        code: Code = "",
        baseline: Annotated[bool, Field(description="Run the baseline ({baseline_name}) instead of `code`.")] = False,
        workload: Workload = "eval",
    ) -> dict:
        """Runs one benchmark of a policy (or the baseline) while capturing the given OQueues, and
        saves each OQueue's records as JSON lines for run_python. Returns the collection id, each
        file's path and record count, the run's metrics, and its directory (which also holds the
        benchmark's raw output, readable with run_python). {budget}"""
        capture = [self._oqueue(n) for n in oqueues]
        candidate = self._candidate(code, baseline)
        self._check_workload(workload)
        self._spend_benchmark()
        out = self._next_dir("collect")
        self.log(f"[turn {self.turn_id}] collect_data -> {out.name}: {oqueues} from {'baseline' if candidate is None else 'policy'} on {workload}")
        evaluation = self.runner.evaluate(out, candidate, workload=workload, capture=capture)
        if not evaluation.build_ok:
            return {"build_ok": False, "log_tail": evaluation.build_log[-BUILD_LOG_TAIL:], "calls_left_this_turn": self._left()}
        if evaluation.error:
            raise ToolError(f"the run failed twice: {evaluation.error[-1500:]}")
        run_dir = evaluation.run_dirs[-1]
        files = {}
        for n in oqueues:
            path = run_dir / f"{n}.jsonl"
            files[n] = {"path": str(path), "records": sum(1 for _ in path.open()) if path.exists() else 0}
        return {
            "build_ok": True,
            "turn_id": self.turn_id,
            "collection": out.name,
            "files": files,
            "metrics": evaluation.runs[-1],
            "run_dir": str(run_dir),
            "calls_left_this_turn": self._left(),
        }
