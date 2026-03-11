#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

set -e

# Function to stop the guest VM
stop_guest() {
    echo "Stopping guest VM..."
    # `-r` means if there's no qemu, the kill won't be executed
    pgrep qemu | xargs -r kill
}

# Trap EXIT signal to ensure guest VM is stopped on script exit
trap stop_guest EXIT

# TODO - export some variables like MVN_PATH/YCSB_DIR from prepare_host.sh
export PATH=$PATH:$(realpath ./apache-maven-3.9.12/bin/)

# Run YCSB + redis bench
echo "Running YCSB bench connected to $GUEST_SERVER_IP_ADDRESS"
export JAVA_HOME=$(realpath "./jdk-25.0.2")
$YCSB_PATH/bin/ycsb load redis -p redis.host="$GUEST_SERVER_IP_ADDRESS" -p redis.port="6379" -P $YCSB_DIR/workloads/workloada
$YCSB_PATH/bin/ycsb run redis -p redis.host="$GUEST_SERVER_IP_ADDRESS" -p redis.port="6379" -P $YCSB_DIR/workloads/workloada

# The trap will automatically stop the guest VM when the script exits
