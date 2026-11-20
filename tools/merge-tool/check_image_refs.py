#!/usr/bin/env python3

"""Fail if the tree references any upstream asterinas/* Docker image."""

import argparse
import re
import subprocess
import sys

DESCRIPTION = """\
    Guard against pulling upstream Docker images after an upstream merge.

    Scans all tracked text files and fails (exit 1) if it finds:
      * any tagged upstream image reference, e.g. `asterinas/asterinas:0.18.0`,
        `docker.io/asterinas/osdk:${VERSION}`, `asterinas/coco:latest`;
      * any untagged upstream image used where it would be pulled, e.g.
        `FROM asterinas/asterinas`, `image: asterinas/nix`, `docker pull asterinas/osdk`;
      * any ldosproject/{asterinas,osdk,nix} tag other than DOCKER_IMAGE_VERSION.

    Use `ldosproject/*` images tagged with DOCKER_IMAGE_VERSION instead.
"""

# Not preceded by a path/URL character, so github.com/asterinas/asterinas and
# ldosproject/asterinas are not matched.
NOT_PATH = r"(?<![\w./-])"

# Any tagged image under the upstream `asterinas` Docker Hub namespace.
UPSTREAM_TAGGED_RE = re.compile(NOT_PATH + r"(?:docker\.io/)?asterinas/[a-z0-9._-]+:[^\s'\"]+")

# Untagged upstream images in a position where Docker would pull them.
UPSTREAM_PULLED_RE = re.compile(
    r"(?:\bFROM\s+(?:--\S+\s+)*|\bimage:\s*['\"]?|\bcontainer:\s*['\"]?|"
    r"\bdocker\s+(?:pull|run|create)\b[^\n]*?\s)"
    r"(?:docker\.io/)?asterinas/[a-z0-9._-]+(?![\w./:-])"
)

LDOS_RE = re.compile(NOT_PATH + r"ldosproject/(?:asterinas|osdk|nix):([\w.${}-]+)")
# A tag that is a variable (e.g. ${VERSION} or ${{ ... }}) is resolved elsewhere.
VAR_TAG_RE = re.compile(r"^\$")

# Known references that are documentation only and are never pulled by our
# builds or CI. Keep this list short and justify every entry.
ALLOWLIST = {
    # Upstream CoCo how-to; we do not build or publish a coco image.
    "book/src/kernel/vm-based-containers/coco.md",
}


def tracked_files():
    out = subprocess.run(
        ["git", "ls-files", "-z"], check=True, stdout=subprocess.PIPE
    ).stdout
    return [f for f in out.decode().split("\0") if f]


def main():
    parser = argparse.ArgumentParser(
        prog="check_image_refs.py",
        description=DESCRIPTION,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.parse_args()

    toplevel = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"], check=True, stdout=subprocess.PIPE, text=True
    ).stdout.strip()

    with open(f"{toplevel}/DOCKER_IMAGE_VERSION", encoding="utf-8") as f:
        version = f.read().strip()
    print(f"Expected ldosproject image version: {version}")

    errors = []
    for path in tracked_files():
        if path in ALLOWLIST:
            continue
        try:
            with open(f"{toplevel}/{path}", "r", encoding="utf-8") as f:
                lines = f.readlines()
        except (UnicodeDecodeError, IsADirectoryError, FileNotFoundError):
            continue

        for lineno, line in enumerate(lines, 1):
            for m in UPSTREAM_TAGGED_RE.finditer(line):
                errors.append(f"{path}:{lineno}: upstream image `{m.group(0)}`")
            for m in UPSTREAM_PULLED_RE.finditer(line):
                errors.append(f"{path}:{lineno}: upstream image pulled: `{m.group(0).strip()}`")
            for m in LDOS_RE.finditer(line):
                tag = m.group(1)
                if tag != version and not VAR_TAG_RE.match(tag):
                    errors.append(
                        f"{path}:{lineno}: `{m.group(0)}` does not use DOCKER_IMAGE_VERSION ({version})"
                    )

    if errors:
        print(f"\nFound {len(errors)} disallowed Docker image reference(s):")
        for e in errors:
            print(f"  {e}")
        print("\nRun tools/merge-tool/resolve_image_conflicts.py or fix these by hand.")
        return 1

    print("OK: no upstream asterinas/* images and all ldosproject tags match.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
