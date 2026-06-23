#!/bin/bash
# SPDX-License-Identifier: MPL-2.0
#
# run-local.sh — Build (cross-compile) and run the Asterinas x86_64 kernel
# natively on an arm64 host, with no Docker and no edits to tracked files.
#
# Boot path: BOOT_METHOD=qemu-direct + multiboot protocol => no GRUB, no OVMF.
# Acceleration: none possible (x86 guest on arm64 host) => QEMU uses TCG.
#
# Usage:
#   tools/dev-local/run-local.sh                 # build + run interactive shell
#   tools/dev-local/run-local.sh --build-only    # cross-compile only
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEV_DIR="${REPO_ROOT}/tools/dev-local"
INITRAMFS="${DEV_DIR}/.work/initramfs.cpio.gz"
BUILD_DIR="${REPO_ROOT}/test/build"

# shellcheck disable=SC1091
[ -f "${HOME}/.cargo/env" ] && source "${HOME}/.cargo/env"

# --- scratch disk images that tools/qemu_args.sh references unconditionally ---
# (raid*.img are auto-created by qemu_args.sh itself; we create the rest.)
ensure_images() {
  mkdir -p "${BUILD_DIR}"
  if [ ! -f "${BUILD_DIR}/ext2.img" ]; then
    echo "[run] creating ext2.img"; truncate -s 2G "${BUILD_DIR}/ext2.img"; mke2fs -q -F "${BUILD_DIR}/ext2.img"
  fi
  if [ ! -f "${BUILD_DIR}/exfat.img" ]; then
    echo "[run] creating exfat.img"; truncate -s 64M "${BUILD_DIR}/exfat.img"; mkfs.exfat "${BUILD_DIR}/exfat.img" >/dev/null
  fi
  for img in capture.img capture_legacy.img; do
    [ -f "${BUILD_DIR}/${img}" ] || { echo "[run] creating ${img}"; truncate -s 4G "${BUILD_DIR}/${img}"; }
  done
}

# --- minimal initramfs ---
ensure_initramfs() {
  [ -f "${INITRAMFS}" ] || "${DEV_DIR}/build-initramfs.sh"
}

# --- workaround: OSDK generates its own throwaway workspace under
# target/osdk/ with a SEPARATE Cargo.lock that ignores the repo's pin. ostd
# declares `unwinding = "0.2.8"` (caret), so a fresh resolution grabs 0.2.9,
# which fails to build against the pinned nightly (catch_unwind intrinsic
# returns i32, not bool). Pin it back to 0.2.8 in the generated lock. Safe to
# run repeatedly; only the generated (untracked) lock is touched.
GEN_CRATE="${REPO_ROOT}/target/osdk/aster-nix-run-base"
pin_unwinding() {
  local lock="${GEN_CRATE}/Cargo.lock"
  [ -f "${lock}" ] || return 0
  # The version line immediately follows the `name = "unwinding"` line. Only
  # touch the lock when it isn't already pinned to 0.2.8 — otherwise the
  # spurious `cargo update` forces a relink on every run.
  local ver
  ver="$(awk '/^name = "unwinding"$/{getline; print; exit}' "${lock}")"
  if [ -n "${ver}" ] && [ "${ver}" != 'version = "0.2.8"' ]; then
    echo "[run] pinning unwinding -> 0.2.8 in generated crate"
    ( cd "${GEN_CRATE}" && cargo update -p unwinding --precise 0.2.8 >/dev/null 2>&1 ) || true
  fi
}

# Verify the kernel image OSDK produced is a real ELF, not a truncated stub.
# Rust *incremental* compilation has been observed to silently emit a ~20-byte
# binary when relinking this no_std kernel after a dependency change; booting
# that yields QEMU's "linux kernel too old to load a ram disk". We disable
# incremental (below) to prevent it, and assert here as a backstop.
assert_kernel_ok() {
  local elf
  elf="$(find "${REPO_ROOT}/target/osdk" -name '*.qemu_elf' -printf '%s %p\n' 2>/dev/null | sort -rn | head -1 | cut -d' ' -f2-)"
  local sz magic
  sz="$(stat -c%s "${elf}" 2>/dev/null || echo 0)"
  magic="$(head -c4 "${elf}" 2>/dev/null | od -An -tx1 | tr -d ' \n')"
  if [ -z "${elf}" ] || [ "${sz}" -lt 1000000 ] || [ "${magic}" != "7f454c46" ]; then
    echo "ERROR: kernel image looks corrupt (${elf:-none}, ${sz} bytes)." >&2
    echo "       Forcing a clean relink. Re-run this script." >&2
    rm -rf "${REPO_ROOT}/target/osdk"
    # Drop the (any-profile) bin + its fingerprints so cargo truly relinks.
    find "${REPO_ROOT}/target/x86_64-unknown-none" -maxdepth 2 -name 'aster-nix-osdk-bin' -delete 2>/dev/null || true
    find "${REPO_ROOT}/target/x86_64-unknown-none" -type d -name 'aster-nix-osdk-bin-*' -path '*/.fingerprint/*' -exec rm -rf {} + 2>/dev/null || true
    exit 1
  fi
}

# --- optimization profile ---------------------------------------------------
# Debug (default) is easy to build but SLOW under TCG emulation. An optimized
# kernel emulates much faster because there are far fewer instructions for QEMU
# to translate. Mirrors `make run RELEASE=1` / `RELEASE_LTO=1`.
#   RELEASE=1      -> --release        (recommended for everyday speed)
#   RELEASE_LTO=1  -> --profile release-lto (fastest to run, slowest to build)
PROFILE_ARGS=()
if [ "${RELEASE_LTO:-0}" = "1" ]; then
  PROFILE_ARGS=(--profile release-lto)
  export OSTD_TASK_STACK_SIZE_IN_PAGES="${OSTD_TASK_STACK_SIZE_IN_PAGES:-8}"
elif [ "${RELEASE:-0}" = "1" ]; then
  PROFILE_ARGS=(--release)
  export OSTD_TASK_STACK_SIZE_IN_PAGES="${OSTD_TASK_STACK_SIZE_IN_PAGES:-8}"
fi

# --- the osdk invocation (mirrors `make run BOOT_PROTOCOL=multiboot ENABLE_KVM=0`) ---
OSDK_ARGS=(
  --target-arch=x86_64
  --boot-method=qemu-direct
  --grub-boot-protocol=multiboot
  --kcmd-args="ostd.log_level=${LOG_LEVEL:-error}"
  --initramfs="${INITRAMFS}"
  "${PROFILE_ARGS[@]}"
)

ensure_initramfs
ensure_images

# tools/qemu_args.sh sets up host port-forwards; the default SSH_PORT=22 is a
# privileged port already taken by the host/VM sshd, which makes QEMU abort
# ("Could not set up host forwarding rule 'tcp::22-:22'"). Remap the guest-22
# forward to an unprivileged host port. Override any of these via the env.
export SSH_PORT="${SSH_PORT:-10022}"

# Common env for both build and run.
#  - OVMF=off            => tools/qemu_args.sh skips UEFI firmware (qemu-direct)
#  - CARGO_INCREMENTAL=0 => avoid incremental-compilation link corruption that
#                           silently produces a truncated kernel image
COMMON_ENV=(OVMF=off BOOT_METHOD=qemu-direct ENABLE_KVM=0 CARGO_INCREMENTAL=0)

cd "${REPO_ROOT}/kernel"

# Build with the unwinding workaround. On a CLEAN tree the generated crate
# doesn't exist yet, so the first build generates it (and may pull 0.2.9 and
# fail); we then pin and retry once.
build_kernel() {
  pin_unwinding
  env "${COMMON_ENV[@]}" cargo osdk build "${OSDK_ARGS[@]}" && return 0
  echo "[run] build failed; pinning unwinding=0.2.8 and retrying..."
  pin_unwinding
  env "${COMMON_ENV[@]}" cargo osdk build "${OSDK_ARGS[@]}"
}

echo "[run] cross-compiling kernel (x86_64-unknown-none)..."
build_kernel
assert_kernel_ok

[ "${1:-}" = "--build-only" ] && exit 0

# SMP / MEM are consumed by tools/qemu_args.sh from the environment. With more
# than one vCPU, enable multi-threaded TCG so QEMU translates/runs vCPUs across
# host cores (helps throughput for multi-threaded guest workloads, not boot).
RUN_EXTRA_ARGS=()
if [ "${SMP:-1}" -gt 1 ] 2>/dev/null; then
  echo "[run] SMP=${SMP}: enabling multi-threaded TCG"
  RUN_EXTRA_ARGS+=(--qemu-args="-accel tcg,thread=multi")
fi

echo "[run] launching x86_64 kernel under qemu (TCG). guest:22 -> host:${SSH_PORT}. Ctrl-A X to quit."
exec env "${COMMON_ENV[@]}" cargo osdk run "${OSDK_ARGS[@]}" "${RUN_EXTRA_ARGS[@]}"
