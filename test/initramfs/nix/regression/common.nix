{ lib, stdenv, glibc, hostPlatform, rustPlatform, pkgsBuildHost, dir
, testPlatform ? "asterinas", extraAttrs ? { }, extraBuildInputs ? [ ] }:
let
  cargoManifest = ./../../src/regression/${dir}/Cargo.toml;
  cargoLock = ./../../src/regression/${dir}/Cargo.lock;
  hasRustCrate = builtins.pathExists cargoManifest;
in stdenv.mkDerivation ({
  pname = "${dir}-test";
  version = "0.1.0";
  src = lib.fileset.toSource {
    root = ./../../src/regression;
    fileset = lib.fileset.unions [
      ./../../src/regression/common
      ./../../src/regression/${dir}
    ];
  };

  HOST_PLATFORM = "${hostPlatform.system}";
  TEST_PLATFORM = "${testPlatform}";

  CC = "${stdenv.cc.targetPrefix}cc";
  RUST_TARGET = stdenv.hostPlatform.rust.rustcTarget;
  RUSTFLAGS =
    "-C target-feature=+crt-static -C linker=${stdenv.cc.targetPrefix}cc";

  buildInputs = [ glibc glibc.static ] ++ extraBuildInputs;
  nativeBuildInputs =
    [ rustPlatform.cargoSetupHook pkgsBuildHost.cargo pkgsBuildHost.rustc ];
  buildCommand = ''
    ${lib.optionalString hasRustCrate ''
      cargoSetupPostUnpackHook
      export CARGO_HOME="$PWD/.cargo"
    ''}

    mkdir -p $out
    make --no-print-directory BUILD_DIR=$(mktemp -d) OUTPUT_DIR=$out -C "$src/${dir}"
  '';
} // lib.optionalAttrs hasRustCrate {
  cargoDeps = rustPlatform.importCargoLock { lockFile = cargoLock; };
} // extraAttrs)
