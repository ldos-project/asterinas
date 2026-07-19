# SPDX-License-Identifier: MPL-2.0
# Builds `read_oqueues`, the in-guest CBOR decoder for the OQueue filesystem (`/oqueues`).
{ stdenv, glibc, libcbor }:
let
  # libcbor ships only a shared library by default, but this tool is copied straight into the
  # initramfs (outside the nix closure), so it must link statically. Rebuild libcbor as a static
  # archive. LTO is disabled so the archive carries a usable symbol index (its LTO objects are
  # otherwise invisible to a plain, non-LTO link).
  libcborStatic = libcbor.overrideAttrs (old: {
    cmakeFlags = (old.cmakeFlags or [ ]) ++ [
      "-DBUILD_SHARED_LIBS=OFF"
      "-DCMAKE_INTERPROCEDURAL_OPTIMIZATION=OFF"
    ];
  });
in stdenv.mkDerivation {
  pname = "oqueue-reader";
  version = "0.1.0";
  dontUnpack = true;
  # `glibc.static` provides `libc.a` for static linking, matching the regression tests.
  buildInputs = [ glibc glibc.static libcborStatic ];
  # The tool uses only the C standard library and libcbor, so `-nostdlib++` lets it link statically
  # without pulling in libstdc++ (which is not present in the initramfs).
  buildCommand = ''
    mkdir -p $out/bin
    $CXX -O2 -static -nostdlib++ -fno-exceptions -fno-rtti -Wall -Werror \
      -I${libcborStatic.dev}/include \
      ${./../src/oqueue/read_oqueues.cpp} \
      -L${libcborStatic}/lib -lcbor \
      -o $out/bin/read_oqueues
  '';
}
