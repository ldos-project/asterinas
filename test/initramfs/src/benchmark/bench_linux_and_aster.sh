#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -e
set -o pipefail

# Ensure all dependencies are installed
if ! command -v yq >/dev/null 2>&1; then
    echo >&2 "Error: missing required tool: yq"
    exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
    echo >&2 "Error: missing required tool: jq"
    exit 1
fi

# Set up paths
BENCHMARK_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
source "${BENCHMARK_ROOT}/common/prepare_host.sh"
RESULT_TEMPLATE="${BENCHMARK_ROOT}/result_template.json"

# Parse benchmark results
parse_raw_results() {
    local search_pattern="$1"
    local nth_occurrence="$2"
    local result_index="$3"
    local result_file="$4"

    # Extract and sanitize numeric results
    local linux_result aster_result
    linux_result=$(awk "/${search_pattern}/ {print \$$result_index}" "${LINUX_OUTPUT}" | tr -d '\r' | sed 's/[^0-9.]*//g' | sed -n "${nth_occurrence}p")
    aster_result=$(awk "/${search_pattern}/ {print \$$result_index}" "${ASTER_OUTPUT}" | tr -d '\r' | sed 's/[^0-9.]*//g' | sed -n "${nth_occurrence}p")

    # Ensure both results are valid
    if [ -z "${linux_result}" ] || [ -z "${aster_result}" ]; then
        echo "Error: Failed to parse the results from the benchmark output" >&2
        exit 1
    fi

    # Write the results into the template
    yq --arg linux_result "${linux_result}" --arg aster_result "${aster_result}" \
        '(.[] | select(.extra == "linux_result") | .value) |= $linux_result |
         (.[] | select(.extra == "aster_result") | .value) |= $aster_result' \
        "${RESULT_TEMPLATE}" > "${result_file}"
    echo "Results written to ${result_file}"
}

# Generate a new result template based on unit and legend
generate_template() {
    local unit="$1"
    local legend="$2"

    # Replace placeholders with actual system names
    local linux_legend=${legend//"{system}"/"Linux"}
    local asterinas_legend=${legend//"{system}"/"Asterinas"}

    # Generate the result template JSON
    yq -n --arg linux "$linux_legend" --arg aster "$asterinas_legend" --arg unit "$unit" '[
        { "name": $linux, "unit": $unit, "value": 0, "extra": "linux_result" },
        { "name": $aster, "unit": $unit, "value": 0, "extra": "aster_result" }
    ]' > "${RESULT_TEMPLATE}"
}

# Extract the result file path based on benchmark location
extract_result_file() {
    local bench_result="$1"
    local relative_path="${bench_result#*/benchmark/}"
    local first_dir="${relative_path%%/*}"
    local filename=$(basename "$bench_result")

    # Handle different naming conventions for result files
    if [[ "$filename" == bench_* ]]; then
        local second_part=$(dirname "$bench_result" | awk -F"/benchmark/$first_dir/" '{print $2}' | cut -d'/' -f1)
        echo "result_${first_dir}-${second_part}.json"
    else
        local result_file="result_${relative_path//\//-}"
        echo "${result_file/.yaml/.json}"
    fi
}

# Run the specified benchmark with runtime configurations
run_benchmark() {
    local benchmark="$1"
    local run_mode="$2"
    local runtime_configs_str="$3" # String with key=value pairs, one per line

    echo "Preparing libraries..."
    prepare_libs
    prepare_ycsb

    # Default values
    local smp_val=1
    local mem_val="8G"
    local aster_scheme_cmd_part="SCHEME=iommu" # Default scheme

    # Process runtime_configs_str to override defaults and gather extra args
    while IFS='=' read -r key value; do
         if [[ -z "$key" ]]; then continue; fi # Skip empty lines/keys
         case "$key" in
             "smp")
                 smp_val="$value"
                 ;;
             "mem")
                 mem_val="$value"
                 ;;
             "aster_scheme")
                 if [[ "$value" == "null" ]]; then
                     aster_scheme_cmd_part="" # Remove default SCHEME=iommu
                 else
                     aster_scheme_cmd_part="SCHEME=${value}" # Override default
                 fi
                 ;;
             *)
                 echo "Warning: Unknown runtime configuration key '$key'" >&2
                 exit 1
                 ;;
         esac
     done <<< "$runtime_configs_str"

    # Prepare commands for Asterinas and Linux using arrays
    local asterinas_cmd_arr=(make run_kernel "BENCHMARK=${benchmark}")
    # Add scheme part only if it's not empty and the platform is not TDX (OSDK doesn't support multiple SCHEME)
    [[ -n "$aster_scheme_cmd_part" && "$platform" != "tdx" ]] && asterinas_cmd_arr+=("$aster_scheme_cmd_part")
    asterinas_cmd_arr+=(
        "SMP=${smp_val}"
        "MEM=${mem_val}"
        ENABLE_KVM=1
        RELEASE_LTO=1
        NETDEV=tap
        VHOST=on
    )
    if [[ "$platform" == "tdx" ]]; then
        asterinas_cmd_arr+=(INTEL_TDX=1)
    fi

    local linux_append="console=ttyS0 rdinit=/benchmark/common/bench_runner.sh ${benchmark} linux mitigations=off hugepages=0 transparent_hugepage=never quiet"

    local linux_cmd_arr=(
        qemu-system-x86_64
        --no-reboot
        -smp "${smp_val}"
        -m "${mem_val}"
        -machine q35,kernel-irqchip=split
        --enable-kvm
        -kernel "${LINUX_KERNEL}"
        -initrd "${BENCHMARK_ROOT}/../../build/initramfs.cpio.gz"
        -drive "if=none,format=raw,id=x0,file=${BENCHMARK_ROOT}/../../build/ext2.img"
        -device "virtio-blk-pci,bus=pcie.0,addr=0x6,drive=x0,serial=vext2,disable-legacy=on,disable-modern=off,queue-size=64,num-queues=1,request-merging=off,backend_defaults=off,discard=off,write-zeroes=off,event_idx=off,indirect_desc=off,queue_reset=off"
        -device "virtio-net-pci,netdev=net01,disable-legacy=on,disable-modern=off,csum=off,guest_csum=off,ctrl_guest_offloads=off,guest_tso4=off,guest_tso6=off,guest_ecn=off,guest_ufo=off,host_tso4=off,host_tso6=off,host_ecn=off,mrg_rxbuf=off,ctrl_vq=off,ctrl_rx=off,ctrl_vlan=off,ctrl_rx_extra=off,guest_announce=off,ctrl_mac_addr=off,host_ufo=off,guest_uso4=off,guest_uso6=off,host_uso=off"
        -netdev "tap,id=net01,script=${BENCHMARK_ROOT}/../../../../tools/net/qemu-ifup.sh,downscript=${BENCHMARK_ROOT}/../../../../tools/net/qemu-ifdown.sh,vhost=on"
        -nographic
    )
    if [[ "$platform" != "tdx" ]]; then
        linux_cmd_arr+=(
            -cpu Icelake-Server,-pcid,+x2apic
        )
    else
        linux_cmd_arr+=(
            -machine confidential-guest-support=tdx0
            -cpu host,-kvm-steal-time,pmu=off
            -bios /root/ovmf/release/OVMF.fd
            -nodefaults
            -serial stdio
            -object '{ "qom-type": "tdx-guest", "id": "tdx0", "sept-ve-disable": true, "quote-generation-socket": { "type": "vsock", "cid": "2", "port": "4050" } }'
        )
    fi

    # A benchmark that ships a seeds.txt file runs once per seed, rebooting the
    # VM in between so every run starts from a freshly booted machine state.
    # The list can be overridden with BENCHMARK_DBBENCH_MIXGRAPH_SEEDS
    # (comma-separated). The same list is passed to both OSes, one seed per boot.
    local seeds=""
    if [[ -f "${BENCHMARK_ROOT}/${benchmark}/seeds.txt" ]]; then
        if [[ -n "${BENCHMARK_DBBENCH_MIXGRAPH_SEEDS}" ]]; then
            seeds="${BENCHMARK_DBBENCH_MIXGRAPH_SEEDS//,/ }"
        else
            seeds=$(tr '\n' ' ' < "${BENCHMARK_ROOT}/${benchmark}/seeds.txt")
        fi
    fi

    # Run the benchmark depending on the mode
    case "${run_mode}" in
        "guest_only")
            if [[ -n "${seeds}" ]]; then
                # Start from empty output files; per-boot runs append to them.
                : > "${ASTER_OUTPUT}"
                : > "${LINUX_OUTPUT}"
                for seed in ${seeds}; do
                    echo "Running benchmark ${benchmark} (seed ${seed}) on Asterinas..."
                    # KCMDARGS becomes --kcmd-args and is visible in /proc/cmdline
                    # inside the guest, where bench_runner.sh picks it up.
                    "${asterinas_cmd_arr[@]}" "KCMDARGS=BENCHMARK_DBBENCH_MIXGRAPH_SEEDS=${seed}" 2>&1 | tee -a "${ASTER_OUTPUT}"
                    prepare_fs
                    echo "Running benchmark ${benchmark} (seed ${seed}) on Linux..."
                    # The token rides on the kernel command line, visible in
                    # /proc/cmdline inside the guest.
                    "${linux_cmd_arr[@]}" -append "${linux_append} BENCHMARK_DBBENCH_MIXGRAPH_SEEDS=${seed}" 2>&1 | tee -a "${LINUX_OUTPUT}"
                done
            else
                echo "Running benchmark ${benchmark} on Asterinas..."
                # Execute directly from array, redirect stderr to stdout, then tee
                "${asterinas_cmd_arr[@]}" 2>&1 | tee "${ASTER_OUTPUT}"
                prepare_fs
                echo "Running benchmark ${benchmark} on Linux..."
                # Execute directly from array, redirect stderr to stdout, then tee
                "${linux_cmd_arr[@]}" -append "${linux_append}" 2>&1 | tee "${LINUX_OUTPUT}"
            fi
            ;;
        "host_guest")
            # Note: host_guest_bench_runner.sh expects commands as single strings.
            # We need to reconstruct the string representation for compatibility.
            # Use printf %q to quote arguments safely.
            local asterinas_cmd_str
            printf -v asterinas_cmd_str '%q ' "${asterinas_cmd_arr[@]}"
            local linux_cmd_str
            printf -v linux_cmd_str '%q ' "${linux_cmd_arr[@]}" -append "${linux_append}"

            echo "Running benchmark ${benchmark} on host and guest..."
            bash "${BENCHMARK_ROOT}/common/host_guest_bench_runner.sh" \
                "${BENCHMARK_ROOT}/${benchmark}" \
                "${asterinas_cmd_str}" \
                "${linux_cmd_str}" \
                "${ASTER_OUTPUT}" \
                "${LINUX_OUTPUT}"
            ;;
        *)
            echo "Error: Unknown benchmark type '${run_mode}'" >&2
            exit 1
            ;;
    esac
}

# Parse the benchmark configuration
parse_results() {
    local bench_result="$1"

    local search_pattern=$(yq -r '.result_extraction.search_pattern // empty' "$bench_result")
    local nth_occurrence=$(yq -r '.result_extraction.nth_occurrence // 1' "$bench_result")
    local result_index=$(yq -r '.result_extraction.result_index // empty' "$bench_result")
    local unit=$(yq -r '.chart.unit // empty' "$bench_result")
    local legend=$(yq -r '.chart.legend // {system}' "$bench_result")

    generate_template "$unit" "$legend"
    parse_raw_results "$search_pattern" "$nth_occurrence" "$result_index" "$(extract_result_file "$bench_result")"
}

# Assemble the per-seed latency histograms of one OS from its SEED_HIST and
# SEED_HIST_BUCKET lines. Produces a JSON object keyed by seed whose values are
# nested {phase: {op: {headline stats, buckets: [...]}}} objects.
histograms_from_lines() {
    jq -sR '
        split("\n")[:-1]
        | map(select(length > 0) | split(" ")
            | if .[0] == "SEED_HIST"
              then { type: "head", seed: .[1], phase: .[2], op: .[3],
                     stats: { count: (.[4] | tonumber), avg: (.[5] | tonumber),
                              stddev: (.[6] | tonumber), min: (.[7] | tonumber),
                              median: (.[8] | tonumber), max: (.[9] | tonumber),
                              p50: (.[10] | tonumber), p75: (.[11] | tonumber),
                              p99: (.[12] | tonumber), p99_9: (.[13] | tonumber),
                              p99_99: (.[14] | tonumber) } }
              else { type: "bucket", seed: .[1], phase: .[2], op: .[3],
                     bucket: { left: (.[4] | tonumber), right: (.[5] | tonumber),
                               count: (.[6] | tonumber) } }
              end)
        | group_by(.seed)
        | map({ key: .[0].seed,
                value: (reduce .[] as $r ({}; . as $acc
              | if $r.type == "head"
                then $acc | .[$r.phase][$r.op] = ($r.stats + { buckets: [] })
                else $acc | .[$r.phase][$r.op].buckets += [$r.bucket]
                end)) })
        | from_entries
    '
}

# Parse per-seed results from the raw outputs of a multi-seed benchmark run.
# The benchmark emits one SEED_RESULT / SEED_FILL / SEED_TIME line per seed;
# Linux and Asterinas must agree on both the seed count and their order.
parse_multi_results() {
    local benchmark="$1"
    local bench_result="$2"
    local result_file="$(extract_result_file "$bench_result")"

    local linux_runs aster_runs
    linux_runs=$(awk '/^SEED_RESULT /{print $2, $3}' "${LINUX_OUTPUT}" | tr -d '\r')
    aster_runs=$(awk '/^SEED_RESULT /{print $2, $3}' "${ASTER_OUTPUT}" | tr -d '\r')

    if [[ -z "${linux_runs}" || -z "${aster_runs}" ]]; then
        echo "Error: No SEED_RESULT lines found in the benchmark output" >&2
        exit 1
    fi

    local linux_count aster_count
    linux_count=$(wc -l <<< "${linux_runs}")
    aster_count=$(wc -l <<< "${aster_runs}")
    if [[ "${linux_count}" -ne "${aster_count}" ]]; then
        echo "Error: Mismatched number of seeds (linux=${linux_count}, asterinas=${aster_count})" >&2
        exit 1
    fi
    if [[ -f "${BENCHMARK_ROOT}/${benchmark}/seeds.txt" ]]; then
        local expected_count
        expected_count=$(wc -l < "${BENCHMARK_ROOT}/${benchmark}/seeds.txt")
        if [[ "${linux_count}" -ne "${expected_count}" ]]; then
            echo "Warning: expected ${expected_count} seeds, got ${linux_count}" >&2
        fi
    fi

    local linux_fills aster_fills
    linux_fills=$(awk '/^SEED_FILL /{print $2, $3}' "${LINUX_OUTPUT}" | tr -d '\r')
    aster_fills=$(awk '/^SEED_FILL /{print $2, $3}' "${ASTER_OUTPUT}" | tr -d '\r')

    local linux_times aster_times
    linux_times=$(awk '/^SEED_TIME /{print $2, $3, $4, $5}' "${LINUX_OUTPUT}" | tr -d '\r')
    aster_times=$(awk '/^SEED_TIME /{print $2, $3, $4, $5}' "${ASTER_OUTPUT}" | tr -d '\r')

    # Per-seed latency histograms. Each seed emits one SEED_HIST headline line
    # plus SEED_HIST_BUCKET rows per operation type (write for fill; read,
    # write, seek for mix). Both OSes must agree on the block count.
    local linux_hist_lines aster_hist_lines
    linux_hist_lines=$(grep '^SEED_HIST' "${LINUX_OUTPUT}" | tr -d '\r' || true)
    aster_hist_lines=$(grep '^SEED_HIST' "${ASTER_OUTPUT}" | tr -d '\r' || true)

    if [[ -z "${linux_hist_lines}" || -z "${aster_hist_lines}" ]]; then
        echo "Error: No SEED_HIST lines found in the benchmark output" >&2
        exit 1
    fi

    local linux_hist_head aster_hist_head
    linux_hist_head=$(awk '/^SEED_HIST /{n++} END{print n+0}' "${LINUX_OUTPUT}")
    aster_hist_head=$(awk '/^SEED_HIST /{n++} END{print n+0}' "${ASTER_OUTPUT}")
    if [[ "${linux_hist_head}" -ne "${aster_hist_head}" ]]; then
        echo "Error: Mismatched number of histogram blocks (linux=${linux_hist_head}, asterinas=${aster_hist_head})" >&2
        exit 1
    fi
    if [[ "${linux_hist_head}" -ne $(( linux_count * 4 )) ]]; then
        echo "Warning: expected $(( linux_count * 4 )) histogram blocks per OS, got ${linux_hist_head} on linux" >&2
    fi

    local linux_hists aster_hists
    linux_hists=$(histograms_from_lines <<< "${linux_hist_lines}")
    aster_hists=$(histograms_from_lines <<< "${aster_hist_lines}")

    # All per-seed streams follow the same seed order as the results.
    paste -d ' ' \
        <(printf '%s\n' "${linux_runs}") \
        <(printf '%s\n' "${aster_runs}") \
        <(printf '%s\n' "${linux_fills}") \
        <(printf '%s\n' "${aster_fills}") \
        <(printf '%s\n' "${linux_times}") \
        <(printf '%s\n' "${aster_times}") \
        | jq -sR \
            --arg benchmark "${benchmark}" \
            --argjson linux_hists "${linux_hists}" \
            --argjson aster_hists "${aster_hists}" '
            def median:
                sort as $s
                | (length / 2) as $mid
                | if length % 2 == 1 then $s[$mid | floor] else (($s[$mid - 1] + $s[$mid]) / 2) end;
            def stats: {count: length, median: median, mean: (add / length), min: min, max: max};
            split("\n")[:-1]
            | map(split(" ") | {
                seed: .[0],
                linux: (.[1] | tonumber),
                asterinas: (.[3] | tonumber),
                fill: {
                    linux: (.[5] | tonumber),
                    asterinas: (.[7] | tonumber)
                },
                timing: {
                    linux: {fill_s: (.[9] | tonumber), mix_s: (.[10] | tonumber), total_s: (.[11] | tonumber)},
                    asterinas: {fill_s: (.[13] | tonumber), mix_s: (.[14] | tonumber), total_s: (.[15] | tonumber)}
                }
              })
            | map(. + { histogram: {
                    linux:     ($linux_hists[.seed] // null),
                    asterinas: ($aster_hists[.seed] // null)
                } })
            | . as $runs
            | {
                benchmark: $benchmark,
                mode: "multi_seed",
                runs: $runs,
                summary: {
                    linux: ($runs | map(.linux) | stats),
                    asterinas: ($runs | map(.asterinas) | stats)
                },
                timing_summary: {
                    linux: {
                        fill_s: ($runs | map(.timing.linux.fill_s) | add),
                        mix_s: ($runs | map(.timing.linux.mix_s) | add),
                        total_s: ($runs | map(.timing.linux.total_s) | add)
                    },
                    asterinas: {
                        fill_s: ($runs | map(.timing.asterinas.fill_s) | add),
                        mix_s: ($runs | map(.timing.asterinas.mix_s) | add),
                        total_s: ($runs | map(.timing.asterinas.total_s) | add)
                    }
                }
              }' > "${result_file}"
    echo "Results written to ${result_file}"
}

# Clean up temporary files
cleanup() {
    echo "Cleaning up..."
    rm -f "${LINUX_OUTPUT}" "${ASTER_OUTPUT}" "${RESULT_TEMPLATE}"
}

# Main function to coordinate the benchmark run
main() {
    local benchmark="$1"
    local platform="$2"

    if [[ -z "${BENCHMARK_ROOT}/${benchmark}" ]]; then
        echo "Error: No benchmark specified" >&2
        exit 1
    fi
    echo "Running benchmark $benchmark..."

    # Determine the run mode (host-only or host-guest)
    local run_mode="guest_only"
    [[ -f "${BENCHMARK_ROOT}/${benchmark}/host.sh" ]] && run_mode="host_guest"

    local bench_result="${BENCHMARK_ROOT}/${benchmark}/bench_result.yaml"
    local runtime_configs_str=""

    # Try reading from single result file first
    if [[ -f "$bench_result" ]]; then
        # Read runtime_config object, convert to key=value lines, ensuring value is string
        runtime_configs_str=$(yq -r '(.runtime_config // {}) | to_entries | .[] | .key + "=" + (.value | tostring)' "$bench_result")
    else
        # If not found, try reading from the first file in bench_results/ that has a non-empty runtime_config
        for job_yaml in "${BENCHMARK_ROOT}/${benchmark}"/bench_results/*; do
            if [[ -f "$job_yaml" ]]; then
                echo "Reading runtime configurations from $job_yaml..."
                # Read runtime_config object, convert to key=value lines, ensuring value is string
                runtime_configs_str=$(yq -r '(.runtime_config // {}) | to_entries | .[] | .key + "=" + (.value | tostring)' "$job_yaml")
                # Check if runtime_config was actually found and non-empty
                if [[ -n "$runtime_configs_str" ]]; then
                    break # Found it, stop looking
                fi
            fi
        done
    fi

    # Run the benchmark, passing the config string
    run_benchmark "$benchmark" "$run_mode" "$runtime_configs_str"

    # Parse results if benchmark configuration exists
    if [[ -f "$bench_result" ]]; then
        if [[ "$(yq -r '.multi_run // false' "$bench_result")" == "true" ]]; then
            parse_multi_results "$benchmark" "$bench_result"
        else
            parse_results "$bench_result"
        fi
    else
        for job in "${BENCHMARK_ROOT}/${benchmark}"/bench_results/*; do
            [[ -f "$job" ]] && parse_results "$job"
        done
    fi

    # Cleanup temporary files
    cleanup
    echo "Benchmark completed successfully."
}

main "$@"
