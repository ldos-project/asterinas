{ lib, stdenv, glibc, hostPlatform, dir, testPlatform ? "asterinas"
, extraAttrs ? { }, extraBuildInputs ? [ ], extraNativeBuildInputs ? [ ] }:
stdenv.mkDerivation ({
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

  buildInputs = [ glibc glibc.static ] ++ extraBuildInputs;
  nativeBuildInputs = extraNativeBuildInputs;
  buildCommand = ''
    if declare -F cargoSetupPostUnpackHook >/dev/null; then
      cargoSetupPostUnpackHook
      export CARGO_HOME="$PWD/.cargo"
    fi

    mkdir -p $out
    make --no-print-directory BUILD_DIR=$(mktemp -d) OUTPUT_DIR=$out -C "$src/${dir}"
  '';
} // extraAttrs)
