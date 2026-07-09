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
LINUX_KERNEL_VERSION="6.16.0"
LINUX_MODULES_DIR="${BENCHMARK_ROOT}/../build/initramfs/lib/modules/${LINUX_KERNEL_VERSION}/kernel"
WGET_SCRIPT="${BENCHMARK_ROOT}/../../../tools/atomic_wget.sh"

# Prepare Linux kernel and modules
prepare_libs() {
    mkdir -p "${LINUX_DEPENDENCIES_DIR}"

    # Array of files to download and their URLs
    declare -A files=(
        # Currently there are no files that are available this way. All files are in the docker image.
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
    mke2fs -F -O ^ext_attr -O ^resize_inode -O ^dir_index ${BENCHMARK_ROOT}/../../build/ext2.img
    make initramfs BENCHMARK=${benchmark}
}

JDK_URL="https://download.oracle.com/java/25/latest/jdk-25_linux-x64_bin.tar.gz"
MVN_URL="https://archive.apache.org/dist/maven/maven-3/3.9.12/binaries/apache-maven-3.9.12-bin.tar.gz"
export CACHE_DIR=$(realpath -m ".cache")
export MVN_DIR=$(realpath -m "${CACHE_DIR}/apache-maven-3.9.12")
export YCSB_PATH=$(realpath -m "${CACHE_DIR}/ycsb")
resolve_jdk_path() {
  local jdk_latest=$(ls -1d ${CACHE_DIR}/jdk-* 2>/dev/null | sort -V | tail -n 1)
  if [ -z "$jdk_latest" ]; then
    return 1
  fi
  
  realpath "$jdk_latest"
}

export JDK_PATH=$(resolve_jdk_path || true)
export JAVA_HOME=$JDK_PATH


prepare_ycsb() {
  mkdir -p .cache
  pushd .cache
  trap 'popd' ERR
  if [ ! -d "$JDK_PATH" ]; then
    wget "$JDK_URL" -O jdk.tar.gz
    tar -xvf ./jdk.tar.gz
    export JDK_PATH=$(resolve_jdk_path)
    export JAVA_HOME=$JDK_PATH
  fi

  if [ ! -d "$MVN_DIR" ]; then
    wget $MVN_URL
    tar -xvf ./apache-maven-3.9.12-bin.tar.gz
  fi
  trap - ERR
  popd

  if [ ! -d "$YCSB_PATH" ]; then
    # Use custom fork of YCSB with delete support
    git clone https://github.com/tewaro/YCSB.git -b tewaro/quickfix-coreworkload-deletes-master --depth=1 $YCSB_PATH

    # Build
    if [ ! -x "$JAVA_HOME/bin/java" ]; then
      echo "Invalid JAVA_HOME: $JAVA_HOME" >&2
      exit 1
    fi
    pushd $YCSB_PATH
    $MVN_DIR/bin/mvn -pl site.ycsb:redis-binding -am clean package
    $MVN_DIR/bin/mvn -pl site.ycsb:memcached-binding -am clean package
    popd
  fi
}
