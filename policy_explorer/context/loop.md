# The policy search

Each turn, you propose one policy, implement it as the subsystem context below describes, and
submit it with `submit_policy`; it will be built and benchmarked for you and compared with the
baseline. You'll see the result, with every earlier turn's, and can try again. Your turn is scored
only on what you `submit_policy`; a turn records a result only when it ends with that call.

## Tools

Besides reading past turns (`database_catalog` for one line per turn, `query_database` for a
turn's full record) and submitting a policy, you have these tools.

`run_python` runs arbitrary Python code (any installed package, e.g. pandas/numpy/scikit-learn/
pytorch) in this turn's scratch workspace, and can read this search's own policy database (every
past turn's description, code, performance data, every benchmark's and collection's files, and its
workspace). Use it to analyze data, train models, and save intermediate artifacts. Check earlier
turns' workspaces for artifacts they saved before recomputing them.

`run_benchmark` builds and benchmarks a policy without ending your turn, so you can try policies
before you submit one. With `baseline=true` it benchmarks the baseline; otherwise pass your Rust in
`code`. `workload='eval'` is the workload your submitted policies are scored on. 

`build_policy` only builds and boots your policy, to check that it compiles and runs.

Every benchmark run leaves its raw files in its own directory (the tool results give the paths:
`saved_under`, `run_dir`), readable with `run_python`: the benchmark's output, which the subsystem
context describes, and, for runs that record OQueues, one `<path>.jsonl` per OQueue. All of a
turn's runs are under its `turn_NNNNNN/` (`bench_*`, `collect_*`, `build_*`, and `score/` for the
scored evaluation), next to its `workspace/`.

The OQueue tools (`list_oqueues`, `collect_data`) and how to read their recordings are described
under "Data capture" below.

Stay within your workspace and this search's own database — do not explore other directories on
this machine, including other, unrelated experiments' results; that data is not relevant here.
