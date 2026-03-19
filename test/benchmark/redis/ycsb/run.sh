#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

echo "Running redis server"
/usr/local/redis/bin/redis-server /benchmark/redis/ycsb/ycsb.conf &
PID=$!
echo "waiting?"
echo "PID=$PID"

while true; do
  { cat /proc/$PID/status || true; } | grep HWM
  sleep 0.1
done

wait $PID
