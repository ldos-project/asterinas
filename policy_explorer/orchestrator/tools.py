# SPDX-License-Identifier: MPL-2.0

"""The tools the agent can call during a turn."""

# Always offered: read the database, list OQueues, `run_python` (the agent's way to analyze captured
# data and everything else on disk), submit. The kernel-booting tools (`build_policy`,
# `run_benchmark`, `collect_data`) come with `enable_benchmark_tool` and share a per-turn budget.
# `load_skill` comes with a search's `skills`.

import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Annotated, Callable, Literal

from pydantic import BaseModel, Field, ValidationError

from orchestrator import mariposa
from orchestrator.database import Database
from orchestrator.evaluation import Runner
from orchestrator.policy_file import PolicyFile
from orchestrator.skills import Skill
from orchestrator.stats import means
from orchestrator.subsystem import OQueue, Subsystem

BUILD_LOG_TAIL = 4000


class ToolError(Exception):
    """A tool call that failed; its message goes back to the agent."""


@dataclass
class SubmittedPolicy:
    """A policy the agent submitted to end its turn."""

    description: str
    pseudocode: str
    code: str


def _tool(name: str, description: str, arguments: type[BaseModel], handler: Callable) -> tuple[dict, Callable[[dict], object]]:
    """An OpenAI function tool whose `arguments` model gives its schema and checks its calls."""
    # The returned handler takes a call's raw arguments; `handler` gets them parsed.
    def call(raw: dict):
        try:
            parsed = arguments.model_validate(raw)
        except ValidationError as error:
            raise ToolError("; ".join(f"{name}'s '{'.'.join(map(str, e['loc']))}': {e['msg']}" for e in error.errors())) from None
        return handler(parsed)

    schema = {"type": "function", "function": {"name": name, "description": description, "parameters": arguments.model_json_schema()}}
    return schema, call


def _truncate(text: str, limit: int = 4_000) -> str:
    """Cut long tool output to `limit` characters."""
    if len(text) <= limit:
        return text
    return text[:limit] + f"\n...[truncated, {len(text) - limit} more characters]"


def _run_python(code: str, workspace_dir: Path, run=subprocess.run, timeout: float = 1800) -> dict:
    """Run the agent's Python code in its workspace."""
    # Not sandboxed: the tool description scopes the agent to its workspace and the database.
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
        """Reset the per-turn state for a new turn."""
        # The data tools default to this turn id; the benchmark budget starts over.
        self.turn_id = turn_id
        self.benchmarks_used = 0

    # --- tools ---------------------------------------------------------------------------------

    def _tools(self) -> dict[str, tuple[dict, Callable[[dict], object]]]:
        """The tools this experiment offers, by name: (schema, handler of a call's raw arguments)."""
        # Built on every use: the argument models carry this experiment's policy file, workloads and
        # skills, and run_python's description names the current turn's workspace.
        Code = Annotated[str, Field(min_length=1, description=f"The complete new content of the policy file, {self.policy.path}.")]
        workloads = sorted(self.subsystem.workloads)
        Workload = Annotated[
            Literal[tuple(workloads)],
            Field(description="; ".join(f"'{w}': {self.subsystem.workloads[w]}" for w in workloads) + " (default 'eval')."),
        ]
        baseline_name = self.subsystem.baseline_name

        class NoArguments(BaseModel):
            pass

        class QueryDatabase(BaseModel):
            turn_id: int

        class SubmitPolicy(BaseModel):
            description: str = Field(min_length=1, description="What the policy does and why; the first line is its catalog entry.")
            pseudocode: str = Field(min_length=1)
            code: Code

        class RunPython(BaseModel):
            code: str = Field(min_length=1, description="Python source to execute.")

        tools = [
            _tool(
                "database_catalog",
                "One entry per past turn of this search: its id, a one-line description of its policy, "
                "whether it built, and its result. Use query_database for a turn's full record.",
                NoArguments,
                self._database_catalog,
            ),
            _tool(
                "query_database",
                "A past turn's full record: description, submitted code, and performance details.",
                QueryDatabase,
                self._query_database,
            ),
            _tool(
                "list_oqueues",
                "The OQueues (streams of kernel records) in this kernel: every OQueue it exports (path, "
                "Rust element type, whether it can be recorded), plus descriptions of the ones this "
                "subsystem documents (what one record means, its fields). collect_data can record any "
                "exported, observable OQueue by its path. The first call may boot the kernel once to list them.",
                NoArguments,
                self._list_oqueues,
            ),
            _tool(
                "submit_policy",
                "Submit this turn's policy. Ends the turn; the policy is then built and scored.",
                SubmitPolicy,
                lambda args: SubmittedPolicy(args.description, args.pseudocode, args.code),
            ),
            _tool(
                "run_python",
                "Run arbitrary Python code (any installed package, e.g. pandas/scikit-learn/"
                "pytorch). Use it to collect and analyze data, train models, and save "
                "intermediate artifacts — datasets, correlation results, model weights, dumping "
                "scripts. "
                f"This turn's writable scratch workspace is at {self.workspace_dir} (the working directory). "
                f"The full policy database is readable at {self.database.root}: each turn's "
                "turn_NNNNNN/ holds its description, code, perf.json, every benchmark's and "
                "collection's files, and its workspace/, so artifacts an earlier turn saved are in "
                "that turn's workspace/. "
                "These two paths are your entire scope: do not read, list, or explore anything "
                "outside them (e.g. other directories elsewhere on this machine, including other, "
                "unrelated experiments' results) — that data is not relevant to this task and "
                "exploring it only spends your budget for this turn. "
                "A call may run for up to 30 minutes. "
                "Returns stdout, stderr, and the exit code.",
                RunPython,
                lambda args: _run_python(args.code, self.workspace_dir),
            ),
        ]
        if self.skills:
            class LoadSkill(BaseModel):
                name: Literal[tuple(sorted(self.skills))]

            tools.append(_tool(
                "load_skill",
                "Returns the full text of one of the skills listed under Skills in your context.",
                LoadSkill,
                lambda args: {"name": args.name, "text": self.skills[args.name].text},
            ))
        if self.enable_benchmark_tool:
            class BuildPolicy(BaseModel):
                code: Code

            class RunBenchmark(BaseModel):
                code: Code = ""
                baseline: bool = Field(False, description=f"Benchmark the baseline ({baseline_name}) instead of `code`.")
                workload: Workload = "eval"
                repeats: int = Field(1, ge=1, le=3)

            class CollectData(BaseModel):
                oqueues: list[str] = Field(
                    min_length=1,
                    description="OQueue paths from list_oqueues (exported and observable, or documented by the subsystem).",
                )
                code: Code = ""
                baseline: bool = Field(False, description=f"Run the baseline ({baseline_name}) instead of `code`.")
                workload: Workload = "eval"

            budget = f"build_policy, run_benchmark and collect_data share a budget of {self.max_benchmarks_per_turn} calls per turn."
            tools += [
                _tool(
                    "build_policy",
                    "Builds the kernel with your policy and boots it on a trivial task, without "
                    "benchmarking. Returns whether it built and booted, and the build/boot log's tail. "
                    "Cheaper than run_benchmark for checking that code compiles. " + budget,
                    BuildPolicy,
                    self._build_policy,
                ),
                _tool(
                    "run_benchmark",
                    "Builds (if you pass code) and benchmarks a policy without ending your turn, so you can "
                    "try policies before you submit. Returns each run's metrics, their mean, the baseline's "
                    "mean on 'eval', and where each run's files are saved (the benchmark's raw output, readable with "
                    "run_python). Not your turn's score: only "
                    "submit_policy scores. " + budget,
                    RunBenchmark,
                    self._run_benchmark,
                ),
                _tool(
                    "collect_data",
                    "Runs one benchmark of a policy (or the baseline) while capturing the given OQueues, "
                    "and saves each OQueue's records as JSON lines for run_python. "
                    "Returns the collection id, each file's path and record count, the run's metrics, and its directory "
                    "(which also holds the benchmark's raw output, readable with run_python). " + budget,
                    CollectData,
                    self._collect_data,
                ),
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

    def _database_catalog(self, arguments) -> dict:
        """Tool: one line per past turn."""
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

    def _query_database(self, arguments) -> dict:
        """Tool: a past turn's full record."""
        if not self.database.has_turn(arguments.turn_id):
            raise ToolError(f"no turn {arguments.turn_id!r} in the database")
        turn = self.database.read_turn(arguments.turn_id)
        return {"turn_id": turn.turn_id, "description": turn.description, "code": turn.code, "perf": turn.perf}

    # --- OQueue tools --------------------------------------------------------------------------

    def _exports(self, boot: bool) -> list[dict] | None:
        """The OQueues the kernel exports, from the newest run's listing; with `boot`, boot once to list them."""
        # Any run's listing, but not a file the agent wrote into a workspace.
        root = self.database.root
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

    def _list_oqueues(self, arguments) -> dict:
        """Tool: the kernel's exported OQueues and the subsystem's documented ones."""
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

    # --- benchmarking tools --------------------------------------------------------------------

    def _spend_benchmark(self) -> None:
        """Take one call from the turn's benchmark budget."""
        if self.benchmarks_used >= self.max_benchmarks_per_turn:
            raise ToolError(f"at most {self.max_benchmarks_per_turn} build/benchmark/collect calls per turn; submit_policy to score your policy")
        self.benchmarks_used += 1

    def _candidate(self, arguments) -> str | None:
        """The policy code to run, or None for the baseline."""
        if arguments.baseline:
            return None
        if not arguments.code:
            raise ToolError(f"`code` must be the complete content of {self.policy.path}, unless `baseline` is set")
        return arguments.code

    def _next_dir(self, prefix: str) -> Path:
        """Next free `<prefix>_NNNN` directory in this turn's directory."""
        turn_dir = self.database.turn_dir(self.turn_id)
        number = 1
        while (turn_dir / f"{prefix}_{number:04d}").exists():
            number += 1
        return turn_dir / f"{prefix}_{number:04d}"

    def _build_policy(self, arguments) -> dict:
        """Tool: build and smoke-boot a policy."""
        self._spend_benchmark()
        out = self._next_dir("build")
        self.log(f"[turn {self.turn_id}] build_policy -> {out.name}")
        result = self.runner.build(arguments.code, out)
        return {"build_ok": result.ok, "log_tail": result.log[-BUILD_LOG_TAIL:], "calls_left_this_turn": self._left()}

    def _left(self) -> int:
        """Calls left in the turn's benchmark budget."""
        return self.max_benchmarks_per_turn - self.benchmarks_used

    def _run_benchmark(self, arguments) -> dict:
        """Tool: benchmark a policy or the baseline without ending the turn."""
        code = self._candidate(arguments)
        workload, repeats = arguments.workload, arguments.repeats
        self._spend_benchmark()
        baseline_runs = self.ensure_baseline()  # reported alongside, for comparison
        out = self._next_dir("bench")
        self.log(f"[turn {self.turn_id}] run_benchmark -> {out.name}: {'baseline' if code is None else 'policy'} on {workload}, {repeats} run(s)")
        evaluation = self.runner.evaluate(out, code, workload=workload, repeats=repeats)
        if not evaluation.build_ok:
            return {"build_ok": False, "log_tail": evaluation.build_log[-BUILD_LOG_TAIL:], "calls_left_this_turn": self._left()}
        if evaluation.error:
            raise ToolError(f"the benchmark failed twice: {evaluation.error[-1500:]}")
        return {
            "build_ok": True,
            "bench_id": out.name,
            "baseline": code is None,
            "workload": workload,
            "runs": evaluation.runs,
            "mean": means(evaluation.runs),
            f"{self.subsystem.baseline_name}_baseline_mean_on_eval": means(baseline_runs),
            "saved_under": str(out),
            "calls_left_this_turn": self._left(),
        }

    def _collect_data(self, arguments) -> dict:
        """Tool: one benchmark run that captures the given OQueues."""
        names = arguments.oqueues
        capture = [self._oqueue(n) for n in names]
        code = self._candidate(arguments)
        workload = arguments.workload
        self._spend_benchmark()
        out = self._next_dir("collect")
        self.log(f"[turn {self.turn_id}] collect_data -> {out.name}: {names} from {'baseline' if code is None else 'policy'} on {workload}")
        evaluation = self.runner.evaluate(out, code, workload=workload, capture=capture)
        if not evaluation.build_ok:
            return {"build_ok": False, "log_tail": evaluation.build_log[-BUILD_LOG_TAIL:], "calls_left_this_turn": self._left()}
        if evaluation.error:
            raise ToolError(f"the run failed twice: {evaluation.error[-1500:]}")
        run_dir = evaluation.run_dirs[-1]
        files = {}
        for n in names:
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
