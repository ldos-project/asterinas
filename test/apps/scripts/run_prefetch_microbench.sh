#!/bin/sh

if test '!' -f /ext2/largefile; then
    echo "You need to create the testing file. You can do that with:"
    echo
    echo "dd if=/dev/zero of=/ext2/largefile bs=1G count=1; sync"
    echo
    echo "(If you are using logging, run this with logging OFF otherwise you will have "
    echo "immediate regret and a wall of messages. Once the file is created you can restart "
    echo "the VM and it will still be in the ext2 image.)"
    exit 1
fi

# Due to current Asterinas limitations, it seems that files greater than 1GB don't work.

COUNT=512

KB=1024
MB=$(( 1024 * 1024 ))
GB=$(( 1024 * 1024 * 1024 ))

/test/prefetch-microbench/stream -v /ext2/largefile \
    0,$KB,$((16 * $KB)),$COUNT,1000 \
    $((100 * $MB)),$KB,$KB,$COUNT,100000 \
    $((200 * $MB)),$KB,$((2 * KB)),$COUNT,100000 \
    $((300 * $MB)),$MB,$((1500 * KB)),$COUNT,1000000 \
