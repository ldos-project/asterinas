#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

set -e

echo "*** Running the OQFS kernel<->user round-trip microbenchmark ***"

# The kernel side is configured entirely from the kernel command line, so read the two knobs that
# are ours to act on from there rather than inventing a second configuration channel.
peer_compute=$(sed -n 's/.*oqbench\.peer_compute=\([0-9]*\).*/\1/p' /proc/cmdline)
busy_procs=$(sed -n 's/.*oqbench\.busy_procs=\([0-9]*\).*/\1/p' /proc/cmdline)

i=0
while [ "$i" -lt "${busy_procs:-0}" ]; do
    sh -c 'while true; do :; done' &
    i=$((i + 1))
done

# Returns once the kernel has captured every sample and released us; the init process then powers
# the machine off.
oqbench_server --compute "${peer_compute:-0}"
