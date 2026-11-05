#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

set -e

echo "Start sched_userspace test......"

/test/process/sched_userspace/sched_userspace

echo "All sched_userspace test passed"
