#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

# Nightly benchmark run: update main, run the suite, publish the results.
#
# Intended to be driven by cron, which starts in $HOME with a minimal PATH, so
# this resolves its own directory and extends PATH:
#
#   0 5 * * * flock -n /tmp/nightly_benchmarks.lock $HOME/asterinas/tools/benchmarks/nightly_benchmarks.sh >> $HOME/bench-nightly.log 2>&1

set -e

# setup_worker.sh installs act to /usr/local/bin; docker is in /usr/bin, which cron already has.
export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"

cd "$(dirname "$(readlink -f "$0")")/../.."

echo "=== $(date -Is) nightly benchmark run starting in $PWD"

BRANCH="${BENCH_BRANCH:-main}"
git fetch --quiet origin "$BRANCH"          # remote, then ref — no prefix
git checkout --quiet "$BRANCH"              # local branch name — no prefix
git merge --ff-only --quiet "origin/$BRANCH"  # remote-tracking ref — prefix
echo "=== $BRANCH at $(git rev-parse --short HEAD)"

tools/benchmarks/run_benchmarks.sh || echo "=== benchmarks reported failures; publishing what completed" >&2

tools/benchmarks/upload_benchmarks.sh

echo "=== $(date -Is) nightly benchmark run finished"
