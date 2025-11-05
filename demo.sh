make run INITARGS=/test/run_prefetch_microbench.sh RELEASE=1 KCMDARGS="prefetch_policy=readahead"
make run INITARGS=/test/run_prefetch_microbench.sh RELEASE=1 KCMDARGS="prefetch_policy=strided"
