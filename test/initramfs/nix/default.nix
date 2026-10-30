{ target ? "x86_64", enableBenchmarkTest ? false, enableConformanceTest ? false
, enableRegressionTest ? false, conformanceTestSuite ? "ltp"
, conformanceTestWorkDir ? "/tmp", conformanceTestSelector ? ""
, regressionTestPlatform ? "asterinas", dnsServer ? "none", smp ? 1
, initramfsCompressed ? true, authorized_keys ? "", enablePython ? false
, benchmarkName ? "none", system ? builtins.currentSystem, }:
let
  crossSystem.config = if target == "x86_64" then
    "x86_64-unknown-linux-gnu"
  else if target == "riscv64" then
    "riscv64-unknown-linux-gnu"
  else if target == "aarch64" then
    "aarch64-unknown-linux-gnu"
  else
    throw "Target arch ${target} not yet supported.";

  pkgs = import ../../../distro/nixpkgs.nix {
    config = { };
    overlays = [ ];
    inherit system crossSystem;
  };
in rec {
  # Packages needed by initramfs
  busybox = pkgs.busybox;
  benchmark = pkgs.callPackage ./benchmark { inherit benchmarkName; };
  conformance = pkgs.callPackage ./conformance {
    inherit smp;
    testSuite = conformanceTestSuite;
    workDir = conformanceTestWorkDir;
    testSelector = conformanceTestSelector;
  };
  regression =
    pkgs.callPackage ./regression { testPlatform = regressionTestPlatform; };
  # The OQueue filesystem CBOR reader, always included so `/oqueues` can be inspected in-guest.
  oqueue-reader = pkgs.callPackage ./oqueue-reader.nix { };
  # The userspace RAID-1 selection policy server, always included (like `oqueue-reader`); it is
  # only exercised when booted with `raid.selection=userspace`.
  raid-policy-server = pkgs.callPackage ./raid-policy-server.nix { };
  # The userspace peer of the OQFS round-trip microbenchmark.
  oqbench-server = pkgs.callPackage ./oqbench-server.nix { };

  initramfs = pkgs.callPackage ./initramfs.nix {
    inherit busybox oqueue-reader raid-policy-server oqbench-server;
    benchmark = if enableBenchmarkTest then benchmark else null;
    conformance = if enableConformanceTest then conformance else null;
    regression = if enableRegressionTest then regression else null;
    dnsServer = dnsServer;
    authorized_keys = authorized_keys;
    enablePython = enablePython;
  };
  initramfs-image = pkgs.callPackage ./initramfs-image.nix {
    inherit initramfs;
    compressed = initramfsCompressed;
  };
  rootfs-image = pkgs.callPackage ./rootfs-image.nix { inherit initramfs; };

  # Packages needed by host
  apacheHttpd = pkgs.apacheHttpd;
  iperf3 = pkgs.iperf3;
  libmemcached = pkgs.libmemcached.overrideAttrs (_: {
    configureFlags = [ "--enable-memaslap" ];
    LDFLAGS = "-lpthread";
    CPPFLAGS = "-fcommon -fpermissive";
  });
  lmbench = pkgs.callPackage ./benchmark/lmbench.nix { };
  redis = (pkgs.redis.overrideAttrs (old: {
    doCheck = false;
    makeFlags = (old.makeFlags or [ ]) ++ [
      "CC=${pkgs.stdenv.cc.targetPrefix}cc"
      "LD=${pkgs.stdenv.cc.targetPrefix}cc"
    ];
  })).override { withSystemd = false; };
  ycsb = pkgs.callPackage ./ycsb.nix { };
}
