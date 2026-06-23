#!/bin/bash
# SPDX-License-Identifier: MPL-2.0
#
# setup-arm64.sh — Provision a NATIVE Asterinas dev environment on an arm64
# Linux host (e.g. an OrbStack Ubuntu VM on an Apple Silicon Mac), WITHOUT
# Docker and WITHOUT editing any tracked repo file.
#
# What this gives you:
#   * Native arm64 Rust toolchain that CROSS-COMPILES the kernel to
#     x86_64-unknown-none (fast — no emulation for the build).
#   * qemu-system-x86_64 to RUN the x86 kernel under TCG software emulation
#     (no KVM/HVF acceleration is possible for a foreign guest arch).
#   * A minimal x86_64 guest userspace (static busybox) so the kernel can
#     boot to a shell. This is "Tier 1": enough for kernel dev + ktests.
#     The full benchmark/app userspace ("Tier 2") is NOT built here.
#
# Why a separate x86 busybox at all: the thing we build IS an x86_64 OS, so
# its userspace (initramfs) must contain x86_64 Linux binaries regardless of
# the host arch. busybox is statically linked, so a minimal boot needs only
# that one binary plus the vDSO — no glibc harvesting required.
#
# This script is idempotent: re-running it skips already-completed steps.
# It echoes every privileged command before running it.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEV_DIR="${REPO_ROOT}/tools/dev-local"
WORK_DIR="${DEV_DIR}/.work"            # scratch (downloads, busybox build tree)
SYSROOT="${DEV_DIR}/guest-sysroot"      # staged x86 guest binaries (Tier 1)
mkdir -p "${WORK_DIR}" "${SYSROOT}"

BUSYBOX_VERSION="1.35.0"
BUSYBOX_URL="https://busybox.net/downloads/busybox-${BUSYBOX_VERSION}.tar.bz2"
VDSO_URL="https://raw.githubusercontent.com/asterinas/linux_vdso/2a6d2db/vdso64.so"
CROSS="x86_64-linux-gnu-"

log()  { echo -e "\033[1;34m[setup]\033[0m $*"; }
run()  { echo -e "\033[1;33m+ $*\033[0m"; "$@"; }

# ---------------------------------------------------------------------------
# Phase 1 — host packages (the only step that needs sudo)
# ---------------------------------------------------------------------------
phase_apt() {
  log "Phase 1: installing host packages via apt (needs sudo)"
  local pkgs=(
    qemu-system-x86          # qemu-system-x86_64: runs the x86 guest under TCG
    ovmf                     # x86 UEFI firmware (only needed for UEFI boot; harmless)
    e2fsprogs                # mke2fs for the ext2 scratch disk
    exfatprogs               # mkfs.exfat for the exfat scratch disk
    cpio gzip bzip2          # initramfs packing + busybox tarball
    gcc-x86-64-linux-gnu     # cross-compiler to build static x86 busybox
    libc6-dev-amd64-cross    # x86_64 libc headers + static libs for the cross build
    build-essential          # host cc/linker for building cargo-osdk
    pkg-config libssl-dev    # cargo-osdk build deps
    curl wget git ca-certificates
  )
  run sudo apt-get update
  run sudo apt-get install -y --no-install-recommends "${pkgs[@]}"
}

# ---------------------------------------------------------------------------
# Phase 2 — Rust toolchain + cargo-osdk (user-local, no sudo)
# ---------------------------------------------------------------------------
phase_rust() {
  log "Phase 2: Rust toolchain + cargo-osdk"
  if ! command -v rustup >/dev/null 2>&1; then
    run bash -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none --profile minimal"
  fi
  # shellcheck disable=SC1091
  source "${HOME}/.cargo/env"
  # rust-toolchain.toml at the repo root pins the nightly + components + the
  # x86_64-unknown-none target. `rustup show` inside the repo installs it.
  run bash -c "cd '${REPO_ROOT}' && rustup show"
  if [ ! -x "${HOME}/.cargo/bin/cargo-osdk" ]; then
    run bash -c "cd '${REPO_ROOT}' && OSDK_LOCAL_DEV=1 cargo install cargo-osdk --path osdk"
  else
    log "cargo-osdk already installed (run 'make install_osdk' to update)"
  fi
}

# ---------------------------------------------------------------------------
# Phase 3 — minimal x86 guest userspace (static busybox + vDSO)
# ---------------------------------------------------------------------------
phase_busybox() {
  log "Phase 3a: cross-build static x86_64 busybox ${BUSYBOX_VERSION}"
  local bb_out="${SYSROOT}/busybox"
  if [ -x "${bb_out}" ]; then
    log "busybox already built at ${bb_out}"
  else
    local tarball="${WORK_DIR}/busybox-${BUSYBOX_VERSION}.tar.bz2"
    local src="${WORK_DIR}/busybox-${BUSYBOX_VERSION}"
    [ -f "${tarball}" ] || run wget -O "${tarball}" "${BUSYBOX_URL}"
    rm -rf "${src}"
    run tar -xf "${tarball}" -C "${WORK_DIR}"
    (
      cd "${src}"
      run make defconfig
      # Match the project's container config: fully static + standalone shell
      # (so the single binary can act as init AND resolve every applet).
      sed -i 's/# CONFIG_STATIC is not set/CONFIG_STATIC=y/'                                 .config
      sed -i 's/# CONFIG_FEATURE_SH_STANDALONE is not set/CONFIG_FEATURE_SH_STANDALONE=y/'   .config
      # TC applet fails to build against modern kernel headers — not needed here.
      sed -i 's/^CONFIG_TC=y/# CONFIG_TC is not set/'                                        .config || true
      sed -i 's/^CONFIG_FEATURE_TC_INGRESS=y/# CONFIG_FEATURE_TC_INGRESS is not set/'        .config || true
      run make CROSS_COMPILE="${CROSS}" -j"$(nproc)" busybox
    )
    run cp "${src}/busybox" "${bb_out}"
    command -v file >/dev/null 2>&1 && file "${bb_out}" || true
  fi

  log "Phase 3b: fetch x86 vDSO"
  local vdso="${SYSROOT}/vdso64.so"
  if [ -f "${vdso}" ]; then
    log "vdso64.so already present"
  else
    run "${REPO_ROOT}/tools/atomic_wget.sh" "${vdso}" "${VDSO_URL}"
  fi
}

main() {
  local what="${1:-all}"
  case "${what}" in
    apt)     phase_apt ;;
    rust)    phase_rust ;;
    guest)   phase_busybox ;;
    all)     phase_apt; phase_rust; phase_busybox ;;
    *) echo "usage: $0 [all|apt|rust|guest]"; exit 1 ;;
  esac
  log "Done (${what}). Next: tools/dev-local/run-local.sh"
}
main "$@"
