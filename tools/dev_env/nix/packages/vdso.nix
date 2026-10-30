# SPDX-License-Identifier: MPL-2.0
#
# The Linux x86_64 vDSO embedded by the kernel build, in the layout expected by
# kernel/core/src/vdso.rs: $out/vdso_x86_64.so.
#
# This mirrors the `build-linux` stage of tools/dev_env/docker/kernel-dev/Dockerfile:
# the vDSO is built from the same Linux release instead of being taken from
# the upstream asterinas/linux_vdso repository.
#
# TODO: Like the Docker image, this does not provide the riscv64 or aarch64 vDSO.
{ stdenv, fetchurl, bc, bison, flex, perl, elfutils, openssl, }:

stdenv.mkDerivation rec {
  pname = "asterinas-vdso";
  version = "6.2.10";

  # Keep this in sync with the Dockerfile's linux-6.2.10.tar.xz.
  src = fetchurl {
    url =
      "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-${version}.tar.xz";
    hash = "sha256-V8Viw80nU/IyVJyrBcitdw7YSK6GQBYZx1gb3/rupP4=";
  };

  nativeBuildInputs = [ bc bison flex perl ];
  buildInputs = [ elfutils openssl ];

  # The kernel build manages its own flags.
  hardeningDisable = [ "all" ];

  buildPhase = ''
    runHook preBuild
    make defconfig
    # KCFLAGS=-Wno-array-bounds matches the Docker build, which needs it to
    # avoid a -Werror failure on this old kernel with a modern compiler.
    make prepare
    make -j$NIX_BUILD_CORES KCFLAGS=-Wno-array-bounds arch/x86/entry/vdso
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    mkdir -p $out
    cp arch/x86/entry/vdso/vdso64.so $out/vdso_x86_64.so
    runHook postInstall
  '';

  # Keep the library byte-identical to what the kernel build produced.
  dontStrip = true;
  dontFixup = true;
}
