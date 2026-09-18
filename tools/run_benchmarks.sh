#!/bin/bash
#
# Run the x86-64 benchmark suite locally with act
# Provenance and every artifact the run produces are captured into a
# run-<datetime>/ directory by howdone.
#
# Usage:
#   tools/run_benchmarks.sh                       # the whole matrix
#   tools/run_benchmarks.sh sysbench/cpu_lat      # only the named benchmarks
#   tools/run_benchmarks.sh nginx/http_file4KB_bw lmbench/pipe_lat
#
# Requires a working docker, and a host with /dev/kvm and /dev/vhost-net.

set -e

# act runs with --pull=false, so make sure the image is present first.
IMAGE="ldosproject/asterinas:$(cat DOCKER_IMAGE_VERSION)"
docker image inspect "$IMAGE" >/dev/null 2>&1 || docker pull "$IMAGE"

MATRIX_ARGS=()
for benchmark in "$@"; do
    MATRIX_ARGS+=(--matrix "benchmarks:${benchmark}")
done

# Run the suite using the benchmark howdone config to capture provenance and artifacts
exec tools/howdone/howdone -c tools/howdone/benchmark_howdone.yaml \
    act workflow_dispatch \
    -W .github/workflows/benchmark_x86.yml \
    -j Benchmarks \
    --bind \
    --pull=false \
    --action-offline-mode \
    --log-prefix-job-id \
    --env SAVE_LOGS=1 \
    --env GIT_CONFIG_COUNT=1 \
    --env GIT_CONFIG_KEY_0=safe.directory \
    --env GIT_CONFIG_VALUE_0='*' \
    "${MATRIX_ARGS[@]}"
