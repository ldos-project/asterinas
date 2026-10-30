# SPDX-License-Identifier: MPL-2.0

"""Reachability test: sends one tiny tool-calling request to the backend/model named in a search
config, using the same client construction as `run_experiment.py`. Costs a few tokens, no build.

    ../.venv/bin/python3 check_model.py subsystems/<name>/configs/<search>.yaml
"""

import argparse
import os
import sys
from pathlib import Path

from run_experiment import BACKENDS, load_config, make_client

PING_TOOL = {
    "type": "function",
    "function": {
        "name": "ping",
        "description": "Call this tool to confirm tool calling works.",
        "parameters": {"type": "object", "properties": {}},
    },
}


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("config", type=Path)
    args = parser.parse_args(argv)

    config = load_config(args.config)
    client = make_client(config)
    endpoint = BACKENDS[config.loop.backend][1] or os.environ.get("OPENAI_BASE_URL") or "SDK default"
    print(f"Testing Tool Calling ")
    print(f"backend={config.loop.backend} model={config.loop.model} endpoint={endpoint}")
    try:
        message, usage = client.chat(
            messages=[{"role": "user", "content": "Call the ping tool."}], tools=[PING_TOOL]
        )
    except Exception as e:
        sys.exit(f"FAILED: {type(e).__name__}: {e}")
    called = [tc["function"]["name"] for tc in message.get("tool_calls", [])]
    print(f"OK: tokens={usage.total_tokens} tool_calls={called} content={message.get('content')!r}")
    return 0 if called == ["ping"] else 1


if __name__ == "__main__":
    sys.exit(main())
