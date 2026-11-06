# SPDX-License-Identifier: MPL-2.0
# Builds `raid_policy_server`, the userspace RAID-1 read-selection policy server that talks to the
# kernel over OQFS (`/oqueues`), as a normal cargo package.
{ rustPlatform, glibc }:
rustPlatform.buildRustPackage {
  pname = "raid-policy-server";
  version = "0.1.0";
  src = ../src/raid_policy_server;
  cargoLock.lockFile = ../src/raid_policy_server/Cargo.lock;

  # This binary must be copied straight into the initramfs (outside the nix closure), like
  # `oqueue-reader`. A dynamically-linked binary's ELF interpreter and libc would point into the
  # nix store, which is not present in the initramfs, so the guest could never `exec` it. Static
  # linking avoids that entirely. `glibc.static` provides `libc.a`.
  buildInputs = [ glibc glibc.static ];
  # `crt-static` makes rustc link the static glibc above instead of the normal dynamic one.
  RUSTFLAGS = "-C target-feature=+crt-static";

  # No unit tests exist for this crate; also avoids running cross-compiled test binaries on the
  # build host when targeting riscv64/loongarch64.
  doCheck = false;
}
