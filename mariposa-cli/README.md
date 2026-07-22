# mariposa-cli

Host-side tooling for the Mariposa interlayer. A single umbrella command,
`mariposa`, dispatches to per-component subcommands; the first component is
**`oqueues`**, which accesses the Mariposa OQueue File System (`/oqueues`) and
also ships an MCP server for AI agents. More components mount the same way.

Everything runs on the **host** (the agent's domain) and reaches the Mariposa
guest over SSH; the guest stays passive and only runs basic tools (`tree`,
`find`, `cat`). All CBOR decoding and dataframe construction happen host-side to
reduce the impact to the guest.

## CLI

```bash
mariposa oqueues list                                       # OQueues as JSON
mariposa oqueues collect scheduler/events --max-records 100 --format json
mariposa oqueues stream  scheduler/events                   # live tail (Ctrl-C)
```

From a checkout you can also run it in-tree without installing, via the
repo-root launcher:

```bash
./mariposa oqueues list
```

### `oqueues` subcommands

| Command                | Purpose                                                     |
|------------------------|-------------------------------------------------------------|
| `tree`                 | Human-readable `tree` of `/oqueues`.                        |
| `list`                 | Machine-readable JSON list of OQueues.                      |
| `metadata <path>`      | Print an OQueue's `metadata.yaml`.                          |
| `collect <path> …`     | Bounded drain → CSV/JSON (`--max-records`, `--timeout`, `--format`). |
| `stream <path> …`      | Live-tail as newline-delimited JSON (Ctrl-C to stop).       |
| `serve`                | Run the OQueues MCP server over stdio.                      |

`collect` requires a bound (`--max-records` or `--timeout`); for an unbounded
drain use `stream`. The stateful MCP session tools collapse into the single live
`stream` command, since a one-shot process has no session to poll.

## MCP server (`oqueues` component)

The `oqueues` component also exposes an MCP server, installed as the
`mariposa-oqueues-mcp` console script (equivalent to `mariposa oqueues serve`).

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

## Install

```bash
python -m venv .venv && . .venv/bin/activate
pip install -e .          # provides `mariposa` and `mariposa-oqueues-mcp`
```

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
