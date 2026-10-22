# SPDX-License-Identifier: MPL-2.0

"""Skills: self-contained guides a search lets the agent load with `load_skill`."""

# A skill is `skills/<name>.md`: front matter with a one-line `description`, then the guide. The
# system prompt lists only the names and descriptions of the skills the search's config allows.

from dataclasses import dataclass
from pathlib import Path

import frontmatter


@dataclass(frozen=True)
class Skill:
    """One skill: what it is for, and its text."""

    name: str
    description: str
    text: str


def load_skill(directory: Path, name: str) -> Skill:
    """Read the skill `name` from `directory`."""
    path = Path(directory) / f"{name}.md"
    if not path.is_file():
        raise ValueError(f"no skill {name!r} (expected {path})")
    post = frontmatter.load(path)
    if not post.get("description"):
        raise ValueError(f"skill {path} has no `description` in its front matter")
    return Skill(name, post["description"], post.content)


def catalog(skills: list[Skill]) -> str:
    """The system prompt's list of the skills the agent may load."""
    entries = "\n".join(f"- `{skill.name}`: {skill.description}" for skill in skills)
    return f"## Skills\n\n`load_skill` returns the full text of any of these guides:\n\n{entries}"
