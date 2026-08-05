# SPDX-License-Identifier: MPL-2.0
# Builds the RAID-1 userspace read-selection policy workspace (`test/initramfs/src/raid_policies`):
# one standalone binary per policy plus the `raid_policy_supervisor`. Each is copied straight into
# the initramfs (outside the nix closure), so — like `oqueue-reader` and the old
# `raid-policy-server` — they must be statically linked, or the guest could never `exec` them (their
# ELF interpreter and libc would point into the absent nix store).
#
# `Cargo.lock` pins the workspace's one dependency (`minicbor`); `buildRustPackage`'s
# `cargoLock.lockFile` vendors it via fixed-output derivations keyed by the lockfile's own checksums,
# which is how a cargo build gets its dependencies inside the network-less nix build sandbox.
{ rustPlatform, glibc }:
rustPlatform.buildRustPackage {
  pname = "raid-policies";
  version = "0.1.0";
  src = ../src/raid_policies;
  cargoLock.lockFile = ../src/raid_policies/Cargo.lock;

  # `glibc.static` provides `libc.a`; `crt-static` makes rustc link it statically. This builds every
  # workspace binary (all policies + the supervisor) statically in one go.
  buildInputs = [ glibc glibc.static ];
  RUSTFLAGS = "-C target-feature=+crt-static";

  # No unit tests exist for these crates (parity checks run via each binary's `--self-test` in-guest);
  # also avoids running cross-compiled test binaries on the build host.
  doCheck = false;
}
