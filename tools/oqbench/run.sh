#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Host-side driver for the OQFS round-trip microbenchmark (kernel/comps/mariposa_benchmark). Like
# the other in-kernel microbenchmarks the run is configured entirely from the kernel command line, so
# this script only sets those parameters, boots, and decodes the captured samples off the data
# capture image afterwards.

set -u -o pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
QEMU_LOG="${ROOT}/qemu.log"
CAPTURE_IMAGE="${ROOT}/test/initramfs/build/capture.img"
DECODER="${ROOT}/kernel/comps/mariposa_data_capture/python/decode_mariposa_data.py"
# `decode_mariposa_data.py` names its output after the capture path the kernel registered.
DECODED_NAME="oqbench_samples.jsonl"

OUTPUT="oqbench-samples.jsonl"
VCPUS=1
KVM=1
# Every other option sets the `oqbench.<name>` kernel parameter of the same name; the kernel owns
# their defaults and their validation, so this script does not duplicate either.
PARAMS=()

usage() {
    cat <<'EOF'
oqbench/run.sh -- run the OQFS kernel<->user round-trip microbenchmark and collect its samples.

USAGE:
    tools/oqbench/run.sh [OPTIONS]

OPTIONS:
    --iterations <N>        Measured iterations (default: 1000000).
    --peer-compute <N>      TSC cycles the userspace peer spins for per request (default: 0).
    --timeout-ms <N>        Per-reply timeout in milliseconds (default: 10000). A timeout ends the
                            run as failed.
    --request-capacity <N>  Request OQueue capacity (default: 2).
    --reply-capacity <N>    Reply OQueue capacity (default: 2).
    --realtime              Run the kernel thread under real-time scheduling (default: normal).
    --rt-prio <N>           Real-time priority for --realtime, in 1..=99 (default: 50).
    --busy-procs <N>        Competing busy-loop processes as scheduler contention (default: 0).
    --vcpus <N>             Guest vCPU count / SMP (default: 1).
    --kvm <on|off>          Use KVM acceleration (default: on).
    --output <FILE>         Sample file to write (default: oqbench-samples.jsonl in CWD).
    -h, --help              Print this help and exit.

The output is JSON Lines, one object per round trip with the four TSC-cycle fields roundtrip,
kernel_to_user, compute and user_to_kernel. Divide by the TSC frequency (printed in the console
metadata block) for seconds. The userspace peer's scheduling cannot be set from userspace on this
kernel, so there is no flag for it.
EOF
}

die() {
    echo "oqbench/run.sh: error: $*" >&2
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --iterations|--peer-compute|--timeout-ms|--request-capacity|--reply-capacity|--rt-prio|--busy-procs)
            name="${1#--}"
            PARAMS+=("oqbench.${name//-/_}=${2:?$1 requires a value}"); shift 2;;
        --realtime)  PARAMS+=("oqbench.realtime"); shift;;
        --vcpus)     VCPUS="${2:?--vcpus requires a value}"; shift 2;;
        --kvm)       case "${2:?--kvm requires a value}" in
                         on|1|true)   KVM=1;;
                         off|0|false) KVM=0;;
                         *) die "--kvm expects on|off, got '$2'";;
                     esac; shift 2;;
        --output)    OUTPUT="${2:?--output requires a value}"; shift 2;;
        -h|--help)   usage; exit 0;;
        *) die "unknown argument '$1' (try --help)";;
    esac
done

# The capture image is not reset by the guest, so drop it and let the build recreate an empty one;
# otherwise a run that died early would decode into the previous run's samples.
rm -f "$CAPTURE_IMAGE"

# The guest powers itself off once the peer sees the run end, so this blocks until it is over.
make -C "$ROOT" run_kernel \
    KCMDARGS="oqbench.enable ${PARAMS[*]}" \
    SMP="$VCPUS" \
    ENABLE_KVM="$KVM" </dev/null \
    || die "make run_kernel failed; inspect ${QEMU_LOG}"

BLOCK="$(grep -o 'MARIPOSA_BENCH|.*' "$QEMU_LOG")"
if grep -q '^MARIPOSA_BENCH|error' <<<"$BLOCK"; then
    grep '^MARIPOSA_BENCH|error' <<<"$BLOCK" >&2
    die "the benchmark reported a fatal run error (see above)"
fi
grep -q '^MARIPOSA_BENCH|end oqueue_roundtrip' <<<"$BLOCK" \
    || die "the run did not complete; inspect ${QEMU_LOG}"

# Echo the metadata so the run is self-describing.
echo "oqbench/run.sh: run metadata:" >&2
sed 's/^/  /' <<<"$BLOCK" >&2

DECODE_DIR="$(mktemp -d)"
trap 'rm -rf "$DECODE_DIR"' EXIT

python3 "$DECODER" "$CAPTURE_IMAGE" --output-dir "$DECODE_DIR" >&2 \
    || die "could not decode ${CAPTURE_IMAGE} (the decoder needs cbor2; see ${DECODER%/*}/requirements.txt)"

cp "${DECODE_DIR}/${DECODED_NAME}" "$OUTPUT" || die "failed to write ${OUTPUT}"

echo "oqbench/run.sh: OK -- wrote $(wc -l <"$OUTPUT") samples to ${OUTPUT}" >&2
