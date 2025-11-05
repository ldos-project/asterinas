#! /bin/bash

function do_run() {
    local TAG=$1
    local TYPE=$2
    local SERVER=$3
    make run INITARGS=/test/run_prefetch_microbench.sh RELEASE=1 KCMDARGS="page_cache.prefetcher_type=$TYPE page_cache.server_prefetcher=$SERVER"  |& tee results-$TAG.log
    egrep "cmd line|hit rate|starting with filename|real  .*m.*s" results-$TAG.log |& tee results-$TAG.out
}

TAG=$(date +"%m%d_%k%M%S")

# do_run "nn-$TAG" "server" "nn"
# do_run "regression-$TAG" "server" "regression"
# do_run "reactive-$TAG" "server" "reactive"
# do_run "none-$TAG" "none" "na"
do_run "builtin-$TAG" "builtin" "na"
# do_run "periodic-$TAG" "server" "periodic"
