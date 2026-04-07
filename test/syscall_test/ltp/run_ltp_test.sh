#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

# Run LTP syscall tests.
#
# Usage: run_ltp_test.sh [TEST_PATTERN...]
#
# If no arguments are given, all tests not on the blocklist are run. If one or more TEST_PATTERN
# arguments are given, only tests whose names match any of the given patterns are run.  Each
# argument is treated as a basic regular expression matched against the test name.
#
# To run a subset of tests via the top-level make target, pass the patterns through INITARGS:
#
#   make run AUTO_TEST=syscall SYSCALL_TEST_SUITE=ltp INITARGS="futex"
#   make run AUTO_TEST=syscall SYSCALL_TEST_SUITE=ltp INITARGS="futex mmap"

LTP_DIR=$(dirname "$0")
TEST_TMP_DIR=${SYSCALL_TEST_WORKDIR:-/tmp}
LOG_FILE=$TEST_TMP_DIR/result.log
RESULT=0

rm -f $LOG_FILE

EXTRA_ARGS=
if [ $# -gt 0 ]; then
    PATTERN=$(echo "$@" | tr ' ' '\|')
    EXTRA_ARGS="-s '$PATTERN'"
else

CREATE_ENTRIES=1 $LTP_DIR/runltp -f syscalls -p -d $TEST_TMP_DIR -l $LOG_FILE $EXTRA_ARGS
if [ $? -ne 0 ]; then
    RESULT=1
fi

cat $LOG_FILE
if ! grep -q "Total Failures: 0" $LOG_FILE; then
    RESULT=1
fi

exit $RESULT
