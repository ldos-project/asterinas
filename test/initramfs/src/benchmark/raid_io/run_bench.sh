#!/bin/sh

# SPDX-License-Identifier: MPL-2.0
#
# One replay of the RAID I/O benchmark, inside the guest. The per-workload `<workload>/run.sh` calls it
# through `bench_runner.sh`, on Mariposa and on Linux alike (`SYSTEM` says which).
#
# The host side (`bench_linux_and_aster.sh`) puts a fresh trace over 95% of RAID_IO_RANGE GiB (default 16)
# on the share and, before each boot, recreates every member as a zeroed RAID_IO_RANGE GiB first partition
# of its disk. To run both systems on the machine's SSDs, which wipes those disks entirely:
#   RAID_DEVICES=/dev/nvme0n1p1,/dev/nvme1n1p1,/dev/nvme2n2p1 \
#       test/initramfs/src/benchmark/bench_linux_and_aster.sh raid_io/<workload> x86-64
# The results land in RAID_IO_SHARE/raid_io/<workload>/{asterinas,linux}. Without RAID_DEVICES both
# systems use image files, which runs anywhere but measures nothing.
#
# Usage: run_bench.sh --workload W [--threads N] [--target /dev/X] [--out-dir D]
#   --target defaults to the array of the running system; --out-dir, relative to the share, defaults
#   to raid_io/W/SYSTEM.

set -e

REPLAYER=/benchmark/bin/io_replayer
SHARE=/host
STAGING=/tmp/raid_io  # put dataset in RAM
MARKER="RAID_IO_BENCH|"
SYSTEM=${SYSTEM:-asterinas}
THREADS=8

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
    *) fail "unknown argument '$1'" ;;
    esac
    shift 2
done

[ -n "$WORKLOAD" ] || fail "missing --workload"
OUT_DIR=${OUT_DIR:-raid_io/${WORKLOAD}/${SYSTEM}}

mountpoint -q "$SHARE" || mount -t virtiofs aster-virtiofs "$SHARE" || fail "could not mount the share"

# Setting the device for benchmark. Assemble the RAID array for linux
if [ "$SYSTEM" = linux ]; then
    # Assemble the three members.
    MEMBERS=
    for SERIAL in raid0 raid1 raid2; do
        # Getting the correct block device by finding and matching their serial
        for F in /sys/block/vd*/serial; do
            if [ "$(cat "$F")" = "$SERIAL" ]; then MEMBERS="$MEMBERS /dev/$(basename "$(dirname "$F")")"; fi
        done
    done
    MDADM_NO_UDEV=1 mdadm --create /dev/md0 --run --assume-clean --metadata=1.0 --level=1 --raid-devices=3 $MEMBERS ||
        fail "mdadm could not create the RAID-1 array from '${MEMBERS}'"
    TARGET=${TARGET:-/dev/md0}
else
    TARGET=${TARGET:-/dev/raid}
fi

# Refuse to run unless the target is a block device.
[ -b "$TARGET" ] || fail "the target '${TARGET}' is not a block device"

# Start from empty staging and result directories, and copy the trace from the share into RAM.
RESULT_DIR="${SHARE}/${OUT_DIR}"
TRACE="${STAGING}/${WORKLOAD}.trace"
rm -rf "$STAGING" "$RESULT_DIR"
mkdir -p "$STAGING" "$RESULT_DIR"
# Move the trace from the shared VirtIOFS to /tmp
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
# Copy the results to the share for the host to collect, and flush them. A failed copy is only tolerated
# when the replayer already failed (its output may be missing), since that failure is reported below.
cp "${STAGING}/latency.csv" "${STAGING}/replayer.log" "${STAGING}/guest_meta.json" "$RESULT_DIR/" ||
    [ "$RC" -ne 0 ] || fail "could not copy the results to '${RESULT_DIR}'"
sync

# Report failure if the replayer failed, otherwise mark the run as finished.
[ "$RC" -eq 0 ] || fail "the replayer exited with status ${RC}"
mark "end lines=$(wc -l <"${STAGING}/latency.csv") replay_seconds=${REPLAY_SECONDS}"
