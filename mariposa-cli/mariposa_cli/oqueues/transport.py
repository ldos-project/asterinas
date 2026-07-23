"""Command execution on the guest.

Two backends share one interface:

* ``ssh``   — run a shell command on the Mariposa guest over SSH.
* ``local`` — run the same command on the host via ``bash -lc`` (for tests).

Both expose ``run`` (capture output) and ``popen`` (stream stdout as a pipe).
The guest is kept passive: every command is a stock tool (``tree``, ``find``,
``cat``), so no OQ-specific code ever runs inside Mariposa.
"""

import subprocess
from collections.abc import Sequence

from .config import Config


class Transport:
    def __init__(self, cfg: Config):
        self._cfg = cfg

    def _argv(self, remote_command: str) -> Sequence[str]:
        if self._cfg.transport == "local":
            return ["bash", "-lc", remote_command]

        argv = ["ssh", "-p", str(self._cfg.port)]
        if self._cfg.key:
            argv += ["-i", self._cfg.key]
        argv += [
            "-o", "BatchMode=yes",
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "LogLevel=ERROR",
            f"{self._cfg.user}@{self._cfg.host}",
            remote_command,
        ]
        return argv

    def run(self, remote_command: str, timeout: float | None = None) -> str:
        """Runs ``remote_command`` and returns its stdout as text.

        ``timeout`` defaults to the config's ``command_timeout`` when omitted.
        Raises ``RuntimeError`` on a non-zero exit so callers can surface the
        guest's stderr to the agent.
        """
        proc = subprocess.run(
            self._argv(remote_command),
            capture_output=True,
            text=True,
            timeout=self._cfg.command_timeout if timeout is None else timeout,
        )
        if proc.returncode != 0:
            raise RuntimeError(
                f"command failed ({proc.returncode}): {remote_command}\n"
                f"{proc.stderr.strip()}"
            )
        return proc.stdout

    def popen(self, remote_command: str) -> subprocess.Popen:
        """Starts ``remote_command`` and returns the process with a binary
        stdout pipe for streaming. The caller owns termination. This is used
        for tasks like streaming OQueues data"""
        return subprocess.Popen(
            self._argv(remote_command),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            bufsize=-1,
        )
