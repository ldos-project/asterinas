# SPDX-License-Identifier: MPL-2.0

"""Store and read an experiment's turns and baseline."""

# The results directory is the search's only persisted state: resuming re-reads it, never replays a
# conversation. Layout:
# - `index.jsonl`: the turn log, and the record of which turns exist. A turn appends
#   `{"turn_id": N, "finished": false}` when it starts and its summary with `"finished": true` once
#   it is recorded; a turn's last line is its state. A turn that crashed stays unfinished, and the
#   next turn takes a new id.
# - `turn_NNNNNN/`: everything one turn uses. While it runs: its benchmarks (`build_NNNN/`,
#   `bench_NNNN/`, `collect_NNNN/`, `score/`, `confirm/`), its `run_python` `workspace/`, and
#   `usage.json`, the tokens spent so far (rewritten after every LLM call). Once recorded:
#   `description.md`, the submitted policy file (named like the kernel's), `perf.json`,
#   `transcript.json`.
# - `baseline/` and `baseline.json`: the baseline's runs and per-run metrics, benchmarked once and reused.

import json
from dataclasses import dataclass
from pathlib import Path


@dataclass
class TurnRecord:
    """One finished turn."""

    turn_id: int
    description: str
    code: str
    perf: dict
    usage: dict


class Database:
    """An experiment's results directory."""

    def __init__(self, root: Path, policy_name: str, higher_is_better: bool = False):
        """Open (or create) the results directory at `root`."""
        # `policy_name`: the policy file's name, so a turn's copy opens with the right syntax.
        # `higher_is_better`: the subsystem's score direction.
        self.root = Path(root)
        self.root.mkdir(parents=True, exist_ok=True)
        self.policy_name = policy_name
        self.higher_is_better = higher_is_better

    def turn_dir(self, turn_id: int) -> Path:
        """Directory of everything a turn uses."""
        return self.root / f"turn_{turn_id:06d}"

    def workspace_dir(self, turn_id: int) -> Path:
        """A turn's `run_python` scratch space."""
        return self.turn_dir(turn_id) / "workspace"

    def baseline_dir(self) -> Path:
        """Directory of the baseline's runs."""
        return self.root / "baseline"

    def start_turn(self) -> int:
        """Start the next turn; return its id."""
        turn_id = max(self._index(), default=0) + 1
        self.turn_dir(turn_id).mkdir()
        self._append_index_line({"turn_id": turn_id, "finished": False})
        return turn_id

    def write_turn(self, record: TurnRecord, transcript: list | None = None) -> None:
        """Record a finished turn."""
        # The index line comes last, so a crash part-way leaves the turn unfinished.
        turn_dir = self.turn_dir(record.turn_id)
        turn_dir.mkdir(parents=True, exist_ok=True)
        (turn_dir / "description.md").write_text(record.description)
        if record.code:
            (turn_dir / self.policy_name).write_text(record.code)
        (turn_dir / "perf.json").write_text(json.dumps(record.perf, indent=2))
        self.write_usage(record.turn_id, record.usage)
        if transcript:
            (turn_dir / "transcript.json").write_text(json.dumps(transcript, indent=2))
        self._append_index_line({
            "turn_id": record.turn_id,
            "finished": True,
            "build_ok": record.perf.get("build_ok"),
            "score": record.perf.get("score"),
            "satisfied": record.perf.get("satisfied"),
            "tokens_used": record.usage.get("total_tokens", 0),
        })

    def read_turn(self, turn_id: int) -> TurnRecord:
        """Read a finished turn."""
        turn_dir = self.turn_dir(turn_id)
        if not turn_dir.is_dir():
            raise FileNotFoundError(f"turn {turn_id} is recorded in index.jsonl, but {turn_dir} is missing")
        return TurnRecord(
            turn_id=turn_id,
            description=(turn_dir / "description.md").read_text(),
            code=(turn_dir / self.policy_name).read_text() if (turn_dir / self.policy_name).exists() else "",
            perf=json.loads((turn_dir / "perf.json").read_text()),
            usage=json.loads((turn_dir / "usage.json").read_text()),
        )

    def write_usage(self, turn_id: int, usage: dict) -> None:
        """Store the tokens a turn has spent so far."""
        # Written to a temporary file and renamed, so a kill mid-write can't leave it truncated.
        path = self.turn_dir(turn_id) / "usage.json"
        temporary = path.with_suffix(".json.tmp")
        temporary.write_text(json.dumps(usage, indent=2))
        temporary.replace(path)

    def has_turn(self, turn_id: int) -> bool:
        """Whether a turn has finished."""
        return self._index().get(turn_id, {}).get("finished", False)

    def all_turns(self) -> list[TurnRecord]:
        """Every finished turn, in order."""
        return [self.read_turn(turn_id) for turn_id, line in sorted(self._index().items()) if line["finished"]]

    def current_best(self) -> TurnRecord | None:
        """The built turn with the best score, if any."""
        candidates = [t for t in self.all_turns() if t.perf.get("build_ok")]
        if not candidates:
            return None
        pick = max if self.higher_is_better else min
        return pick(candidates, key=lambda t: t.perf["score"])

    def total_tokens_used(self) -> int:
        """Tokens spent by every turn so far, including turns that crashed."""
        paths = (self.turn_dir(turn_id) / "usage.json" for turn_id in self._index())
        return sum(json.loads(path.read_text()).get("total_tokens", 0) for path in paths if path.exists())

    def write_baseline(self, runs: list[dict]) -> None:
        """Store the baseline's per-run metrics."""
        # Persisted so a resumed experiment reuses it instead of re-benchmarking.
        (self.root / "baseline.json").write_text(json.dumps(runs, indent=2))

    def read_baseline(self) -> list[dict] | None:
        """The baseline's per-run metrics, if benchmarked yet."""
        baseline_path = self.root / "baseline.json"
        if not baseline_path.exists():
            return None
        return json.loads(baseline_path.read_text())

    def _index(self) -> dict[int, dict]:
        """Each turn's last line in `index.jsonl`."""
        index_path = self.root / "index.jsonl"
        if not index_path.exists():
            return {}
        lines = (json.loads(line) for line in index_path.read_text().splitlines())
        return {line["turn_id"]: line for line in lines}

    def _append_index_line(self, line: dict) -> None:
        """Add a line to `index.jsonl`."""
        with (self.root / "index.jsonl").open("a") as f:
            f.write(json.dumps(line) + "\n")
