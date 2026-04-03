{
  description = "Asterinas benchmark container images";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    # Authoritative source versions — match tools/docker/Dockerfile exactly.
    # Injected into each benchmark sub-flake via follows so versions are
    # declared in exactly one place.
    memcached-src = {
      url = "https://memcached.org/files/memcached-1.6.32.tar.gz";
      flake = false;
    };

    libmemcached-src = {
      url = "https://launchpad.net/libmemcached/1.0/1.0.18/+download/libmemcached-1.0.18.tar.gz";
      flake = false;
    };

    memcached-benchmark = {
      url = "path:./memcached";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.memcached-src.follows = "memcached-src";
      inputs.libmemcached-src.follows = "libmemcached-src";
    };
  };

  outputs = { self, nixpkgs, memcached-benchmark, ... }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      memcachedPkgs = memcached-benchmark.packages.${system};

      # Base image used by `make docker` — version comes from DOCKER_IMAGE_VERSION.
      # After a version bump, update imageDigest and sha256 by running:
      #   nix-prefetch-docker --imageName ldosproject/asterinas \
      #                       --imageTag 0.15.2-20250613
      baseImage = pkgs.dockerTools.pullImage {
        imageName    = "ldosproject/asterinas";
        finalImageTag = "0.15.2-20250613";
        imageDigest  = "sha256:3cb56ee8fccfaf1924f04fe2895f4471feb8ff6adc8c05dfed872c444d7904bc";
        sha256       = "sha256-rEFAT5t/A8kU9SAtrCKHwImcFslvKVjnhlbUfob2SEU=";
      };
    in {
      packages.${system} = {

        memcached = pkgs.dockerTools.buildLayeredImage {
          name = "ldosproject/asterinas";
          tag  = "benchmark-memcached";

          # Layer on top of the exact image that `make docker` uses, so all
          # system libraries (glibc, libevent, etc.) match the Dockerfile.
          fromImage = baseImage;

          # Both packages land in the nix store inside the container.
          # test/Makefile will be updated to pull from their store paths directly.
          contents = [ memcachedPkgs.memcached memcachedPkgs.memaslap ];

          config.Cmd = [ "/usr/bin/bash" ];
        };

      };
    };
}
