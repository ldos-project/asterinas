# Policy Explorer

An agentic search loop for Mariposa kernel policies: each turn an OpenAI-compatible LLM writes a
policy, the harness builds the kernel with it, boots it in QEMU, runs a benchmark, and compares it
with a baseline; the next turn sees every earlier result. Every turn costs a real kernel build and
real benchmark runs.

- [QUICK_START.md](QUICK_START.md): how to run it and how to port it to a new subsystem (for people).
- [AGENTS.md](AGENTS.md): how the code is organized and how to implement a subsystem (for coding agents).
- [SPEC.md](SPEC.md): the template a user fills in to specify a new subsystem.

```bash
cd policy_explorer
../.venv/bin/pip install -r requirements.txt
../.venv/bin/python3 run_experiment.py subsystems/<name>/configs/<search>.yaml
```

## Resuming a search

Re-running the same command resumes the search. Its results directory,
`results/<search>_<label>_<model>/`, is its only state, so a search stopped by Ctrl-C, a crash or a
reboot picks up from there:

- Finished turns are kept, and the next turn's prompt is rebuilt from them; no conversation is
  replayed.
- The baseline is benchmarked once (`baseline.json`) and reused.
- If a killed run left the agent's code in the policy file, the fallback is put back before
  anything else.
- The subsystem's `prepare` runs again, so it must be idempotent.
- `max_turns` counts finished turns and `max_total_tokens` the tokens of every turn, so raising
  them in the config and re-running extends a search that has stopped.
- Changing the model, or a param that is part of the subsystem's label, starts a new results
  directory instead.

`index.jsonl` records whether each turn finished. A turn appends `{"turn_id": N, "finished": false}`
when it starts, and a line with `"finished": true` (and its `build_ok`, `score`, `satisfied`,
`tokens_used`) once it is recorded. A turn's last line is its state, so a turn whose last line says
`"finished": false` crashed. Its directory is left as it was for debugging, and it is not retried.
The tokens it spent still count against `max_total_tokens`: every turn rewrites its `usage.json`
after each LLM call. The next turn takes a new number, so turn numbers can skip.

## Gotchas

- **Baselines are per experiment.** Each search benchmarks its own baseline once and reuses it. If
  you change params partway through, old turns keep the old baseline; `perf.json` records `params`
  per turn so this can be told apart. Copy `baseline.json` into a new results directory before its
  first turn to share a baseline across searches.
- **A turn that never calls `submit_policy` is not a bug.** It is recorded as a failed turn and the
  search moves on; it is a prompt/budget problem.
