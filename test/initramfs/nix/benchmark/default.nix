{ lib, stdenvNoCC, callPackage, hostPlatform, pkgsHostTarget, }: rec {
  # Use `--esx` flag to enable `CONFIG_NO_SHM` and disable `CONFIG_HAVE_TIMERFD_CREATE`.
  fio = pkgsHostTarget.fio.overrideAttrs (_: { configureFlags = [ "--esx" ]; });
  hackbench = callPackage ./hackbench.nix { };
  iperf3 = pkgsHostTarget.iperf3;
  lmbench = callPackage ./lmbench.nix { };
  memcached = pkgsHostTarget.memcached;
  nginx = pkgsHostTarget.nginx;
  redis =
    (pkgsHostTarget.redis.overrideAttrs (_: { doCheck = false; })).override {
      withSystemd = false;
    };
  rocksdb = (pkgsHostTarget.rocksdb.overrideAttrs (old: {
    # Rewrites db_bench/ldb/sst_dump rpaths from their actual NEEDED entries across
    # all buildInputs (snappy, lz4, zstd, zlib, bzip2, liburing, gflags, gcc libs),
    # and pulls those libs into the runtime closure so they reach the guest's /nix/store.
    nativeBuildInputs =
      old.nativeBuildInputs ++ [ pkgsHostTarget.autoPatchelfHook ];
    buildInputs = old.buildInputs ++ [ pkgsHostTarget.gflags ];
    # Needed to build db_bench: https://github.com/facebook/rocksdb/blob/5fbc1cd5bcf63782675168b98e114151490de6d9/tools/db_bench.cc#L10-L15
    cmakeFlags = old.cmakeFlags
      ++ [ "-DWITH_BENCHMARK_TOOLS=1" "-DWITH_GFLAGS=1" ];
    # Needed to make the db_bench binary exist (autoPatchelfHook in postFixup
    # replaces the base package's short rpath with one covering every NEEDED lib)
    preInstall = old.preInstall + ''
      cp db_bench${hostPlatform.extensions.executable} $tools/bin/
      # CMake embeds the build-tree path (/build/...) in db_bench's rpath, which
      # fixupPhase's forbidden-reference check rejects. Clear it here; autoPatchelfHook
      # writes the real rpath from db_bench's NEEDED entries during postFixup.
      patchelf --remove-rpath $tools/bin/db_bench${hostPlatform.extensions.executable}
    '';
  }));

  schbench = callPackage ./schbench.nix { };
  sqlite-speedtest1 = callPackage ./sqlite-speedtest1.nix { };
  sysbench = if hostPlatform.isx86_64 then pkgsHostTarget.sysbench else null;

  package = stdenvNoCC.mkDerivation {
    pname = "benchmark";
    version = "0.1.0";
    src = lib.fileset.toSource {
      root = ./../../src/benchmark;
      fileset = ./../../src/benchmark;
    };

    buildCommand = ''
      mkdir -p $out/bin
      cp -r ${fio}/bin/fio $out/bin/
      cp -r ${hackbench}/bin/hackbench $out/bin/
      cp -r ${iperf3}/bin/iperf3 $out/bin/
      cp -r ${memcached}/bin/memcached $out/bin/
      cp -r ${nginx}/bin/nginx $out/bin/
      cp -r ${redis}/bin/redis-server $out/bin/
      cp -r ${rocksdb.tools}/bin/db_bench $out/bin/
      cp -r ${schbench}/bin/schbench $out/bin/
      cp -r ${sqlite-speedtest1}/bin/sqlite-speedtest1 $out/bin/

      mkdir -p $out/bin/lmbench
      cp -r ${lmbench}/bin/* $out/bin/lmbench/

      mkdir -p $out/nginx/conf
      cp -r ${nginx}/conf/* $out/nginx/conf/

      ${lib.optionalString (sysbench != null) ''
        cp -r ${sysbench}/bin/sysbench $out/bin/
      ''}

      cp -r $src/* $out/
    '';
  };
}
