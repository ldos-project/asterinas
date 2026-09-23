#!/bin/bash
#
# Nightly benchmark run: update main, run the suite, publish the results.
#
# Intended to be driven by cron, which starts in $HOME with a minimal PATH, so
# this resolves its own directory and extends PATH:
#
#   0 9 * * * flock -n /tmp/nightly_benchmarks.lock /home/gvipat/asterinas/tools/nightly_benchmarks.sh >> /home/gvipat/bench-nightly.log 2>&1

set -e

# act installs to ~/.local/bin; docker is in /usr/bin, which cron already has.
export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"

cd "$(dirname "$(readlink -f "$0")")/.."

echo "=== $(date -Is) nightly benchmark run starting in $PWD"

BENCH_BRANCH="gvipat/nightly-benchmark"
BRANCH="${BENCH_BRANCH:-main}"
git fetch --quiet origin "$BRANCH"          # remote, then ref — no prefix
git checkout --quiet "$BRANCH"              # local branch name — no prefix
git merge --ff-only --quiet "origin/$BRANCH"  # remote-tracking ref — prefix
echo "=== $BRANCH at $(git rev-parse --short HEAD)"

tools/run_benchmarks.sh || echo "=== benchmarks reported failures; publishing what completed" >&2

tools/upload_benchmarks.sh

echo "=== $(date -Is) nightly benchmark run finished"
