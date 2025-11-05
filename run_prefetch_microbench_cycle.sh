#! /bin/bash
set -x

CSV_TMP=/tmp/csv.tmp
TAG=$(date +"%m%d_%k%M%S")

rm test/build/log.img

make run INITARGS=/test/run_prefetch_microbench.sh RELEASE=1

strings test/build/log.img > $CSV_TMP
sed '/current_thread_cpu_time/,$d' < $CSV_TMP > read_data_$TAG.csv
sed -n '/current_thread_cpu_time/,$p' < $CSV_TMP > cpu_data_$TAG.csv

rm $CSV_TMP