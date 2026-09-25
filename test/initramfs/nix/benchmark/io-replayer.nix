{ lib, stdenv, glibc, }:
# The LAKE I/O trace replayer (https://github.com/utcs-scea/LAKE, src/linnos/io_replayer), the
# workload generator of the RAID I/O benchmark.
stdenv.mkDerivation {
  pname = "io-replayer";
  version = "0.1.0";
  src = lib.fileset.toSource {
    root = ./../../src/io_replayer;
    fileset = ./../../src/io_replayer;
  };

  # Linked statically, so the binary can be dropped into `/benchmark/bin` on its own without
  # dragging a shared-library closure into the initramfs. `glibc.static` supplies `libc.a`.
  buildInputs = [ glibc.static ];

  buildPhase = ''
    runHook preBuild

    $CXX -std=c++11 -O3 -static -o io_replayer replayer.cpp op_replayers.cpp -lpthread

    runHook postBuild
  '';
  installPhase = ''
    runHook preInstall

    mkdir -p $out/bin
    mv io_replayer $out/bin/

    runHook postInstall
  '';
}
