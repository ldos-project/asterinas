#!/usr/bin/env python3

"""Rewrite upstream Docker image references to ldosproject ones during a merge."""

import argparse
import os
import re
import subprocess
import sys

DESCRIPTION = """\
    Helper for resolving upstream-merge conflicts caused by Docker image references.

    Run this while a `git merge upstream/main` is in progress. It:
      * rewrites upstream image refs (asterinas/{kernel-dev,dev,asterinas,osdk-dev,osdk,nix}:<tag>)
        to the matching ldosproject image tagged with our DOCKER_IMAGE_VERSION (from HEAD),
      * normalizes stale ldosproject/{asterinas,osdk,nix} tags to that same version,
      * auto-resolves conflict hunks whose two sides become identical after the rewrite,
      * leaves other hunks in conflict, but with the image noise removed,
      * resolves a conflicted DOCKER_IMAGE_VERSION to our side,
      * stages (`git add`) every file it touched that has no conflicts left.

    Files that still contain conflict markers are left unstaged.
"""

EPILOG = """\
    Usage:
      git merge upstream/main
      python3 tools/resolve_image_conflicts.py --dry-run   # preview
      python3 tools/resolve_image_conflicts.py             # apply
      # resolve the remaining conflicts, git add them, git commit
"""

VERSION_TAG = r"\d+\.\d+\.\d+(?:-ldos)?(?:-\d{8})?"
VAR_TAG = r"\$\{?\w+\}?"

IMAGE_MAP = {
    "kernel-dev": "asterinas",
    "dev": "asterinas",
    "asterinas": "asterinas",
    "osdk-dev": "osdk",
    "osdk": "osdk",
    "nix": "nix",
}

# The lookbehind avoids matching e.g. github.com/asterinas/asterinas or ldosproject/... paths.
UPSTREAM_RE = re.compile(
    r"(?<![\w./-])asterinas/(kernel-dev|dev|asterinas|osdk-dev|osdk|nix)"
    rf":(?:(?P<ver>{VERSION_TAG})|(?P<var>{VAR_TAG}))"
)
LDOS_RE = re.compile(rf"(?<![\w./-])ldosproject/(asterinas|osdk|nix):{VERSION_TAG}")

# Used to find candidate files with `git grep -E`.
GREP_PATTERN = r"(asterinas/(kernel-dev|dev|asterinas|osdk-dev|osdk|nix)|ldosproject/(asterinas|osdk|nix)):"


def run_cmd(cmd, strip=True):
    try:
        result = subprocess.run(
            cmd, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
        )
    except subprocess.CalledProcessError as e:
        sys.exit(f"Error executing {' '.join(cmd)}: {e.stderr.strip()}")
    return result.stdout.strip() if strip else result.stdout


def rewrite(text, version):
    def upstream_sub(m):
        name = f"ldosproject/{IMAGE_MAP[m.group(1)]}"
        tag = version if m.group("ver") else m.group("var")
        return f"{name}:{tag}"

    text = UPSTREAM_RE.sub(upstream_sub, text)
    return LDOS_RE.sub(lambda m: f"ldosproject/{m.group(1)}:{version}", text)


def is_marker(line, marker):
    return line.startswith(marker) and line[7:8] in ("", " ", "\n", "\r")


def process_conflicts(text, version):
    """Rewrite image refs in `text`, resolving conflict hunks that become identical.

    Returns (new_text, resolved_hunks, remaining_hunks).
    """
    lines = text.splitlines(keepends=True)
    out = []
    resolved = remaining = 0
    i = 0
    while i < len(lines):
        line = lines[i]
        if not is_marker(line, "<<<<<<<"):
            out.append(rewrite(line, version))
            i += 1
            continue

        # Parse one hunk: <<<<<<< ours [||||||| base] ======= theirs >>>>>>>
        start = line
        ours, base, theirs = [], None, []
        base_marker = sep = end = None
        section = ours
        j = i + 1
        while j < len(lines):
            cur = lines[j]
            if section is ours and is_marker(cur, "|||||||"):
                base_marker, base = cur, []
                section = base
            elif section is not theirs and is_marker(cur, "======="):
                sep = cur
                section = theirs
            elif section is theirs and is_marker(cur, ">>>>>>>"):
                end = cur
                break
            else:
                section.append(cur)
            j += 1

        if end is None:
            # Malformed hunk; leave the rest untouched apart from rewriting refs.
            out.extend(rewrite(l, version) for l in lines[i:])
            remaining += 1
            break

        new_ours = rewrite("".join(ours), version)
        new_theirs = rewrite("".join(theirs), version)
        if new_ours == new_theirs:
            out.append(new_ours)
            resolved += 1
        else:
            out.append(start)
            out.append(new_ours)
            if base_marker is not None:
                out.append(base_marker)
                out.extend(base)
            out.append(sep)
            out.append(new_theirs)
            out.append(end)
            remaining += 1
        i = j + 1

    return "".join(out), resolved, remaining


def conflicted_files():
    out = run_cmd(["git", "diff", "--name-only", "--diff-filter=U"])
    return {f for f in out.splitlines() if f}


def candidate_files(conflicted):
    files = set(conflicted)
    grep = subprocess.run(
        ["git", "grep", "-lIE", GREP_PATTERN], stdout=subprocess.PIPE, text=True
    )
    if grep.returncode not in (0, 1):
        sys.exit("git grep failed")
    files.update(f for f in grep.stdout.splitlines() if f)
    return sorted(files)


def main():
    parser = argparse.ArgumentParser(
        prog="resolve_image_conflicts.py",
        description=DESCRIPTION,
        epilog=EPILOG,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "-n", "--dry-run", action="store_true", help="report changes without writing files"
    )
    parser.add_argument(
        "--no-stage", action="store_true", help="do not `git add` conflict-free files"
    )
    args = parser.parse_args()

    os.chdir(run_cmd(["git", "rev-parse", "--show-toplevel"]))

    if subprocess.run(["git", "rev-parse", "-q", "--verify", "MERGE_HEAD"],
                      stdout=subprocess.DEVNULL).returncode != 0:
        sys.exit("No merge in progress (MERGE_HEAD not found). Run `git merge upstream/main` first.")

    ours_version_raw = run_cmd(["git", "show", "HEAD:DOCKER_IMAGE_VERSION"], strip=False)
    version = ours_version_raw.strip()
    if not version:
        sys.exit("HEAD:DOCKER_IMAGE_VERSION is empty")
    print(f"ldos image version (from HEAD): {version}\n")

    conflicted = conflicted_files()
    changed, still_conflicted, to_stage = [], [], []
    for path in candidate_files(conflicted):
        if not os.path.isfile(path):
            continue
        try:
            with open(path, "r", encoding="utf-8", newline="") as f:
                text = f.read()
        except UnicodeDecodeError:
            continue

        if path == "DOCKER_IMAGE_VERSION":
            new_text = ours_version_raw if "<<<<<<<" in text else text
            resolved, remaining = (1 if new_text != text else 0), 0
        else:
            new_text, resolved, remaining = process_conflicts(text, version)

        if remaining:
            still_conflicted.append((path, remaining))
        elif new_text != text or path in conflicted:
            to_stage.append(path)
        if new_text != text:
            changed.append((path, resolved))
            if not args.dry_run:
                with open(path, "w", encoding="utf-8", newline="") as f:
                    f.write(new_text)

    verb = "Would change" if args.dry_run else "Changed"
    print(f"{verb} {len(changed)} file(s):")
    for path, resolved in changed:
        note = f"  ({resolved} hunk(s) resolved)" if resolved else ""
        print(f"  {path}{note}")

    print(f"\nFiles with conflicts remaining ({len(still_conflicted)}):")
    for path, remaining in still_conflicted:
        print(f"  {path}  ({remaining} hunk(s))")

    stage = not args.dry_run and not args.no_stage
    if stage and to_stage:
        run_cmd(["git", "add", "--"] + to_stage)
    verb = "Staged" if stage else "Would stage"
    print(f"\n{verb} {len(to_stage)} conflict-free file(s):")
    for path in to_stage:
        print(f"  {path}")


if __name__ == "__main__":
    main()
