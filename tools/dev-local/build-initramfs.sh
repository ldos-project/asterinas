#!/bin/bash
# SPDX-License-Identifier: MPL-2.0
#
# build-initramfs.sh — Assemble a MINIMAL x86_64 guest initramfs (Tier 1)
# WITHOUT invoking test/Makefile (which harvests x86 binaries from hardcoded
# host paths that don't exist on an arm64 host).
#
# Contents: static x86 busybox (as init + every applet via standalone shell),
# the x86 vDSO, and the repo's arch-agnostic /etc files. Enough to boot the
# kernel to an interactive shell and run ktests.
#
# Output: tools/dev-local/.work/initramfs.cpio.gz
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEV_DIR="${REPO_ROOT}/tools/dev-local"
SYSROOT="${DEV_DIR}/guest-sysroot"
ROOT="${DEV_DIR}/.work/initramfs"           # staging tree
OUT="${DEV_DIR}/.work/initramfs.cpio.gz"

BUSYBOX="${SYSROOT}/busybox"
VDSO="${SYSROOT}/vdso64.so"

[ -x "${BUSYBOX}" ] || { echo "ERROR: ${BUSYBOX} missing — run setup-arm64.sh guest first" >&2; exit 1; }

echo "[initramfs] staging in ${ROOT}"
rm -rf "${ROOT}"
# Note: ext2/exfat/raid1 are mount points that the kernel's lazy_init() mounts
# block devices onto with .unwrap() — they MUST exist or the kernel panics.
mkdir -p "${ROOT}"/{bin,usr/bin,lib/x86_64-linux-gnu,etc,root,tmp,opt,proc,dev,sys,ext2,exfat,raid1}

# busybox as init (matches OSDK.toml: init=/usr/bin/busybox, init_args=[sh,-l]).
# Copy the real binary to BOTH paths — a relative symlink from /usr/bin would
# resolve incorrectly (../bin/busybox -> /usr/bin/busybox, a loop).
cp "${BUSYBOX}" "${ROOT}/bin/busybox"
cp "${BUSYBOX}" "${ROOT}/usr/bin/busybox"
chmod +x "${ROOT}/bin/busybox" "${ROOT}/usr/bin/busybox"

# Applet symlinks. Standalone-shell busybox can resolve applets by itself, but
# we create the common ones explicitly so things work even without it. We
# cannot run `busybox --install` here (the binary is x86, the host is arm64),
# so the list is curated rather than auto-generated.
APPLETS="sh ash ls cat echo mount umount mkdir rm cp mv ln pwd ps dmesg \
         uname env clear sleep grep sed awk head tail wc chmod touch \
         hexdump xxd vi mknod free df du id hostname"
for a in ${APPLETS}; do ln -sf busybox "${ROOT}/bin/${a}"; done

# x86 vDSO where the kernel expects it.
[ -f "${VDSO}" ] && cp "${VDSO}" "${ROOT}/lib/x86_64-linux-gnu/vdso64.so"

# Arch-agnostic /etc from the repo.
cp "${REPO_ROOT}/test/etc/passwd" "${ROOT}/etc/passwd" 2>/dev/null || true
cp "${REPO_ROOT}/test/etc/group"  "${ROOT}/etc/group"  2>/dev/null || true

echo "[initramfs] packing -> ${OUT}"
( cd "${ROOT}" && find . | cpio -o -H newc --quiet | gzip ) > "${OUT}"
echo "[initramfs] done: $(du -h "${OUT}" | cut -f1)"
