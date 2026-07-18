# oqueues-mcp

MCP server that exposes the Mariposa OQueue File System (`/oqueues`) to AI
agents. It runs on the **host** (the agent's domain) and reaches the Mariposa
guest over SSH; the guest stays passive and only runs stock tools (`tree`,
`find`, `cat`). All CBOR decoding and dataframe construction happen host-side.

Design: [../documents/oqueues-mcp-design.md](../documents/oqueues-mcp-design.md).

## Tools

| Tool             | Purpose                                                   |
|------------------|-----------------------------------------------------------|
| `list_tree`      | Human-readable `tree` of `/oqueues`.                      |
| `list_oqueues`   | Machine-readable JSON list of OQueues.                    |
| `read_metadata`  | An OQueue's `metadata.yaml`.                              |
| `stream_collect` | Bounded drain (max_records / timeout) → CSV/JSON.        |
| `stream_start`   | Begin a session (bounded or infinite) → `stream_id`.     |
| `stream_read`    | Records accumulated since the last read.                  |
| `stream_stop`    | Kill signal for an infinite stream.                      |
| `stream_list`    | Active sessions.                                          |

## Configuration (env)

| Var                | Default          | Meaning                              |
|--------------------|------------------|--------------------------------------|
| `OQ_TRANSPORT`     | `ssh`            | `ssh` or `local` (host, for tests).  |
| `OQ_SSH_HOST`      | `127.0.0.1`      | Guest SSH host (QEMU-forwarded).     |
| `OQ_SSH_PORT`      | `61541`          | Forwarded SSH port (`SSH_PORT`).     |
| `OQ_SSH_USER`      | `root`           | SSH user.                            |
| `OQ_SSH_KEY`       | —                | SSH private key path (optional).     |
| `OQ_ROOT`          | `/oqueues`       | OQFS root.                            |
| `OQ_METADATA_FILE` | `metadata.yaml`  | Metadata filename.                   |

## Install

```bash
python -m venv .venv && . .venv/bin/activate
pip install -e .          # provides the `oqueues-mcp` console script
```

## Spin it up for an agent (Claude CLI)

The server speaks MCP over **stdio** by default, so you do **not** background it
yourself — the Claude CLI launches it on demand and manages its lifecycle. Just
register the installed binary once:

```bash
claude mcp add oqueues -s user \
  -e OQ_SSH_HOST=127.0.0.1 \
  -e OQ_SSH_PORT=61541 \
  -- /var/local/ycao/asterinas.mcp/oqueues-mcp/.venv/bin/oqueues-mcp
```

Boot the kernel (with `SSH_PORT` overridden), then any `claude` session gets the
`oqueues` tools. Verify with `claude mcp list` or `/mcp` inside a session.

### Alternative: long-running background server (HTTP)

For a shared/always-on server — or later, when the agent runs in another VM or
machine — run it over HTTP and register the URL instead:

```bash
OQ_MCP_TRANSPORT=streamable-http OQ_MCP_PORT=8765 \
OQ_SSH_PORT=61541 oqueues-mcp &          # backgrounded, long-running

claude mcp add oqueues -s user --transport http http://127.0.0.1:8765/mcp
```

## Test

Uses `OQ_TRANSPORT=local` against a fake `/oqueues` tree — no kernel needed.

```bash
. .venv/bin/activate && pip install -e '.[test]'
PYTHONPATH="$PWD:$PWD/tests" pytest -q
```
