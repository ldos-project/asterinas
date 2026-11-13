#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

/benchmark/bin/stress-ng --hdd 4 --hdd-bytes 256M --hdd-opts direct --temp-path /ext2 -t 60s --metrics-brief
