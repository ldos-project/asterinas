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

git fetch --quiet origin main
git checkout --quiet main
git merge --ff-only --quiet origin/main
echo "=== main at $(git rev-parse --short HEAD)"

tools/run_benchmarks.sh || echo "=== benchmarks reported failures; publishing what completed" >&2

tools/upload_benchmarks.sh

echo "=== $(date -Is) nightly benchmark run finished"
