#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -e
set -o pipefail

# Set BENCHMARK_ROOT to the parent directory of the current directory if it is not set
BENCHMARK_ROOT="${BENCHMARK_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." &>/dev/null && pwd)}"
# Set the log file
LINUX_OUTPUT="${BENCHMARK_ROOT}/linux_output.txt"
ASTER_OUTPUT="${BENCHMARK_ROOT}/aster_output.txt"
# Dependencies for Linux
LINUX_DEPENDENCIES_DIR="/opt/linux_binary_cache"
LINUX_KERNEL="${LINUX_DEPENDENCIES_DIR}/vmlinuz"
LINUX_KERNEL_VERSION="5.15.0-105"
LINUX_MODULES_DIR="${BENCHMARK_ROOT}/../build/initramfs/lib/modules/${LINUX_KERNEL_VERSION}/kernel"
WGET_SCRIPT="${BENCHMARK_ROOT}/../../tools/atomic_wget.sh"

# Prepare Linux kernel and modules
prepare_libs() {
    # Download the Linux kernel and modules
    mkdir -p "${LINUX_DEPENDENCIES_DIR}"

    # Array of files to download and their URLs
    declare -A files=(
        ["${LINUX_KERNEL}"]="https://raw.githubusercontent.com/asterinas/linux_binary_cache/14598b6/vmlinuz-${LINUX_KERNEL_VERSION}"
    )

    # Download files if they don't exist
    for file in "${!files[@]}"; do
        if [ ! -f "$file" ]; then
            echo "Downloading ${file##*/}..."
            ${WGET_SCRIPT} "$file" "${files[$file]}" || {
                echo "Failed to download ${file##*/}."
                exit 1
            }
        fi
    done
}

# Prepare fs for Linux
prepare_fs() {
    # Disable unsupported ext2 features of Asterinas on Linux to ensure fairness
    mke2fs -F -O ^ext_attr -O ^resize_inode -O ^dir_index ${BENCHMARK_ROOT}/../build/ext2.img
    make initramfs BENCHMARK=${benchmark}
}

JRE_URL="https://sdlc-esd.oracle.com/ESD6/JSCDL/jdk/8u481-b10/0d06828d282343ea81775b28020a7cd3/jre-8u481-linux-x64.tar.gz?GroupName=JSC&FilePath=/ESD6/JSCDL/jdk/8u481-b10/0d06828d282343ea81775b28020a7cd3/jre-8u481-linux-x64.tar.gz&BHost=javadl.sun.com&File=jre-8u481-linux-x64.tar.gz&AuthParam=1770754781_ce959abf084ce735fb0cae968215cbb6&ext=.gz"
JRE_PATH="jre1.8.0_471"

YCSB_URL="https://github.com/brianfrankcooper/YCSB/releases/download/0.17.0/ycsb-0.17.0.tar.gz"
YCSB_PATH="ycsb-0.17.0"

prepare_ycsb() {
  if [ ! -d "$JRE_PATH" ]; then
    wget "$JRE_URL" -O jre.tar.gz
    tar -xvf ./jre.tar.gz
  fi
  if [ ! -d "$YCSB_PATH" ]; then
    curl -O --location $YCSB_URL
    tar xfvz "ycsb-0.17.0.tar.gz"
  fi
}
