# SPDX-License-Identifier: MPL-2.0
# Builds the YCSB benchmark client used by the ycsb benchmark job
# Fetches the YCSB repo from https://github.com/ldos-project/YCSB/tree/mootch/mariposa-benchmark-branch,
# and builds it in distribution mode

{ lib, maven, jre, fetchFromGitHub, makeWrapper }:

maven.buildMavenPackage rec {
  pname = "ycsb";
  version = "0.18.0";

  src = fetchFromGitHub {
    owner = "ldos-project";
    repo = "YCSB";
    rev = "6627681826fdc96facc7b777300d6c47b17e70bc";
    hash = "sha256-OZrX8qK0MiKvif5ILSsU/RbQAS6yJzClw1jHjNq/oiU="; 
  };

  mvnHash = "sha256-PhplQ8Cep803JZzZslq39U2BKP0NGHqQzk9hkKPmlPw=";

  # build just the distribution module, core, redis, and memcached
  # these are the only things needed by the benchmark
  # this creates the distribution tar file
  mvnParameters = lib.escapeShellArgs [
    "-pl"
    "distribution"
    "-am"
    "-DskipTests"
    "-Dcheckstyle.skip=true"
  ];

  nativeBuildInputs = [ makeWrapper ];

  # untar YCSB distribution tar file, then wrap it with jre and python
  installPhase = ''
    runHook preInstall

    ycsb_dir="$out/share/ycsb"
    mkdir -p "$ycsb_dir"
    tar -xzf distribution/target/ycsb-*.tar.gz -C "$ycsb_dir" --strip-components=1

    mkdir -p "$out/bin"
    # only jre pinned instead of python + jre
    # ycsb only needs base python3 so just use the one on the host machine
    makeWrapper "$ycsb_dir/bin/ycsb" "$out/bin/ycsb" \
      --prefix PATH : "${lib.makeBinPath [ jre ]}"

    runHook postInstall
  '';
}
