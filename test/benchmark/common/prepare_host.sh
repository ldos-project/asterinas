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

JDK_URL="https://download.oracle.com/java/25/latest/jdk-25_linux-x64_bin.tar.gz"
MVN_URL="https://archive.apache.org/dist/maven/maven-3/3.9.12/binaries/apache-maven-3.9.12-bin.tar.gz"
export MVN_DIR=$(realpath ".cache/apache-maven-3.9.12")
export YCSB_PATH=$(realpath ".cache/ycsb")
export JDK_PATH=$(realpath ".cache/jdk-25.0.2")
export JAVA_HOME=$JDK_PATH

prepare_ycsb() {
  mkdir -p .cache
  pushd .cache && {
    if [ ! -d "$JDK_PATH" ]; then
      wget "$JDK_URL" -O jdk.tar.gz
      tar -xvf ./jdk.tar.gz
    fi

    if [ ! -d "$MVN_DIR" ]; then
      wget $MVN_URL
      tar -xvf ./apache-maven-3.9.12-bin.tar.gz
    fi

    if [ ! -d "$YCSB_PATH" ]; then
      git clone https://github.com/tewaro/YCSB.git -b tewaro/quickfix-coreworkload-deletes-master --depth=1 $YCSB_PATH

      # Build
      pushd $YCSB_PATH
      $MVN_DIR/bin/mvn -pl site.ycsb:redis-binding -am clean package
      $MVN_DIR/bin/mvn -pl site.ycsb:memcached-binding -am clean package
      popd
    fi
  }; popd
}
