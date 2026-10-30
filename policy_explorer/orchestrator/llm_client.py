# SPDX-License-Identifier: MPL-2.0

"""A thin wrapper around an OpenAI-compatible chat-completions client, converting the SDK's
response objects into plain dicts/dataclasses so the rest of the orchestrator never depends on the
SDK's own types, and reading the per-call token usage the API already reports instead of tracking
it separately."""

from dataclasses import dataclass


def _cached_tokens(usage) -> int:
    """The API reports cached prompt tokens as `prompt_tokens_details.cached_tokens`."""
    details = getattr(usage, "prompt_tokens_details", None)
    return getattr(details, "cached_tokens", 0) or 0


def _reasoning_tokens(usage) -> int:
    """Reasoning tokens spent by one call."""
    # Reported as `completion_tokens_details.reasoning_tokens`; absent for non-reasoning models.
    details = getattr(usage, "completion_tokens_details", None)
    return getattr(details, "reasoning_tokens", 0) or 0


@dataclass
class Usage:
    prompt_tokens: int
    completion_tokens: int
    cached_tokens: int
    total_tokens: int
    reasoning_tokens: int = 0


class LLMClient:
    def __init__(self, client, model: str, reasoning_effort: str = "high"):
        self._client = client
        self._model = model
        self._reasoning_effort = reasoning_effort

    def chat(self, messages: list[dict], tools: list[dict]) -> tuple[dict, Usage]:
        response = self._client.chat.completions.create(
            model=self._model,
            messages=messages,
            tools=tools,
            reasoning_effort=self._reasoning_effort,
        )
        # The SDK's own message, as the dict the API takes back in `messages`: tool calls keep their
        # `arguments` as the JSON string the model sent. Other fields (refusal, provider extras) and
        # an absent `tool_calls` are left out.
        message = response.choices[0].message.model_dump(include={"role", "content", "tool_calls"}, exclude_none=True)
        usage = Usage(
            prompt_tokens=response.usage.prompt_tokens,
            completion_tokens=response.usage.completion_tokens,
            cached_tokens=_cached_tokens(response.usage),
            total_tokens=response.usage.total_tokens,
            reasoning_tokens=_reasoning_tokens(response.usage),
        )
        return message, usage
