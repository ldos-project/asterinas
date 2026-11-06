# Nightly Benchmarking

## Overview

This directory contains scripts which run the benchmark workflow [benchmark_x86.yml](../../.github/workflows/benchmark_x86.yml) locally using [Nektos act](https://github.com/nektos/act) and upload the results to the LDOS [benchmark dashboard](https://github.com/ldos-project/benchmark).

## Setup

1. Install [Docker](https://docs.docker.com/engine/install/).
    - Follow the [post install steps](https://docs.docker.com/engine/install/linux-postinstall) to add the user scheduling the benchmark job to the docker group.
    - Note: the benchmarks run QEMU with KVM and vhost tap networking so make sure those modules are loaded.
2. Install [act](https://nektosact.com/installation/nix.html) with [Nix](https://nixos.org/download/#nix-install-linux).
    - Add BENCHMARK_SECRET=\<PAT\> in `asterinas/.secrets` to a GitHub PAT with Contents read and write access to the [dashboard repository](https://github.com/ldos-project/benchmark).
    - Set the permissions on `asterinas/.secrets` with `chmod 600 asterinas/.secrets`.
3. Install PyYAML: `sudo apt install -y python3-yaml`

## Usage

[nightly_benchmarks.sh](./nightly_benchmarks.sh) runs the benchmarks and uploads the results.

1. [run_benchmarks.sh](./run_benchmarks.sh) runs the full benchmark matrix listed in [benchmark_x86.yml](../../.github/workflows/benchmark_x86.yml) and writes the results and logs to a `run-<timestamp>` directory.
2. [upload_benchmarks.sh](./upload_benchmarks.sh) compresses the results and uploads them to the LDOS [benchmark dashboard](https://github.com/ldos-project/benchmark).
