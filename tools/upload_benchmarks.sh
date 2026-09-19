#!/bin/bash
#
# Upload the most recent benchmark run to the dashboard.
#
# Archives the last run-<datetime>/ directory left by tools/run_benchmarks.sh,
# pushes the per-benchmark results through the workflow's Results job, uploads
# the archive as a release asset, then removes the raw directory.
#
# Reads BENCHMARK_SECRET from .secrets, which act loads automatically.
#
# Usage:
#   tools/upload_benchmarks.sh

set -e

if ! grep -qs '^BENCHMARK_SECRET=.' .secrets; then
    echo "Error: BENCHMARK_SECRET is not set in .secrets. Create it with:" >&2
    echo "  echo 'BENCHMARK_SECRET=<token>' > .secrets && chmod 600 .secrets" >&2
    exit 1
fi

shopt -s nullglob
RUN_DIRS=(run-*/)
if (( ${#RUN_DIRS[@]} == 0 )); then
    echo "Error: no run-*/ directory to upload" >&2
    exit 1
fi
RUN_DIR="${RUN_DIRS[-1]%/}"
echo "Uploading $RUN_DIR"

tar -czf "${RUN_DIR}.tar.gz" "$RUN_DIR"

# Clear state left in the workspace by a previous upload: stale configs would be
# re-uploaded alongside the current ones, and the gh-pages clone fails if it exists.
rm -rf results configs benchmark-data-repository
mkdir results
cp "$RUN_DIR"/result_*.json results/

act workflow_dispatch \
    -W .github/workflows/benchmark_x86.yml \
    --bind \
    --pull=false \
    --matrix benchmarks:sysbench/cpu_lat \
    --env SKIP_BENCHMARKS=1 \
    --env UPLOAD_RESULTS=1 \
    --env BENCHMARK_ARCHIVE="${RUN_DIR}.tar.gz"

rm -rf "$RUN_DIR" results configs benchmark-data-repository
echo "Uploaded; archive kept at ${RUN_DIR}.tar.gz"
