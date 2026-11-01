#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

# Runs `sched_rr` with the given arguments. The init process runs its arguments
# with `sh -l`, so it needs a script rather than the binary. Usage, from the host:
#   make run_kernel ENABLE_REGRESSION_TEST=true \
#       INITARGS="/test/process/sched_rr/sched_rr.sh sleep 64" ...

set -e

"$(dirname "$0")/sched_rr" "$@"
