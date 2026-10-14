#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

set -e

./oqfs_produce_consume_test
./oqfs_strong_observe_test
./oqfs_weak_observe_test
