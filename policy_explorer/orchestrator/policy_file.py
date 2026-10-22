# SPDX-License-Identifier: MPL-2.0

"""Put the agent's policy into the kernel source file it is compiled from, and take it out again."""

# The policy is one whole source file behind an interface the engineer wrote. Its committed content
# is the fallback: the agent's starting point, what the baseline runs with, and what the file holds
# outside a build. While the agent's code is in the file, a backup of the fallback exists; a run
# killed before putting the fallback back leaves the backup, and the next start restores it.

import json
from contextlib import contextmanager
from pathlib import Path


class PolicyFile:
    """The kernel source file the agent's policy is written into."""

    def __init__(self, repo_root: Path, path: str, backup: Path):
        """Open the policy file at `path` (repo-relative), first undoing what a killed run left in it."""
        self.repo_root = Path(repo_root)
        self.path = path
        self.backup = Path(backup)
        if self.backup.exists():
            left = json.loads(self.backup.read_text())
            (self.repo_root / left["path"]).write_text(left["fallback"])
            self.backup.unlink()
        self.fallback = (self.repo_root / path).read_text()

    @contextmanager
    def containing(self, code: str):
        """Keep the agent's code in the file for the duration of a `with` block."""
        # Restored however the block ends (exception, Ctrl-C, SIGTERM via run_experiment's handler).
        self.backup.parent.mkdir(parents=True, exist_ok=True)
        self.backup.write_text(json.dumps({"path": self.path, "fallback": self.fallback}))
        (self.repo_root / self.path).write_text(code)
        try:
            yield
        finally:
            (self.repo_root / self.path).write_text(self.fallback)
            self.backup.unlink()
