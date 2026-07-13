# SPDX-License-Identifier: MPL-2.0
# Builds `read_oqueues`, the in-guest CBOR decoder for the OQueue filesystem (`/oqueues`).
{ stdenv, glibc }:
stdenv.mkDerivation {
  pname = "oqueue-reader";
  version = "0.1.0";
  dontUnpack = true;
  # `glibc.static` provides `libc.a` for static linking, matching the regression tests.
  buildInputs = [ glibc glibc.static ];
  # The tool uses only the C standard library, so `-nostdlib++` lets it link statically without
  # pulling in libstdc++ (which is not present in the initramfs).
  buildCommand = ''
    mkdir -p $out/bin
    $CXX -O2 -static -nostdlib++ -fno-exceptions -fno-rtti -Wall -Werror \
      ${./../src/oqueue/read_oqueues.cpp} -o $out/bin/read_oqueues
  '';
}
