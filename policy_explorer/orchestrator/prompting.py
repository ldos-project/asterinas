# SPDX-License-Identifier: MPL-2.0

"""Assemble each turn's system prompt."""

# Context document + a summary of every prior turn, rebuilt from the database (never from a saved
# chat). The search condition's own prompt is the turn's opening user message, not part of this.

from orchestrator.database import Database


def render_context(text: str, fallback: str) -> str:
    """Fill the context document's `{{policy_file}}` placeholder with the policy file's starting code."""
    # Keeps the document's example identical to the file in the tree.
    return text.replace("{{policy_file}}", fallback.rstrip("\n"))


def _summarize_turn(turn) -> str:
    """One past turn's line in the history."""
    if not turn.perf.get("build_ok"):
        return f"Turn {turn.turn_id}: \"{turn.description}\" — build failed. Compiler error:\n{turn.perf.get('error')}"
    return f"Turn {turn.turn_id}: \"{turn.description}\" — {turn.perf['summary']}"


def build_system_prompt(context: str, database: Database, turn_id: int) -> str:
    """Build the system prompt for turn `turn_id`."""
    turns = database.all_turns()
    if not turns:
        history = "No prior turns yet — this is the first."
    else:
        history = "\n".join(_summarize_turn(turn) for turn in turns)

    best = database.current_best()
    if best is None:
        best_line = "No policy has beaten the baseline yet."
    else:
        best_line = f"Current best: turn {best.turn_id}, {best.perf['summary']}"

    return "\n\n".join([context, f"Prior turns:\n{history}", best_line, f"This is turn {turn_id}."])
