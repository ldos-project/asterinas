# SPDX-License-Identifier: MPL-2.0

"""Run one turn's conversation with the agent."""

import json
from typing import Callable

from orchestrator.llm_client import LLMClient, Usage
from orchestrator.tools import SubmittedPolicy, ToolError, Toolbox


def _arguments(tool_call: dict) -> dict:
    """A tool call's arguments, which the model sends as a JSON string."""
    # A long `code` string can come back as invalid JSON (often unterminated); the agent is told so
    # and can retry.
    try:
        return json.loads(tool_call["function"]["arguments"])
    except json.JSONDecodeError as error:
        raise ToolError(f"the arguments are not valid JSON ({error})") from None


def run_turn(
    client: LLMClient,
    toolbox: Toolbox,
    system_prompt: str,
    *,
    max_tokens_per_turn: int = 400_000,
    messages_out: list | None = None,
    user_prompt: str | None = None,
    on_usage: Callable[[list[Usage]], None] | None = None,
) -> tuple[SubmittedPolicy, list[Usage]]:
    """Talk to the agent until it submits a policy; return the policy and the token usage."""
    # No cap on round trips (a tool call is cheap); only a valid submit_policy ends the turn.
    # `max_tokens_per_turn` caps cost instead: every call resends the whole conversation (a real turn
    # once spent 4.5M tokens). Exceeding it raises RuntimeError with `.usages` and `.messages`
    # attached, so the caller can record the spend.
    # `messages_out` receives the whole conversation however the turn ends; `user_prompt` is the
    # opening user message (the search's task). `on_usage` receives the usage so far after every
    # call, so the caller can persist it before a crash loses it.
    messages = [{"role": "system", "content": system_prompt}]
    if user_prompt is not None:
        messages.append({"role": "user", "content": user_prompt})
    usages: list[Usage] = []
    tool_schemas = toolbox.schemas()

    while True:
        message, usage = client.chat(messages=messages, tools=tool_schemas)
        usages.append(usage)
        if on_usage is not None:
            on_usage(usages)
        messages.append(message)
        tool_calls = message.get("tool_calls", [])

        if not tool_calls:
            # The model replied with plain text and no tool call; nudge it to act.
            messages.append({"role": "user", "content": "Please call a tool, ending with submit_policy."})
            continue

        # A valid submit_policy ends the turn at once, even if bundled with other calls or over
        # budget. An invalid one is answered with its error below, so the agent can retry.
        submit_errors: dict[str, str] = {}
        for tool_call in tool_calls:
            if tool_call["function"]["name"] == "submit_policy":
                try:
                    submitted = toolbox.validate_submit(_arguments(tool_call))
                    if messages_out is not None:
                        messages_out.extend(messages)
                    return submitted, usages
                except ToolError as error:
                    submit_errors[tool_call["id"]] = str(error)

        # Budget check only when another round trip is needed.
        if sum(u.total_tokens for u in usages) >= max_tokens_per_turn:
            error = RuntimeError(f"turn exceeded its {max_tokens_per_turn}-token budget without calling submit_policy")
            error.usages = usages
            error.messages = messages
            if messages_out is not None:
                messages_out.extend(messages)
            raise error

        # Answer every other tool call.
        for tool_call in tool_calls:
            if tool_call["id"] in submit_errors:
                content = submit_errors[tool_call["id"]]
            else:
                try:
                    content = json.dumps(toolbox.dispatch(tool_call["function"]["name"], _arguments(tool_call)))
                except ToolError as error:
                    content = str(error)
            messages.append({"role": "tool", "tool_call_id": tool_call["id"], "content": content})
