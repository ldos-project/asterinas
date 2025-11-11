#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

set -e

NETTEST_DIR=/test/raid1
cd ${NETTEST_DIR}

echo "Start raid1 test......"

./raid_smoke_test

echo "All raid1 test passed"