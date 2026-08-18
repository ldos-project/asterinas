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
    local linux_result="" aster_result=""
    if [[ -s "${LINUX_OUTPUT}" ]]; then
        linux_result=$(awk "/${search_pattern}/ {print \$$result_index}" "${LINUX_OUTPUT}" | tr -d '\r' | sed 's/[^0-9.]*//g' | sed -n "${nth_occurrence}p")
    fi
    if [[ -s "${ASTER_OUTPUT}" ]]; then
        aster_result=$(awk "/${search_pattern}/ {print \$$result_index}" "${ASTER_OUTPUT}" | tr -d '\r' | sed 's/[^0-9.]*//g' | sed -n "${nth_occurrence}p")
    fi

    # Ensure at least one result is valid
    if [[ -z "${linux_result}" ]] && [[ -z "${aster_result}" ]]; then
        echo "Error: Failed to parse the results from the benchmark output" >&2
        exit 1
    fi

    # Write the results into the template
    yq --arg linux_result "${linux_result:-null}" --arg aster_result "${aster_result:-null}" \
        --arg capture_args "${BENCHMARK_CAPTURE_ARGS:-}" \
        '.capture_args = $capture_args |
         (.results[] | select(.extra == "linux_result") | .value) |= $linux_result |
         (.results[] | select(.extra == "aster_result") | .value) |= $aster_result' \
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
    yq -n --arg linux "$linux_legend" --arg aster "$asterinas_legend" --arg unit "$unit" '{
        "capture_args": "",
        "results": [
            { "name": $linux, "unit": $unit, "value": 0, "extra": "linux_result" },
            { "name": $aster, "unit": $unit, "value": 0, "extra": "aster_result" }
        ]
    }' > "${RESULT_TEMPLATE}"
}

# Extract the result file path based on benchmark location
extract_result_file() {
    local bench_result="$1"
    local relative_path="${bench_result#*/benchmark/}"
    local first_dir="${relative_path%%/*}"
    local filename=$(basename "$bench_result")

    # Handle different naming conventions for result files
    local base
    if [[ "$filename" == bench_* ]]; then
        local second_part=$(dirname "$bench_result" | awk -F"/benchmark/$first_dir/" '{print $2}' | cut -d'/' -f1)
        base="result_${first_dir}-${second_part}"
    else
        local result_file="result_${relative_path//\//-}"
        base="${result_file/.yaml/.json}"
        base="${base%.json}"
    fi

    local data_dir="${BENCHMARK_ROOT}/../data"
    mkdir -p "${data_dir}"
    echo "${data_dir}/${base}_${git_hash}.json"
}

save_capture_data() {
    local benchmark="$1"
    local seed="$2"
    local os_variant="$3"
    local capture_img="${BENCHMARK_ROOT}/../../build/capture.img"

    if [[ ! -f "${capture_img}" ]] || [[ $(stat -c %b "${capture_img}") -eq 0 ]]; then
        echo "No capture data written, skipping save."
        return 0
    fi

    local data_dir="${BENCHMARK_ROOT}/../data"
    mkdir -p "${data_dir}"

    local benchmark_short="${benchmark//\//-}"
    local timestamp=$(date +%Y%m%d-%H%M%S)
    local dest="${data_dir}/capture_${os_variant}_${benchmark_short}_${seed}_${timestamp}.img"
    cp --reflink=auto "${capture_img}" "${dest}"
    echo "Capture data saved: ${dest}"
}

save_benchmark_output() {
    local os_variant="$1"   # linux, asterinas, mariposa
    local benchmark="$2"

    local data_dir="${BENCHMARK_ROOT}/../data"
    mkdir -p "${data_dir}"

    local benchmark_short="${benchmark//\//-}"
    local timestamp=$(date +%Y%m%d-%H%M%S)
    local dest="${data_dir}/${os_variant}_${benchmark_short}_${git_hash}_${timestamp}.txt"

    local src
    if [[ "${os_variant}" == "linux" ]]; then
        src="${LINUX_OUTPUT}"
    else
        src="${ASTER_OUTPUT}"
    fi

    {
        echo "# benchmark: ${benchmark_short}"
        echo "# os: ${os_variant}"
        echo "# capture_args: ${BENCHMARK_CAPTURE_ARGS:-}"
        echo "# git: ${git_hash}"
        echo "# timestamp: $(date -Iseconds)"
        echo "# ---"
        cat "${src}"
    } > "${dest}"
    echo "${dest}"
}

# Run the specified benchmark with runtime configurations
run_benchmark() {
    local benchmark="$1"
    local run_mode="$2"
    local runtime_configs_str="$3" # String with key=value pairs, one per line
    local run_os="${4:-both}"

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

    # asterinas = baseline (no Mariposa features), mariposa = default (with OQueues/capture)
    # both_baseline = linux + baseline (for -o all pass 1)
    local baseline_flag=false
    if [[ "${run_os}" == "asterinas" || "${run_os}" == "both_baseline" ]]; then
        baseline_flag=true
    fi
    if ${baseline_flag}; then
        asterinas_cmd_arr+=(BASELINE_ASTERINAS=1)
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
                    if [[ "${run_os}" == "asterinas" || "${run_os}" == "mariposa" || "${run_os}" == "both" || "${run_os}" == "both_baseline" ]]; then
                        echo "Running benchmark ${benchmark} (seed ${seed}) on Asterinas..."
                        # KCMDARGS becomes --kcmd-args and is visible in /proc/cmdline
                        # inside the guest, where bench_runner.sh picks it up.
                        "${asterinas_cmd_arr[@]}" "KCMDARGS=BENCHMARK_DBBENCH_MIXGRAPH_SEEDS=${seed} ${BENCHMARK_CAPTURE_ARGS:-}" 2>&1 | tee -a "${ASTER_OUTPUT}"
                        save_capture_data "${benchmark}" "${seed}" "mariposa"
                        prepare_fs
                    fi
                    if [[ "${run_os}" == "linux" || "${run_os}" == "both" || "${run_os}" == "both_baseline" ]]; then
                        echo "Running benchmark ${benchmark} (seed ${seed}) on Linux..."
                        # The token rides on the kernel command line, visible in
                        # /proc/cmdline inside the guest.
                        "${linux_cmd_arr[@]}" -append "${linux_append} BENCHMARK_DBBENCH_MIXGRAPH_SEEDS=${seed}" 2>&1 | tee -a "${LINUX_OUTPUT}"
                    fi
                done
            else
                if [[ "${run_os}" == "asterinas" || "${run_os}" == "mariposa" || "${run_os}" == "both" || "${run_os}" == "both_baseline" ]]; then
                    echo "Running benchmark ${benchmark} on Asterinas..."
                    # Execute directly from array, redirect stderr to stdout, then tee
                    "${asterinas_cmd_arr[@]}" 2>&1 | tee "${ASTER_OUTPUT}"
                    save_capture_data "${benchmark}" "noseed" "mariposa"
                    prepare_fs
                fi
                if [[ "${run_os}" == "linux" || "${run_os}" == "both" || "${run_os}" == "both_baseline" ]]; then
                    echo "Running benchmark ${benchmark} on Linux..."
                    # Execute directly from array, redirect stderr to stdout, then tee
                    "${linux_cmd_arr[@]}" -append "${linux_append}" 2>&1 | tee "${LINUX_OUTPUT}"
                fi
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
                "${LINUX_OUTPUT}" \
                "${run_os}"
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
# When only one OS was run, the other's fields are set to null.
parse_multi_results() {
    local benchmark="$1"
    local bench_result="$2"
    local run_os="$3"
    local result_file="$(extract_result_file "$bench_result")"

    # Map run_os to the JSON key name for the non-linux guest OS.
    local guest_key="asterinas"
    case "${run_os}" in
        mariposa|both) guest_key="mariposa" ;;
        asterinas)     guest_key="asterinas" ;;
    esac

    local linux_runs aster_runs
    linux_runs=$(awk '/^SEED_RESULT /{print $2, $3}' "${LINUX_OUTPUT}" | tr -d '\r')
    aster_runs=$(awk '/^SEED_RESULT /{print $2, $3}' "${ASTER_OUTPUT}" | tr -d '\r')

    local have_linux=false have_asterinas=false
    [[ -n "${linux_runs}" ]] && have_linux=true
    [[ -n "${aster_runs}" ]] && have_asterinas=true

    if ! ${have_linux} && ! ${have_asterinas}; then
        echo "Error: No SEED_RESULT lines found in either benchmark output" >&2
        exit 1
    fi

    # Validate seed counts when both OSes are present
    if ${have_linux} && ${have_asterinas}; then
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
    fi

    local linux_fills aster_fills
    linux_fills=$(awk '/^SEED_FILL /{print $2, $3}' "${LINUX_OUTPUT}" | tr -d '\r')
    aster_fills=$(awk '/^SEED_FILL /{print $2, $3}' "${ASTER_OUTPUT}" | tr -d '\r')

    local linux_times aster_times
    linux_times=$(awk '/^SEED_TIME /{print $2, $3, $4, $5}' "${LINUX_OUTPUT}" | tr -d '\r')
    aster_times=$(awk '/^SEED_TIME /{print $2, $3, $4, $5}' "${ASTER_OUTPUT}" | tr -d '\r')

    # Per-seed latency histograms. Each seed emits one SEED_HIST headline line
    # plus SEED_HIST_BUCKET rows per operation type (write for fill; read,
    # write, seek for mix).
    local linux_hist_lines aster_hist_lines
    linux_hist_lines=$(grep '^SEED_HIST' "${LINUX_OUTPUT}" | tr -d '\r' || true)
    aster_hist_lines=$(grep '^SEED_HIST' "${ASTER_OUTPUT}" | tr -d '\r' || true)

    if ${have_linux} && [[ -z "${linux_hist_lines}" ]]; then
        echo "Error: No SEED_HIST lines found in Linux output" >&2
        exit 1
    fi
    if ${have_asterinas} && [[ -z "${aster_hist_lines}" ]]; then
        echo "Error: No SEED_HIST lines found in Asterinas output" >&2
        exit 1
    fi

    # Validate histogram block counts when both OSes are present
    if ${have_linux} && ${have_asterinas}; then
        local linux_hist_head aster_hist_head
        linux_hist_head=$(awk '/^SEED_HIST /{n++} END{print n+0}' "${LINUX_OUTPUT}")
        aster_hist_head=$(awk '/^SEED_HIST /{n++} END{print n+0}' "${ASTER_OUTPUT}")
        if [[ "${linux_hist_head}" -ne "${aster_hist_head}" ]]; then
            echo "Error: Mismatched number of histogram blocks (linux=${linux_hist_head}, asterinas=${aster_hist_head})" >&2
            exit 1
        fi
        local linux_count
        linux_count=$(wc -l <<< "${linux_runs}")
        if [[ "${linux_hist_head}" -ne $(( linux_count * 4 )) ]]; then
            echo "Warning: expected $(( linux_count * 4 )) histogram blocks per OS, got ${linux_hist_head} on linux" >&2
        fi
    fi

    local linux_hists_file aster_hists_file
    linux_hists_file=$(mktemp)
    aster_hists_file=$(mktemp)
    trap 'rm -f "${linux_hists_file}" "${aster_hists_file}"' RETURN
    histograms_from_lines <<< "${linux_hist_lines}" > "${linux_hists_file}"
    histograms_from_lines <<< "${aster_hist_lines}" > "${aster_hists_file}"

    # Build the paste pipeline and jq based on which OSes have data.
    # For single-OS mode, the missing OS fields are set to null.
    if ${have_linux} && ${have_asterinas}; then
        # Both OSes: original behavior
        paste -d ' ' \
            <(printf '%s\n' "${linux_runs}") \
            <(printf '%s\n' "${aster_runs}") \
            <(printf '%s\n' "${linux_fills}") \
            <(printf '%s\n' "${aster_fills}") \
            <(printf '%s\n' "${linux_times}") \
            <(printf '%s\n' "${aster_times}") \
            | jq -sR \
                --arg benchmark "${benchmark}" \
                --arg guest_os "${guest_key}" \
                --arg capture_args "${BENCHMARK_CAPTURE_ARGS:-}" \
                --slurpfile linux_hists "${linux_hists_file}" \
                --slurpfile aster_hists "${aster_hists_file}" '
                def median:
                    sort as $s
                    | (length / 2) as $mid
                    | if length % 2 == 1 then $s[$mid | floor] else (($s[$mid - 1] + $s[$mid]) / 2) end;
                def stats: {count: length, median: median, mean: (add / length), min: min, max: max};
                def safe_stats: map(select(. != null)) | if length > 0 then stats else null end;
                def safe_add: map(select(. != null)) | if length > 0 then add else null end;
                split("\n")[:-1]
                | map(split(" ") | {
                    seed: .[0],
                    linux: (.[1] | try tonumber catch null),
                    ($guest_os): (.[3] | try tonumber catch null),
                    fill: {
                        linux: (.[5] | try tonumber catch null),
                        ($guest_os): (.[7] | try tonumber catch null)
                    },
                    timing: {
                        linux: {fill_s: (.[9] | try tonumber catch null), mix_s: (.[10] | try tonumber catch null), total_s: (.[11] | try tonumber catch null)},
                        ($guest_os): {fill_s: (.[13] | try tonumber catch null), mix_s: (.[14] | try tonumber catch null), total_s: (.[15] | try tonumber catch null)}
                    }
                  })
                | map(. + { histogram: {
                        linux:     ($linux_hists[0][.seed] // null),
                        ($guest_os): ($aster_hists[0][.seed] // null)
                    } })
                | . as $runs
                | {
                    benchmark: $benchmark,
                    capture_args: $capture_args,
                    mode: "multi_seed",
                    runs: $runs,
                    summary: {
                        linux: ($runs | map(.linux) | safe_stats),
                        ($guest_os): ($runs | map(.[$guest_os]) | safe_stats)
                    },
                    timing_summary: {
                        linux: {
                            fill_s: ($runs | map(.timing.linux.fill_s) | safe_add),
                            mix_s: ($runs | map(.timing.linux.mix_s) | safe_add),
                            total_s: ($runs | map(.timing.linux.total_s) | safe_add)
                        },
                        ($guest_os): {
                            fill_s: ($runs | map(.timing[$guest_os].fill_s) | safe_add),
                            mix_s: ($runs | map(.timing[$guest_os].mix_s) | safe_add),
                            total_s: ($runs | map(.timing[$guest_os].total_s) | safe_add)
                        }
                    }
                  }' > "${result_file}"
    elif ${have_asterinas}; then
        # Guest OS only (asterinas or mariposa)
        printf '%s\n' "${aster_runs}" \
            | jq -sR \
                --arg benchmark "${benchmark}" \
                --arg guest_os "${guest_key}" \
                --arg capture_args "${BENCHMARK_CAPTURE_ARGS:-}" \
                --slurpfile aster_hists "${aster_hists_file}" '
                def median:
                    sort as $s
                    | (length / 2) as $mid
                    | if length % 2 == 1 then $s[$mid | floor] else (($s[$mid - 1] + $s[$mid]) / 2) end;
                def stats: {count: length, median: median, mean: (add / length), min: min, max: max};
                split("\n")[:-1]
                | map(split(" ") | {
                    seed: .[0],
                    ($guest_os): (.[1] | try tonumber catch null)
                  })
                | map(. + { histogram: {
                        linux: null,
                        ($guest_os): ($aster_hists[0][.seed] // null)
                    },
                    fill: { linux: null, ($guest_os): null },
                    timing: { linux: null, ($guest_os): null }
                  })
                | . as $runs
                | {
                    benchmark: $benchmark,
                    capture_args: $capture_args,
                    mode: "multi_seed",
                    runs: $runs,
                    summary: {
                        linux: null,
                        ($guest_os): ($runs | map(.[$guest_os]) | stats)
                    },
                    timing_summary: {
                        linux: null,
                        ($guest_os): null
                    }
                  }' > "${result_file}"
    else
        # Linux only
        printf '%s\n' "${linux_runs}" \
            | jq -sR \
                --arg benchmark "${benchmark}" \
                --arg capture_args "${BENCHMARK_CAPTURE_ARGS:-}" \
                --slurpfile linux_hists "${linux_hists_file}" '
                def median:
                    sort as $s
                    | (length / 2) as $mid
                    | if length % 2 == 1 then $s[$mid | floor] else (($s[$mid - 1] + $s[$mid]) / 2) end;
                def stats: {count: length, median: median, mean: (add / length), min: min, max: max};
                split("\n")[:-1]
                | map(split(" ") | {
                    seed: .[0],
                    linux: (.[1] | try tonumber catch null)
                  })
                | map(. + { histogram: {
                        linux: ($linux_hists[0][.seed] // null),
                        asterinas: null
                    },
                    fill: { linux: null, asterinas: null },
                    timing: { linux: null, asterinas: null }
                  })
                | . as $runs
                | {
                    benchmark: $benchmark,
                    capture_args: $capture_args,
                    mode: "multi_seed",
                    runs: $runs,
                    summary: {
                        linux: ($runs | map(.linux) | stats),
                        asterinas: null
                    },
                    timing_summary: {
                        linux: null,
                        asterinas: null
                    }
                  }' > "${result_file}"
    fi
    echo "Results written to ${result_file}"
}

# Parse per-seed results from three separate OS output files and combine into
# a single JSON with keys: linux, asterinas (baseline), mariposa.
# Arguments: benchmark linux_file baseline_file mariposa_file
parse_three_way_results() {
    local benchmark="$1"
    local linux_file="$2"
    local baseline_file="$3"
    local mariposa_file="$4"
    local result_file="$(extract_result_file "${BENCHMARK_ROOT}/${benchmark}/bench_result.yaml")"

    local linux_runs baseline_runs mariposa_runs
    linux_runs=$(awk '/^SEED_RESULT /{print $2, $3}' "${linux_file}" | tr -d '\r')
    baseline_runs=$(awk '/^SEED_RESULT /{print $2, $3}' "${baseline_file}" | tr -d '\r')
    mariposa_runs=$(awk '/^SEED_RESULT /{print $2, $3}' "${mariposa_file}" | tr -d '\r')

    if [[ -z "${linux_runs}" && -z "${baseline_runs}" && -z "${mariposa_runs}" ]]; then
        echo "Error: No SEED_RESULT lines found in any output" >&2
        exit 1
    fi

    local linux_fills baseline_fills mariposa_fills
    linux_fills=$(awk '/^SEED_FILL /{print $2, $3}' "${linux_file}" | tr -d '\r')
    baseline_fills=$(awk '/^SEED_FILL /{print $2, $3}' "${baseline_file}" | tr -d '\r')
    mariposa_fills=$(awk '/^SEED_FILL /{print $2, $3}' "${mariposa_file}" | tr -d '\r')

    local linux_times baseline_times mariposa_times
    linux_times=$(awk '/^SEED_TIME /{print $2, $3, $4, $5}' "${linux_file}" | tr -d '\r')
    baseline_times=$(awk '/^SEED_TIME /{print $2, $3, $4, $5}' "${baseline_file}" | tr -d '\r')
    mariposa_times=$(awk '/^SEED_TIME /{print $2, $3, $4, $5}' "${mariposa_file}" | tr -d '\r')

    local linux_hist_lines baseline_hist_lines mariposa_hist_lines
    linux_hist_lines=$(grep '^SEED_HIST' "${linux_file}" | tr -d '\r' || true)
    baseline_hist_lines=$(grep '^SEED_HIST' "${baseline_file}" | tr -d '\r' || true)
    mariposa_hist_lines=$(grep '^SEED_HIST' "${mariposa_file}" | tr -d '\r' || true)

    local linux_hists_file baseline_hists_file mariposa_hists_file
    linux_hists_file=$(mktemp)
    baseline_hists_file=$(mktemp)
    mariposa_hists_file=$(mktemp)
    trap 'rm -f "${linux_hists_file}" "${baseline_hists_file}" "${mariposa_hists_file}"' RETURN
    histograms_from_lines <<< "${linux_hist_lines}" > "${linux_hists_file}"
    histograms_from_lines <<< "${baseline_hist_lines}" > "${baseline_hists_file}"
    histograms_from_lines <<< "${mariposa_hist_lines}" > "${mariposa_hists_file}"

    paste -d ' ' \
        <(printf '%s\n' "${linux_runs}") \
        <(printf '%s\n' "${baseline_runs}") \
        <(printf '%s\n' "${mariposa_runs}") \
        <(printf '%s\n' "${linux_fills}") \
        <(printf '%s\n' "${baseline_fills}") \
        <(printf '%s\n' "${mariposa_fills}") \
        <(printf '%s\n' "${linux_times}") \
        <(printf '%s\n' "${baseline_times}") \
        <(printf '%s\n' "${mariposa_times}") \
        | jq -sR \
            --arg benchmark "${benchmark}" \
            --arg capture_args "${BENCHMARK_CAPTURE_ARGS:-}" \
            --slurpfile linux_hists "${linux_hists_file}" \
            --slurpfile baseline_hists "${baseline_hists_file}" \
            --slurpfile mariposa_hists "${mariposa_hists_file}" '
            def median:
                sort as $s
                | (length / 2) as $mid
                | if length % 2 == 1 then $s[$mid | floor] else (($s[$mid - 1] + $s[$mid]) / 2) end;
            def stats: {count: length, median: median, mean: (add / length), min: min, max: max};
            def safe_stats: map(select(. != null)) | if length > 0 then stats else null end;
            def safe_add: map(select(. != null)) | if length > 0 then add else null end;
            split("\n")[:-1]
            | map(split(" ") | {
                seed: .[0],
                linux: (.[1] | try tonumber catch null),
                asterinas: (.[3] | try tonumber catch null),
                mariposa: (.[5] | try tonumber catch null),
                fill: {
                    linux: (.[7] | try tonumber catch null),
                    asterinas: (.[9] | try tonumber catch null),
                    mariposa: (.[11] | try tonumber catch null)
                },
                timing: {
                    linux:    {fill_s: (.[13] | try tonumber catch null), mix_s: (.[14] | try tonumber catch null), total_s: (.[15] | try tonumber catch null)},
                    asterinas:{fill_s: (.[17] | try tonumber catch null), mix_s: (.[18] | try tonumber catch null), total_s: (.[19] | try tonumber catch null)},
                    mariposa: {fill_s: (.[21] | try tonumber catch null), mix_s: (.[22] | try tonumber catch null), total_s: (.[23] | try tonumber catch null)}
                }
              })
            | map(. + { histogram: {
                    linux:     ($linux_hists[0][.seed] // null),
                    asterinas: ($baseline_hists[0][.seed] // null),
                    mariposa:  ($mariposa_hists[0][.seed] // null)
                } })
            | . as $runs
            | {
                benchmark: $benchmark,
                capture_args: $capture_args,
                mode: "multi_seed",
                runs: $runs,
                summary: {
                    linux: ($runs | map(.linux) | safe_stats),
                    asterinas: ($runs | map(.asterinas) | safe_stats),
                    mariposa: ($runs | map(.mariposa) | safe_stats)
                },
                timing_summary: {
                    linux: {
                        fill_s: ($runs | map(.timing.linux.fill_s) | safe_add),
                        mix_s: ($runs | map(.timing.linux.mix_s) | safe_add),
                        total_s: ($runs | map(.timing.linux.total_s) | safe_add)
                    },
                    asterinas: {
                        fill_s: ($runs | map(.timing.asterinas.fill_s) | safe_add),
                        mix_s: ($runs | map(.timing.asterinas.mix_s) | safe_add),
                        total_s: ($runs | map(.timing.asterinas.total_s) | safe_add)
                    },
                    mariposa: {
                        fill_s: ($runs | map(.timing.mariposa.fill_s) | safe_add),
                        mix_s: ($runs | map(.timing.mariposa.mix_s) | safe_add),
                        total_s: ($runs | map(.timing.mariposa.total_s) | safe_add)
                    }
                }
              }' > "${result_file}"
    echo "Results written to ${result_file}"
}
cleanup() {
    echo "Cleaning up..."
    rm -f "${LINUX_OUTPUT}" "${ASTER_OUTPUT}" "${RESULT_TEMPLATE}"
}

# Main function to coordinate the benchmark run
main() {
    local parse_only=false
    local gc_nix=false
    local run_os="both"

    while getopts "cpo:" opt; do
        case ${opt} in
            c) gc_nix=true ;;
            p) parse_only=true ;;
            o)
                run_os="${OPTARG}"
                if [[ "${run_os}" != "linux" && "${run_os}" != "asterinas" && \
                      "${run_os}" != "mariposa" && "${run_os}" != "both" && \
                      "${run_os}" != "all" ]]; then
                    echo "Error: --os must be 'linux', 'asterinas', 'mariposa', 'both', or 'all'" >&2
                    exit 1
                fi
                ;;
            *) echo "Usage: $0 [-c] [-p] [-o linux|asterinas|mariposa|both|all] <benchmark> <platform>" >&2; exit 1 ;;
        esac
    done
    shift $((OPTIND - 1))

    local benchmark="$1"
    local platform="$2"

    # Compute git hash (short + dirty flag) for tagging outputs
    local git_hash
    git_hash=$(git -C "${BENCHMARK_ROOT}" rev-parse --short HEAD 2>/dev/null || echo "unknown")
    if ! git -C "${BENCHMARK_ROOT}" diff --quiet HEAD 2>/dev/null || \
       ! git -C "${BENCHMARK_ROOT}" diff --quiet --cached HEAD 2>/dev/null; then
        git_hash="${git_hash}-dirty"
    fi

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

    # -o all: run each OS variant separately, then combine into one result JSON
    if [[ "${run_os}" == "all" ]]; then
        if ${gc_nix} && ! ${parse_only}; then
            echo "Running nix garbage collection (before benchmarks)..."
            nix-collect-garbage -d
        fi

        echo "=== Pass 1/3: Linux ==="
        if ! ${parse_only}; then
            run_benchmark "$benchmark" "$run_mode" "$runtime_configs_str" "linux"
        fi
        if ${gc_nix} && ! ${parse_only}; then
            echo "Running nix garbage collection..."
            nix-collect-garbage -d
        fi
        local all_linux
        all_linux=$(save_benchmark_output "linux" "$benchmark")

        echo "=== Pass 2/3: Asterinas baseline ==="
        if ! ${parse_only}; then
            run_benchmark "$benchmark" "$run_mode" "$runtime_configs_str" "asterinas"
        fi
        if ${gc_nix} && ! ${parse_only}; then
            echo "Running nix garbage collection..."
            nix-collect-garbage -d
        fi
        local all_baseline
        all_baseline=$(save_benchmark_output "asterinas" "$benchmark")

        echo "=== Pass 3/3: Mariposa ==="
        if ! ${parse_only}; then
            run_benchmark "$benchmark" "$run_mode" "$runtime_configs_str" "mariposa"
        fi
        if ${gc_nix} && ! ${parse_only}; then
            echo "Running nix garbage collection (after benchmarks)..."
            nix-collect-garbage -d
        fi
        local all_mariposa
        all_mariposa=$(save_benchmark_output "mariposa" "$benchmark")

        parse_three_way_results "$benchmark" "${all_linux}" "${all_baseline}" "${all_mariposa}"

        echo "Three-way comparison completed: $(extract_result_file "$bench_result")"
        echo "Raw outputs preserved: ${all_linux}, ${all_baseline}, ${all_mariposa}"
        exit 0
    fi

    # Single-pass (linux, asterinas, mariposa, or both)
    if ${gc_nix} && ! ${parse_only}; then
        echo "Running nix garbage collection (before benchmark)..."
        nix-collect-garbage -d
    fi
    if ! ${parse_only}; then
        run_benchmark "$benchmark" "$run_mode" "$runtime_configs_str" "$run_os"
    fi

    # Save raw outputs to data/
    if ! ${parse_only}; then
        case "${run_os}" in
            linux)
                save_benchmark_output "linux" "$benchmark"
                ;;
            both)
                save_benchmark_output "linux" "$benchmark"
                save_benchmark_output "mariposa" "$benchmark"
                ;;
            asterinas|mariposa)
                save_benchmark_output "${run_os}" "$benchmark"
                ;;
        esac
    fi

    if ${gc_nix} && ! ${parse_only}; then
        echo "Running nix garbage collection (after benchmark)..."
        nix-collect-garbage -d
    fi

    # Parse results if benchmark configuration exists
    if [[ -f "$bench_result" ]]; then
        if [[ "$(yq -r '.multi_run // false' "$bench_result")" == "true" ]]; then
            parse_multi_results "$benchmark" "$bench_result" "$run_os"
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
