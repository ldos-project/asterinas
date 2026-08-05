#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

set -e

# Every policy program, in the order they are swapped through below. `lowest_index` is the trivial
# drop-in policy added as a separate crate to prove the extensibility bar.
POLICIES="avg_latency roundrobin linnos linnos_plus decision_tree lowest_index"

echo "Start raid1 test......"

# 1. Run each policy program's offline `--self-test` (feature-vector parity checks; no OQueues
#    needed). A mismatch with the kernel's feature vector exits non-zero and fails the whole test.
for p in $POLICIES; do
    echo "raid1: running self-test for policy '$p'"
    /usr/bin/raid_policy_$p --self-test
done
echo "raid1: all policy self-tests passed"

cd /test/raid1

# Poll the supervisor-reported active policy until it equals $1, then return. This asserts the swap
# ACTUALLY TOOK EFFECT rather than sleeping and hoping.
wait_active() {
    want=$1
    i=0
    while [ $i -lt 30 ]; do
        got=$(cat /tmp/raid_policy_active 2>/dev/null || true)
        if [ "$got" = "$want" ]; then
            echo "raid1: policy '$want' is active"
            return 0
        fi
        i=$((i + 1))
        sleep 1
    done
    echo "raid1: FAILED waiting for policy '$want' to become active (last seen: '$got')"
    exit 1
}

# The supervisor boots running the default policy (avg_latency); confirm before the first swap.
wait_active avg_latency

# 2. Swap through EVERY policy (including the drop-in lowest_index) and run the RAID-1 smoke test
#    under each, confirming each swap took effect first.
for p in $POLICIES; do
    echo "raid1: swapping to policy '$p'"
    echo "$p" > /tmp/raid_policy
    wait_active "$p"
    ./raid_smoke_test
    echo "raid1: smoke test passed under policy '$p'"
done

# 3. An UNKNOWN policy name must be rejected: the supervisor logs loudly and keeps the previous
#    policy serving I/O (a default is for an UNSET value, not an INVALID one).
prev=lowest_index    # the last policy activated by the loop above
echo "raid1: requesting an unknown policy; expecting it to be rejected"
echo "definitely_not_a_real_policy" > /tmp/raid_policy
# Give the supervisor several 100ms poll cycles to observe and reject the request.
i=0
while [ $i -lt 5 ]; do
    sleep 1
    i=$((i + 1))
done
active=$(cat /tmp/raid_policy_active 2>/dev/null || true)
if [ "$active" != "$prev" ]; then
    echo "raid1: FAILED unknown policy changed active from '$prev' to '$active'"
    exit 1
fi
echo "raid1: unknown policy correctly rejected; '$active' still active"
# I/O must still work under the retained policy.
./raid_smoke_test
echo "raid1: I/O still works after rejecting the unknown policy"

echo "All raid1 test passed"
