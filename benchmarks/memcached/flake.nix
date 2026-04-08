{
  description = "Memcached benchmark — server (guest) and memaslap client (host)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    memcached-src = {
      url = "https://memcached.org/files/memcached-1.6.32.tar.gz"; # version: 1.6.32
      flake = false;
    };

    libmemcached-src = {
      url = "https://launchpad.net/libmemcached/1.0/1.0.18/+download/libmemcached-1.0.18.tar.gz"; # version: 1.0.18
      flake = false;
    };
  };

  outputs = { nixpkgs, memcached-src, libmemcached-src, ... }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};

      # Versions are kept as comments on the input URLs above; update both together.
      memcachedVersion    = "1.6.32";
      libmemcachedVersion = "1.0.18";

      packages = {

        # memcached server — runs inside the Asterinas guest via initramfs.
        # Built statically (musl) so it has no dynamic library dependencies;
        # the nix store is not present in the Asterinas guest environment.
        memcached = pkgs.pkgsStatic.memcached.overrideAttrs (_old: {
          version = memcachedVersion;
          src = memcached-src;
        });

        # memaslap benchmark client — runs on the host.
        # Build mirrors tools/docker/Dockerfile: autotools, --enable-memaslap,
        # -fcommon/-fpermissive workarounds for the old 1.0.18 codebase.
        memaslap = pkgs.stdenv.mkDerivation {
          pname = "memaslap";
          version = libmemcachedVersion;
          src = libmemcached-src;

          nativeBuildInputs = with pkgs; [ pkg-config ];
          buildInputs = with pkgs; [ libevent cyrus_sasl ];

          CFLAGS   = "-fpermissive -fcommon";
          CPPFLAGS = "-fcommon -fpermissive";
          LDFLAGS  = "-lpthread";

          configureFlags = [ "--enable-memaslap" ];

          # Dockerfile overrides CPPFLAGS for the make step (drops -fpermissive).
          preBuild = "export CPPFLAGS=-fcommon";

          installPhase = ''
            runHook preInstall
            install -Dm755 clients/memaslap $out/bin/memaslap
            runHook postInstall
          '';
        };

      };
    in {
      packages.${system}      = packages;
      defaultPackage.${system} = packages.memcached;
    };
}
