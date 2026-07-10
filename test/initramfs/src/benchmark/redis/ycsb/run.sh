#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

echo "Running redis server"
/benchmark/bin/redis-server /etc/ycsb.conf &
echo "waiting for stop message"
/bin/nc -l -s 10.0.2.15 -p 5201 -e true
echo "done"
