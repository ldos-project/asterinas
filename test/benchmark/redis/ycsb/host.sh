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

# Run YCSB + redis bench
echo "Running YCSB bench connected to $GUEST_SERVER_IP_ADDRESS"
export JAVA_HOME=./jre1.8.0_471
YCSB_DIR=./ycsb-0.17.0
$YCSB_DIR/bin/ycsb.sh load redis -p redis.host="$GUEST_SERVER_IP_ADDRESS" -p redis.port="6379" -P .$YCSB_DIR/workloads/workloada
$YCSB_DIR/bin/ycsb.sh run redis -p redis.host="$GUEST_SERVER_IP_ADDRESS" -p redis.port="6379" -P $YCSB_DIR/workloads/workloada

# The trap will automatically stop the guest VM when the script exits
