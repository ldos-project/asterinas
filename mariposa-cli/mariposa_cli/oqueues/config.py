"""Runtime configuration, sourced from environment variables.

The MCP server runs on the *host* (the agent's domain). It reaches the Mariposa
guest over SSH, which QEMU forwards from guest:22 to host ``SSH_PORT``. For
tests, ``OQ_TRANSPORT=local`` runs the same commands directly on the host so a
fake ``/oqueues`` tree can be exercised without a running kernel.
"""

import os
from dataclasses import dataclass


@dataclass(frozen=True)
class Config:
    # "ssh" (reach the guest over SSH) or "local" (run commands on the host, usually for testing).
    transport: str
    host: str
    port: int
    user: str
    key: str | None
    root: str
    metadata_file: str
    command_timeout: float

    @staticmethod
    def from_env() -> "Config":
        return Config(
            transport=os.environ.get("OQ_TRANSPORT", "ssh"),
            host=os.environ.get("OQ_SSH_HOST", "127.0.0.1"),
            port=int(os.environ.get("OQ_SSH_PORT", "61541")),
            user=os.environ.get("OQ_SSH_USER", "root"),
            key=os.environ.get("OQ_SSH_KEY") or None,
            root=os.environ.get("OQ_ROOT", "/oqueues"),
            metadata_file=os.environ.get("OQ_METADATA_FILE", "metadata.yaml"),
            command_timeout=float(os.environ.get("OQ_COMMAND_TIMEOUT", "30.0")),
        )
