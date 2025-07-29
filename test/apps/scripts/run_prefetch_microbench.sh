#!/bin/sh

if test '!' -f /ext2/largefile; then
    dd if=/dev/zero of=/ext2/largefile bs=1G count=1
    sync
fi

COUNT=10

KB=1024
MB=$(( 1024 * 1024 ))
GB=$(( 1024 * 1024 * 1024 ))

/test/prefetch-microbench/stream -v /ext2/largefile 0,$KB,$((16 * $KB)),$COUNT,0 $((100 * $MB)),$KB,$KB,$COUNT,100
