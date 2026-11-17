#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

/benchmark/bin/stress-ng --yield 16 -t 60s --metrics-brief
