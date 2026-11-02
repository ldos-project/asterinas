#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

# Find nix packages which are create by the asterinas build and remove all versions of them that are
# no longer accessible from a "GC root".
#
# Due to issues in other clean scripts, this can miss the most recent version of packages because
# they are still referenced by the build directory in some way.

set -euo pipefail

store_paths=()
store_path_list=$(mktemp)
trap 'rm -f "$store_path_list"' EXIT

# Package names defined by this repo. Generic names make this an approximate match.
project_package_names='^(aster_nixos_installer|asterinas-osdk-bin|hello-asterinas|initramfs|initramfs-image|oqueue-reader|raid-policy-server|oqbench-server|dropbear-conf|resolv-conf|host-resolv-conf|gvisor-libs|gvisor-prebuilt|benchmark|conformance|regression)(-[0-9].*)?$'

nix --extra-experimental-features nix-command path-info --all > "$store_path_list"
while IFS= read -r store_path; do
    package_name=${store_path##*/}
    package_name=${package_name#*-}
    package_name=${package_name%.drv}
    if [[ "$package_name" =~ $project_package_names ]]; then
        store_paths+=("$store_path")
    fi
done < "$store_path_list"

if ((${#store_paths[@]})); then
    nix --extra-experimental-features nix-command store delete --recursive --skip-alive "${store_paths[@]}"
fi
