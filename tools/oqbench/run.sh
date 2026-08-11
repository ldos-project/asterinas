#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Host-side driver for the OQFS round-trip microbenchmark (kernel/comps/oqueue_roundtrip_bench):
# it builds and boots the kernel with the benchmark enabled, waits for the run to complete, fetches
# the results file from the guest over scp (using the initramfs's dropbear/sftp-server), verifies
# the sample count, and then shuts the guest down. Any failed boot, incomplete run, failed fetch, or
# count mismatch is a hard failure with a clear message.

set -u -o pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
QEMU_LOG="${ROOT}/qemu.log"
# The results file the userspace peer writes in the guest (its `--output` default).
GUEST_RESULTS="/tmp/oqbench-samples.csv"

# ssh/scp reach the guest on the forwarded port; the guest regenerates host keys each boot, so host
# key checking is disabled.
SSH_PORT="${SSH_PORT:-$((20000 + RANDOM % 20000))}"
export SSH_PORT
SSH_OPTS=(-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
          -o LogLevel=ERROR -o ConnectTimeout=5 -o BatchMode=yes)

# ---- Defaults -----------------------------------------------------------------------------------

ITERATIONS=1000000
WARMUP=10000
PEER_COMPUTE=0
TIMEOUT_MS=10000
REQUEST_CAPACITY=2
REPLY_CAPACITY=2
SCHED=normal
RT_PRIO=50
BUSY_PROCS=0
VCPUS=1
KVM=1
OUTPUT=""

usage() {
    cat <<'EOF'
oqbench/run.sh -- run the OQFS kernel<->user round-trip microbenchmark and collect its samples.

USAGE:
    tools/oqbench/run.sh [OPTIONS]

OPTIONS:
    --iterations <N>        Measured iterations (default: 1000000). Must be > 0. At 32 bytes/sample
                            the in-guest sample array is 32*N bytes (e.g. 32 MB for a million).
    --warmup <N>            Warmup iterations, excluded from the samples (default: 10000).
    --peer-compute <N>      Userspace peer's synthetic work per request, in iterations (default: 0).
    --timeout-ms <N>        Per-reply timeout in milliseconds (default: 10000). Must be > 0. A
                            timeout is fatal: the kernel prints an error and powers the guest off
                            rather than surviving it.
    --request-capacity <N>  Request OQueue capacity (default: 2). Must be > 0.
    --reply-capacity <N>    Reply OQueue capacity (default: 2). Must be > 0.
    --sched <normal|realtime>  Driver-thread scheduling policy (default: normal).
    --rt-prio <N>           Real-time priority for --sched realtime, in 1..=99 (default: 50).
    --busy-procs <N>        Competing busy-loop processes as scheduler contention (default: 0 = none).
    --vcpus <N>             Guest vCPU count / SMP (default: 1). Must be > 0.
    --kvm <on|off>          Use KVM acceleration (default: on).
    --output <FILE>         CSV sample file to write (default: oqbench-samples.csv in CWD).
    -h, --help              Print this help and exit.

Each CSV line is one sample: four TSC-cycle fields
roundtrip,kernel_to_user,compute,user_to_kernel. Divide by the TSC frequency (printed in the console
metadata block) for seconds. The userspace peer's scheduling cannot be set from userspace on this
kernel, so there is no flag for it (see the READMEs).
EOF
}

die() {
    echo "oqbench/run.sh: error: $*" >&2
    exit 1
}

# ---- Argument parsing ---------------------------------------------------------------------------

require_uint() {
    # $1 = value, $2 = flag name
    [[ "$1" =~ ^[0-9]+$ ]] || die "$2 expects a non-negative integer, got '$1'"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --iterations)        ITERATIONS="${2:?--iterations requires a value}"; shift 2;;
        --warmup)            WARMUP="${2:?--warmup requires a value}"; shift 2;;
        --peer-compute)      PEER_COMPUTE="${2:?--peer-compute requires a value}"; shift 2;;
        --timeout-ms)        TIMEOUT_MS="${2:?--timeout-ms requires a value}"; shift 2;;
        --request-capacity)  REQUEST_CAPACITY="${2:?--request-capacity requires a value}"; shift 2;;
        --reply-capacity)    REPLY_CAPACITY="${2:?--reply-capacity requires a value}"; shift 2;;
        --sched)             SCHED="${2:?--sched requires a value}"; shift 2;;
        --rt-prio)           RT_PRIO="${2:?--rt-prio requires a value}"; shift 2;;
        --busy-procs)        BUSY_PROCS="${2:?--busy-procs requires a value}"; shift 2;;
        --vcpus)             VCPUS="${2:?--vcpus requires a value}"; shift 2;;
        --kvm)               KVM_ARG="${2:?--kvm requires a value}"; shift 2
                             case "$KVM_ARG" in
                                 on|1|true)  KVM=1;;
                                 off|0|false) KVM=0;;
                                 *) die "--kvm expects on|off, got '$KVM_ARG'";;
                             esac;;
        --output)            OUTPUT="${2:?--output requires a value}"; shift 2;;
        -h|--help)           usage; exit 0;;
        *) die "unknown argument '$1' (try --help)";;
    esac
done

# ---- Validation (refuse nonsense rather than defaulting around it) -------------------------------

require_uint "$ITERATIONS" --iterations
require_uint "$WARMUP" --warmup
require_uint "$PEER_COMPUTE" --peer-compute
require_uint "$TIMEOUT_MS" --timeout-ms
require_uint "$REQUEST_CAPACITY" --request-capacity
require_uint "$REPLY_CAPACITY" --reply-capacity
require_uint "$RT_PRIO" --rt-prio
require_uint "$BUSY_PROCS" --busy-procs
require_uint "$VCPUS" --vcpus

[[ "$ITERATIONS" -gt 0 ]] || die "--iterations must be greater than 0"
[[ "$TIMEOUT_MS" -gt 0 ]] || die "--timeout-ms must be greater than 0"
[[ "$REQUEST_CAPACITY" -gt 0 ]] || die "--request-capacity must be greater than 0"
[[ "$REPLY_CAPACITY" -gt 0 ]] || die "--reply-capacity must be greater than 0"
[[ "$VCPUS" -gt 0 ]] || die "--vcpus must be greater than 0"

case "$SCHED" in
    normal) ;;
    realtime) { [[ "$RT_PRIO" -ge 1 && "$RT_PRIO" -le 99 ]]; } || die "--rt-prio must be in 1..=99";;
    *) die "unknown --sched '$SCHED' (known: normal, realtime)";;
esac

[[ -n "$OUTPUT" ]] || OUTPUT="oqbench-samples.csv"
# Fail early on an unwritable output location rather than after a long boot.
: >"$OUTPUT" || die "cannot write output file '$OUTPUT'"

# An SSH key must exist before the build: `test/initramfs/Makefile` bakes `~/.ssh/*.pub` into the
# guest's authorized_keys, and scp logs in with the matching private key.
if ! ls "${HOME}/.ssh/"*.pub >/dev/null 2>&1; then
    echo "oqbench/run.sh: no SSH key found; generating one for the guest fetch" >&2
    ssh-keygen -t ed25519 -N '' -q -f "${HOME}/.ssh/id_ed25519" \
        || die "could not generate an SSH key (needed to fetch results from the guest)"
fi
# The authorized_keys build file is regenerated only when absent, so drop any stale one to force the
# current public keys into the guest.
rm -f "${ROOT}/test/initramfs/build/authorized_keys"

# ---- Build and boot -----------------------------------------------------------------------------

# `oqbench.await_fetch` keeps the guest alive after a successful run so we can scp the results before
# powering it off; on an anomaly the kernel still powers off itself.
KCMDARGS="oqbench.enable oqbench.await_fetch"
KCMDARGS+=" oqbench.iterations=${ITERATIONS}"
KCMDARGS+=" oqbench.warmup=${WARMUP}"
KCMDARGS+=" oqbench.timeout_ms=${TIMEOUT_MS}"
KCMDARGS+=" oqbench.request_capacity=${REQUEST_CAPACITY}"
KCMDARGS+=" oqbench.reply_capacity=${REPLY_CAPACITY}"
KCMDARGS+=" oqbench.peer_compute=${PEER_COMPUTE}"
KCMDARGS+=" oqbench.busy_procs=${BUSY_PROCS}"
if [[ "$SCHED" == "realtime" ]]; then
    KCMDARGS+=" oqbench.realtime oqbench.rt_prio=${RT_PRIO}"
fi

echo "oqbench/run.sh: booting scenario: iterations=${ITERATIONS} warmup=${WARMUP} peer_compute=${PEER_COMPUTE}" \
     "sched=${SCHED}(${RT_PRIO}) busy_procs=${BUSY_PROCS} vcpus=${VCPUS} kvm=${KVM} ssh_port=${SSH_PORT}" >&2

# Remove any stale qemu.log first, so leftovers from a prior run cannot be parsed as this run's.
rm -f "$QEMU_LOG"

# Boot in the background: the guest stays alive until we fetch the file and power it off, so we
# cannot block on `make` the way an anomaly-only run would. `setsid` puts the whole build/boot tree
# (make -> cargo osdk -> qemu) in its own process group so cleanup can tear it all down; stdin is
# kept off the terminal.
setsid make -C "$ROOT" run_kernel \
    KCMDARGS="$KCMDARGS" \
    SMP="$VCPUS" \
    ENABLE_KVM="$KVM" </dev/null &
MAKE_PID=$!

# However we leave, never abandon a live guest: power it off cleanly, then kill the whole process
# group so no orphaned QEMU keeps holding the forwarded ports.
cleanup() {
    if kill -0 "$MAKE_PID" 2>/dev/null; then
        ssh -p "$SSH_PORT" "${SSH_OPTS[@]}" root@localhost 'poweroff -f' >/dev/null 2>&1 || true
        for _ in $(seq 1 10); do kill -0 "$MAKE_PID" 2>/dev/null || break; sleep 1; done
        kill -- -"$MAKE_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# ---- Wait for the run to complete ---------------------------------------------------------------

# Wait for either the success marker (guest now alive and waiting for the fetch) or for `make` to
# exit (an anomaly powered the guest off, or the build/boot failed). A wall-clock cap keeps a hung
# guest from blocking forever.
MAX_WAIT="${OQBENCH_RUN_TIMEOUT:-7200}"
waited=0
ready=0
while true; do
    if [[ -f "$QEMU_LOG" ]] && grep -qF "OQBENCH: run complete" "$QEMU_LOG"; then
        ready=1
        break
    fi
    if ! kill -0 "$MAKE_PID" 2>/dev/null; then
        break
    fi
    if [[ "$waited" -ge "$MAX_WAIT" ]]; then
        die "timed out after ${MAX_WAIT}s waiting for the run to complete; inspect ${QEMU_LOG}"
    fi
    sleep 1
    waited=$((waited + 1))
done

# The log exists only if QEMU booted this run (it was just removed); its absence means the build or
# boot failed before QEMU started.
[[ -f "$QEMU_LOG" ]] || die "make run_kernel failed before QEMU booted; no ${QEMU_LOG} was produced"

# Isolate the last begin..end result block (stripping any leading console noise).
BLOCK="$(awk '
    { if (match($0, /OQBENCH\|/)) line = substr($0, RSTART); else next }
    line ~ /^OQBENCH\|begin/ { buf = line "\n"; capture = 1; next }
    capture { buf = buf line "\n" }
    line ~ /^OQBENCH\|end/ { capture = 0; done = buf }
    END { printf "%s", done }
' "$QEMU_LOG")"

# A run-level error line (e.g. the userspace peer never attached) means the kernel powered off; there
# is nothing to fetch.
if grep -q '^OQBENCH|error' <<<"$BLOCK"; then
    echo "$BLOCK" | grep '^OQBENCH|error' >&2
    die "the benchmark reported a fatal run error (see above)"
fi

if [[ "$ready" -ne 1 ]]; then
    die "run did not complete (no success marker); an anomaly stopped it or the boot failed -- inspect ${QEMU_LOG}"
fi

# Echo the metadata block so the run is self-describing.
echo "oqbench/run.sh: run metadata:" >&2
grep '^OQBENCH|' <<<"$BLOCK" | sed 's/^/  /' >&2

MEASURED="$(sed -n 's/^OQBENCH|counts .*measured=\([0-9]*\).*/\1/p' <<<"$BLOCK")"
[[ -n "$MEASURED" ]] || die "could not read the measured sample count from the result block"

# ---- Fetch the results file over scp ------------------------------------------------------------

FETCHED="$(mktemp)"
trap 'rm -f "$FETCHED"; cleanup' EXIT

# dropbear may still be coming up right after the marker; retry the copy briefly.
fetched=0
for attempt in $(seq 1 30); do
    if scp -P "$SSH_PORT" "${SSH_OPTS[@]}" "root@localhost:${GUEST_RESULTS}" "$FETCHED" >/dev/null 2>&1; then
        fetched=1
        break
    fi
    sleep 1
done
[[ "$fetched" -eq 1 ]] || die "failed to scp ${GUEST_RESULTS} from the guest (port ${SSH_PORT})"

FETCHED_COUNT="$(wc -l <"$FETCHED")"
if [[ "$FETCHED_COUNT" -ne "$MEASURED" ]]; then
    die "fetched ${FETCHED_COUNT} samples but the run reported measured=${MEASURED}; the results file is truncated or inconsistent"
fi

# Only now, with a complete and consistent file, write the output.
cp "$FETCHED" "$OUTPUT" || die "failed to write ${OUTPUT}"

echo "oqbench/run.sh: OK -- fetched ${FETCHED_COUNT} samples to ${OUTPUT}" >&2

# Bring the guest down now that the file is safely on the host (the EXIT trap is a backstop).
cleanup
