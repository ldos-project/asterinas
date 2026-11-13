#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

/benchmark/bin/stress-ng --iomix 2 --iomix-bytes 256M --temp-path /ext2 -t 60s --metrics-brief
