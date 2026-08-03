# SPDX-License-Identifier: MPL-2.0
# Builds `raid_policy_server`, the userspace RAID-1 read-selection policy server that talks to the
# kernel over OQFS (`/oqueues`).
{ stdenv, rustc, glibc }:
stdenv.mkDerivation {
  pname = "raid-policy-server";
  version = "0.1.0";
  dontUnpack = true;
  nativeBuildInputs = [ rustc ];
  # This binary must be copied straight into the initramfs (outside the nix closure), like
  # `oqueue-reader`. A dynamically-linked binary's ELF interpreter and libc would point into the
  # nix store, which is not present in the initramfs, so the guest could never `exec` it. Static
  # linking avoids that entirely. `glibc.static` provides `libc.a`.
  buildInputs = [ glibc glibc.static ];
  buildCommand = ''
    mkdir -p $out/bin
    rustc -O --edition 2021 \
      --crate-name raid_policy_server \
      -C target-feature=+crt-static \
      -C linker=$CC \
      ${./../src/raid_policy_server/main.rs} \
      -o $out/bin/raid_policy_server
  '';
}
