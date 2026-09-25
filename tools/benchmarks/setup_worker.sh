#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

# Minimal setup for a machine that runs the benchmark suite via
# tools/benchmarks/run_benchmarks.sh. Installs Docker, act and howdone's
# dependencies, and loads the virtualisation modules the benchmarks require.
#
# Targets Debian/Ubuntu. Run as a normal user with sudo access:
#
#   tools/benchmarks/setup_worker.sh
#
# Log out and back in afterwards, so the new group memberships take effect.
# Everything here is idempotent and safe to re-run.

set -e

if [ "$(id -u)" -eq 0 ]; then
    echo "Error: run this as a normal user with sudo access, not as root," >&2
    echo "so that the docker and kvm groups are granted to the right account." >&2
    exit 1
fi

echo "==> Base packages"
sudo apt-get update
# python3-yaml is what howdone asks for by name when PyYAML is missing.
sudo apt-get install -y curl git make python3 python3-yaml

echo "==> Docker"
if command -v docker >/dev/null 2>&1; then
    echo "already installed: $(docker --version)"
else
    curl -fsSL https://get.docker.com | sudo sh
fi
sudo usermod -aG docker "$USER"

echo "==> act"
if command -v act >/dev/null 2>&1; then
    echo "already installed: $(act --version)"
else
    curl -fsSL https://raw.githubusercontent.com/nektos/act/master/install.sh \
        | sudo bash -s -- -b /usr/local/bin
fi
# act prompts for a default runner image on first use unless this exists. The
# benchmark job takes its image from `container:`, so the value is never used.
if [ ! -f "$HOME/.config/act/actrc" ]; then
    mkdir -p "$HOME/.config/act"
    echo "-P ubuntu-latest=catthehacker/ubuntu:act-latest" > "$HOME/.config/act/actrc"
fi

echo "==> Virtualisation modules"
# The benchmarks hard-code --enable-kvm and run with NETDEV=tap VHOST=on.
if grep -q vmx /proc/cpuinfo; then
    KVM_MODULE=kvm_intel
elif grep -q svm /proc/cpuinfo; then
    KVM_MODULE=kvm_amd
else
    KVM_MODULE=""
    echo "WARNING: no vmx/svm in /proc/cpuinfo - this host cannot run KVM." >&2
fi
for module in $KVM_MODULE vhost_net tun; do
    sudo modprobe "$module" || echo "WARNING: could not load $module" >&2
done
printf '%s\n' $KVM_MODULE vhost_net tun | sudo tee /etc/modules-load.d/asterinas-benchmark.conf >/dev/null
sudo usermod -aG kvm "$USER"

echo
echo "==> Checks"
status() { if [ -e "$2" ]; then echo "  ok      $1 ($2)"; else echo "  MISSING $1 ($2)"; fi; }
status "KVM"       /dev/kvm
status "vhost-net" /dev/vhost-net
status "tun"       /dev/net/tun
python3 -c 'import yaml' 2>/dev/null \
    && echo "  ok      PyYAML (for howdone)" \
    || echo "  MISSING PyYAML (for howdone)"
echo "  info    $(nproc) CPUs, $(free -g | awk '/^Mem:/{print $2}') GiB RAM, \
$(df -BG --output=avail . | tail -1 | tr -d ' ') free on $(pwd)"
echo
echo "The suite needs ~40 GiB of RAM for the full matrix: the default guest is"
echo "MEM=8G and lmbench/ramfs_create_delete_files_0k_ops asks for 32G."
echo
echo "Done. Log out and back in, then run: tools/benchmarks/run_benchmarks.sh sysbench/cpu_lat"
