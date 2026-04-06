#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

set -e

# Function to stop the guest VM
stop_guest() {
    echo "Stopping guest VM..."
    # The message doesn't matter here, any data will make the vm exit
    echo stop | nc 10.0.2.15 5201
    sleep 15
    # `-r` means if there's no qemu, the kill won't be executed
    pgrep qemu | xargs -r kill
}

# Trap EXIT signal to ensure guest VM is stopped on script exit
trap stop_guest EXIT

# TODO - export some variables like MVN_PATH/YCSB_DIR from prepare_host.sh
export PATH=$PATH:$(realpath .cache/apache-maven-3.9.12/bin/)

# Run YCSB + redis bench
echo "Running YCSB bench connected to $GUEST_SERVER_IP_ADDRESS"
export JAVA_HOME=$(realpath ".cache/jdk-25.0.2")

cd $YCSB_PATH/

FIELDLENGTH=4096



for _ in $(seq 1 4096); do
  ./bin/ycsb load redis -p redis.host="$GUEST_SERVER_IP_ADDRESS" -p redis.port="6379" -P ./workloads/workloada \
  -p recordcount=4096\
  -p fieldcount=1\
  -p fieldlength=$FIELDLENGTH\
  -p minfieldlength=4096\
  -p insertstart=0\
  -p fieldlengthdistribution=uniform

  ./bin/ycsb run redis -p redis.host="$GUEST_SERVER_IP_ADDRESS" -p redis.port="6379" -P ./workloads/workloada \
    -p operationcount=4096\
    -p recordcount=4096\
    -p workload=site.ycsb.workloads.CoreWorkload\
    -p readproportion=0.05\
    -p updateproportion=0.00\
    -p scanproportion=0.00\
    -p insertproportion=0.00\
    -p readmodifywriteproportion=0.00\
    -p scanproportion=0.00\
    -p deleteproportion=0.95\
    -p threadcount=16\
    -p fieldcount=1\
    -p fieldlength=$FIELDLENGTH\
    -p minfieldlength=4096\
    -p fieldlengthdistribution=uniform
done

# The trap will automatically stop the guest VM when the script exits
