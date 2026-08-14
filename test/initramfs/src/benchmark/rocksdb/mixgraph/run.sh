#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

set -e

# Why this workload: db_bench's mixgraph models a production Facebook
# social-graph workload (skewed key access, Pareto value sizes, mixed
# get/put/seek) as characterized in:
#   Cao, Dong, Vemuri, & Du. "Characterizing, Modeling, and Benchmarking
#   RocksDB Key-Value Workloads at Facebook." USENIX FAST 2020.
#   https://www.usenix.org/conference/fast20/presentation/cao
# The key space is pre-populated with fillrandom, exactly as that study's
# benchmark commands do. Phases are separate invocations so only mixgraph is
# time-boxed (--duration); the fill is count-driven (--num), identical on both
# OSes. --use_existing_db=true prevents db_bench from destroying the filled DB.
#
# Seeding: every seed produces an independent fillrandom + mixgraph pair
# against a freshly created DB. Both phases share the same seed so the mixgraph
# keys align with the filled DB. --histogram emits per-op latency percentiles.
# The seed list is controlled by the BENCHMARK_DBBENCH_MIXGRAPH_SEEDS
# environment variable (comma-separated, no spaces, since it may travel via the
# kernel command line); when unset it defaults to seeds.txt next to this script.
# bench_linux_and_aster.sh reboots the VM once per seed so every run starts
# from a freshly booted machine; a direct `make run_kernel` boot runs the whole
# list back to back instead.
echo "*** Running the RocksDB mixgraph benchmark ***"

if [ -n "${BENCHMARK_DBBENCH_MIXGRAPH_SEEDS}" ]; then
    SEEDS="$(echo "${BENCHMARK_DBBENCH_MIXGRAPH_SEEDS}" | tr ',' ' ')"
elif [ -f "$(dirname "$0")/seeds.txt" ]; then
    SEEDS="$(tr '\n' ' ' < "$(dirname "$0")/seeds.txt")"
else
    echo "Error: No seeds given (set BENCHMARK_DBBENCH_MIXGRAPH_SEEDS or provide seeds.txt)." >&2
    exit 1
fi

# Emit the latency histogram of one phase's transcript in a machine-parseable
# form: a SEED_HIST line with the headline stats followed by one
# SEED_HIST_BUCKET line per bucket row. Parses db_bench 9.10.0's
# HistogramStat::ToString() layout; the pct/cum bucket columns are omitted
# because they are derivable from the bucket counts and the headline Count.
emit_hist() {
    awk -v seed="$3" -v phase="$2" '
        /^Microseconds per / {
            op = $3
            sub(/:$/, "", op)
            in_block = 1
            next
        }
        in_block && /^Count: / {
            count = $2; avg = $4; stddev = $6
            next
        }
        in_block && /^Min: / {
            min = $2; median = $4; max = $6
            next
        }
        in_block && /^Percentiles: / {
            p50 = $3; p75 = $5; p99 = $7; p99_9 = $9; p99_99 = $11
            printf "SEED_HIST %s %s %s %s %s %s %s %s %s %s %s %s %s %s\n", \
                seed, phase, op, count, avg, stddev, min, median, max, p50, p75, p99, p99_9, p99_99
            next
        }
        in_block && /^[\[\(]/ {
            left = $2
            sub(/,/, "", left)
            printf "SEED_HIST_BUCKET %s %s %s %s %s %s\n", seed, phase, op, left, $3, $5
        }
    ' "$1"
}

for seed in ${SEEDS}; do
    # A fresh DB per seed keeps every run independent.
    rm -rf /tmp/db_bench_mixgraph

    fill_start=$(date +%s)
    /benchmark/bin/db_bench --benchmarks=fillrandom --num=1000000 --threads=4 \
        --seed="${seed}" --histogram --db=/tmp/db_bench_mixgraph > /tmp/fill.log 2>&1
    mix_start=$(date +%s)
    /benchmark/bin/db_bench --benchmarks=mixgraph --use_existing_db=true --num=1000000 \
        --duration=30 --threads=4 \
        --mix_get_ratio=0.83 --mix_put_ratio=0.14 --mix_seek_ratio=0.03 \
        --key_dist_a=0.002312 --key_dist_b=0.3467 \
        --seed="${seed}" --histogram --db=/tmp/db_bench_mixgraph > /tmp/mix.log 2>&1
    mix_end=$(date +%s)

    # Keep the full transcripts (including latency histograms) in the VM output.
    cat /tmp/fill.log /tmp/mix.log

    fill_ops=$(awk '/^fillrandom .*ops\/sec/{print $5; exit}' /tmp/fill.log)
    mix_ops=$(awk '/^mixgraph .*ops\/sec/{print $5; exit}' /tmp/mix.log)

    echo "SEED_FILL ${seed} ${fill_ops}"
    echo "SEED_RESULT ${seed} ${mix_ops}"
    echo "SEED_TIME ${seed} $((mix_start - fill_start)) $((mix_end - mix_start)) $((mix_end - fill_start))"

    emit_hist /tmp/fill.log fill "${seed}"
    emit_hist /tmp/mix.log mix "${seed}"
done
