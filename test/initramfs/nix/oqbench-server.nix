# SPDX-License-Identifier: MPL-2.0
# Builds `oqbench_server`, the userspace peer of the OQFS round-trip microbenchmark. Modelled on
# `raid-policy-server.nix`.
{ rustPlatform, glibc }:
rustPlatform.buildRustPackage {
  pname = "oqbench-server";
  version = "0.1.0";
  src = ../src/oqbench_server;
  cargoLock.lockFile = ../src/oqbench_server/Cargo.lock;

  # Static linking so the binary can be copied straight into the initramfs, outside the nix closure.
  buildInputs = [ glibc glibc.static ];
  RUSTFLAGS = "-C target-feature=+crt-static";

  doCheck = false;
}
