# SPDX-License-Identifier: MPL-2.0
# Builds the YCSB benchmark client used by the ycsb benchmark job
# Fetches the YCSB repo from https://github.com/ldos-project/YCSB/tree/mootch/mariposa-benchmark-branch,
# and builds it in distribution mode

{ lib, maven, jre, python3, fetchFromGitHub, makeWrapper }:

maven.buildMavenPackage rec {
  pname = "ycsb";
  version = "0.18.0";

  src = fetchFromGitHub {
    owner = "ldos-project";
    repo = "YCSB";
    rev = "d050dfab3f6a0c28c50068f47884f0b83c95f250";
    hash = "sha256-Dgbw7263+jwomBjoXnp93czeY2F9U5qUeP3pHmKlQDo=";
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
    makeWrapper "$ycsb_dir/bin/ycsb" "$out/bin/ycsb" \
      --prefix PATH : "${lib.makeBinPath [ jre python3 ]}" \
      --set-default YCSB_HOME "$ycsb_dir"

    runHook postInstall
  '';
}
