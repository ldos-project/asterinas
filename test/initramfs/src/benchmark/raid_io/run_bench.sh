#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

# One replay of the RAID I/O benchmark, inside the guest. To drive it in Mariposa, use init; to drive it in 
# Debian, use SSH, with --replayer, --share and --staging pointed at its own paths.
#
# Usage: run_bench.sh --workload W --threads N --target /dev/X --out-dir D
#                     [--replayer PATH] [--share DIR] [--staging DIR]

set -e

REPLAYER=/benchmark/bin/io_replayer
SHARE=/host
STAGING=/tmp/raid_io  # put dataset in RAM
MARKER="RAID_IO_BENCH|"

# `mark` prints a line the host driver watches the console for; `fail` prints an error marker and stops.
mark() { echo "${MARKER}$*"; }
fail() {
    mark "error $*"
    exit 1
}

# Parse `--name value` pairs: `$1` is the option name and `$2` its value, so each turn takes the value
# from `$2` and then `shift 2` drops both.
while [ "$#" -gt 0 ]; do
    case "$1" in
    --workload) WORKLOAD=$2 ;;
    --threads) THREADS=$2 ;;
    --target) TARGET=$2 ;;
    --out-dir) OUT_DIR=$2 ;;
    --replayer) REPLAYER=$2 ;;
    --share) SHARE=$2 ;;
    --staging) STAGING=$2 ;;
    *) fail "unknown argument '$1'" ;;
    esac
    shift 2
done

# Refuse to run unless the target is a block device.
[ -b "$TARGET" ] || fail "the target '${TARGET}' is not a block device"

# Start from empty staging and result directories, and copy the trace from the share into RAM.
RESULT_DIR="${SHARE}/${OUT_DIR}"
TRACE="${STAGING}/${WORKLOAD}.trace"
rm -rf "$STAGING" "$RESULT_DIR"
mkdir -p "$STAGING" "$RESULT_DIR"
cp "${SHARE}/traces/${WORKLOAD}.trace" "$TRACE" || fail "no ${WORKLOAD}.trace on the share"

# Count the trace's requests and the CPUs, and tell the host driver the replay is about to start.
REQUESTS=$(wc -l <"$TRACE")
CPUS=$(nproc 2>/dev/null || grep -c '^processor' /proc/cpuinfo)
mark "begin workload=${WORKLOAD} requests=${REQUESTS} threads=${THREADS} cpus=${CPUS} target=${TARGET}"

# A heartbeat, so a slow run can be told from a hung one; `stat` reads an inode in RAM.
(while sleep 10; do mark "progress latency_csv_bytes=$(stat -c %s "${STAGING}/latency.csv" 2>/dev/null || echo 0)"; done) &
HEARTBEAT=$!

# Run the replayer on the target and time it; the replayer appends `_baseline.data` to the output name
# it is given, so rename its output to `latency.csv` afterwards.
STARTED=$(date +%s)
RC=0
"$REPLAYER" baseline "${STAGING}/latency" 1 "$THREADS" "$TARGET" "$TRACE" >"${STAGING}/replayer.log" 2>&1 || RC=$?
REPLAY_SECONDS=$(($(date +%s) - STARTED))
kill "$HEARTBEAT" 2>/dev/null || true
cat "${STAGING}/replayer.log"
mv "${STAGING}/latency_baseline.data" "${STAGING}/latency.csv" 2>/dev/null || true

# Record the run's parameters and the kernel command line next to the results.
cat >"${STAGING}/guest_meta.json" <<EOF
{"workload": "${WORKLOAD}", "requests": ${REQUESTS}, "threads": ${THREADS}, "target": "${TARGET}", "cpus": ${CPUS},
 "replay_seconds": ${REPLAY_SECONDS}, "replayer_exit": ${RC},
 "cmdline": "$(sed 's/\\/\\\\/g; s/"/\\"/g' /proc/cmdline)"}
EOF
# Copy the results to the share for the host to collect, and flush them.
cp "${STAGING}/latency.csv" "${STAGING}/replayer.log" "${STAGING}/guest_meta.json" "$RESULT_DIR/" || true
sync

# Report failure if the replayer failed, otherwise mark the run as finished.
[ "$RC" -eq 0 ] || fail "the replayer exited with status ${RC}"
mark "end lines=$(wc -l <"${STAGING}/latency.csv") replay_seconds=${REPLAY_SECONDS}"
