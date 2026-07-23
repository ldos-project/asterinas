# mariposa-cli

Host-side tooling for the Mariposa interlayer. A single umbrella command,
`mariposa-cli`, dispatches to per-component subcommands, each a self-contained
interface to one part of Mariposa:

- **`oqueues`** — connect to the guest's OQueue File System (OQFS); also ships
  an MCP server for AI agents.
- **`data_capture`** — *(planned)* capture Mariposa guest data.
- **`mlops`** — *(planned)* Mariposa MLOps workflows.

More components mount the same way.

## Install

The CLI must be installed before use. 

```bash
cd mariposa-cli
python3 -m venv .venv && . .venv/bin/activate
pip install -e .          # installs mariposa-cli AND its dependencies
```

That single command installs the runtime dependencies (`cbor2`, `polars`,
`mcp`) and installs the `mariposa_cli` package. Because the install is editable (`-e`), it links this working tree, so source edits take effect without reinstalling. Run `mariposa-cli` from anywhere, including the repo root.

## CLI

Every command is `mariposa-cli <component> <subcommand> …`.

### `oqueues`

An interface to the guest's **OQueue File System** (OQFS, `/oqueues`).
Everything runs on the **host** (the agent's domain) and reaches the Mariposa
guest over SSH; the guest stays passive and only runs basic tools (`tree`,
`find`, `cat`). All CBOR decoding and dataframe construction happen host-side to
reduce the impact to the guest.

```bash
mariposa-cli oqueues list                                   # OQueues as JSON
mariposa-cli oqueues collect scheduler/events --max-records 100 --format json
mariposa-cli oqueues stream  scheduler/events               # live tail (Ctrl-C)
```

| Command                | Purpose                                                     |
|------------------------|-------------------------------------------------------------|
| `tree`                 | Human-readable `tree` of `/oqueues`.                        |
| `list`                 | Machine-readable JSON list of OQueues.                      |
| `metadata <path>`      | Print an OQueue's `metadata.yaml`.                          |
| `collect <path> …`     | Bounded drain → CSV/JSON (`--max-records`, `--timeout`, `--format`, `--output`). |
| `stream <path> …`      | Live-tail as newline-delimited JSON (Ctrl-C to stop).       |
| `serve`                | Run the OQueues MCP server over stdio.                      |

`collect` requires a bound (`--max-records` or `--timeout`); for an unbounded
drain use `stream`. The stateful MCP session tools collapse into the single live
`stream` command, since a one-shot process has no session to poll.

### `data_capture`

*Planned — not yet implemented.* Will add `mariposa-cli data_capture …`
subcommands for capturing Mariposa guest data. TODO: describe once the
component lands.

### `mlops`

*Planned — not yet implemented.* Will add `mariposa-cli mlops …` subcommands for
Mariposa MLOps workflows. TODO: describe once the component lands.

## MCP server (`oqueues` component)

The `oqueues` component also exposes an MCP server, installed as the
`mariposa-oqueues-mcp` console script (equivalent to `mariposa-cli oqueues serve`).

| Tool             | Purpose                                                   |
|------------------|-----------------------------------------------------------|
| `list_tree`      | Human-readable `tree` of `/oqueues`.                      |
| `list_oqueues`   | Machine-readable JSON list of OQueues.                    |
| `read_metadata`  | Read an OQueue's `metadata.yaml`.                         |
| `stream_collect` | Bounded drain (max_records / timeout) → CSV/JSON.         |
| `stream_start`   | Begin a session (bounded or infinite) → `stream_id`.      |
| `stream_read`    | Records accumulated since the last read.                  |
| `stream_stop`    | Kill signal for a streaming session.                      |
| `stream_list`    | Active sessions.                                          |

## Configuration (env)

Both front ends read the same variables.

| Var                | Default          | Meaning                              |
|--------------------|------------------|--------------------------------------|
| `OQ_TRANSPORT`     | `ssh`            | `ssh` or `local` (host, for tests).  |
| `OQ_SSH_HOST`      | `127.0.0.1`      | Guest SSH host (QEMU-forwarded).     |
| `OQ_SSH_PORT`      | `61541`          | Forwarded SSH port (`SSH_PORT`).     |
| `OQ_SSH_USER`      | `root`           | SSH user.                            |
| `OQ_SSH_KEY`       | —                | SSH private key path (optional).     |
| `OQ_ROOT`          | `/oqueues`       | OQFS root.                            |
| `OQ_METADATA_FILE` | `metadata.yaml`  | Metadata filename.                   |
| `OQ_COMMAND_TIMEOUT` | `30.0`         | Per-command timeout, seconds.        |

## Register the MCP server with Claude CLI

The server speaks MCP over **stdio** by default, so you do **not** background it
yourself — the Claude CLI launches it on demand and manages its lifecycle. Just
register the installed binary once:

```bash
claude mcp add oqueues -s user \
  -e OQ_SSH_HOST=127.0.0.1 \
  -e OQ_SSH_PORT=61541 \
  -- $ASTERINAS_HOME/mariposa-cli/.venv/bin/mariposa-oqueues-mcp
```

Boot the kernel (with `SSH_PORT` overridden), then any `claude` session gets the
`oqueues` tools. Verify with `claude mcp list` or `/mcp` inside a session.

## Test

Uses `OQ_TRANSPORT=local` against a fake `/oqueues` tree — no kernel needed.

```bash
. .venv/bin/activate && pip install -e '.[test]'
PYTHONPATH="$PWD:$PWD/tests" pytest -q
```
