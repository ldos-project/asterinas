# SPDX-License-Identifier: MPL-2.0

"""Copy the winning policy out of a results directory: `best_policy.py RESULTS_DIR --out DIR`."""

# Writes the policy file plus `best_turn.json`. The winner is the first built turn whose
# verdict was `satisfied` (the turn that stopped the search); failing that, `Database.current_best()`.

import argparse
import json
import sys
from pathlib import Path

import yaml

from orchestrator.database import Database
from run_experiment import load_subsystem


def main():
    """Find the winning turn and write its policy to `--out`."""
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("results_dir", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()

    # Only the subsystem and its params, which every saved config has at the top level, older flat ones too.
    config = yaml.safe_load((args.results_dir / "config.yaml").read_text())
    subsystem = load_subsystem(config["subsystem"], config.get("params", {}))
    database = Database(args.results_dir, Path(subsystem.policy_file).name, subsystem.higher_is_better)
    best = next((t for t in database.all_turns() if t.perf.get("build_ok") and t.perf.get("satisfied")), None)
    selected_by = "first turn that satisfied the subsystem's criteria"
    if best is None:
        best = database.current_best()
        selected_by = "fallback: no turn satisfied the criteria; best score"
    # If the current_best() also says non, that means none of the generated policy even build. 
    if best is None:
        sys.exit(f"{args.results_dir} has no successfully built turn")
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / database.policy_name).write_text(best.code)
    (args.out / "best_turn.json").write_text(json.dumps(
        {"results_dir": args.results_dir.name, "turn_id": best.turn_id, "selected_by": selected_by,
         "summary": best.perf["summary"], "score": best.perf["score"], "description": best.description}, indent=2))
    print(f"{args.results_dir.name}: turn {best.turn_id} ({selected_by}), {best.perf['summary']}")


if __name__ == "__main__":
    main()
