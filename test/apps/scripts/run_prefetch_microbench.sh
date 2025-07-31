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

COUNT=$((8 * 1024))

KB=1024
MB=$(( 1024 * 1024 ))
GB=$(( 1024 * 1024 * 1024 ))

SLEEP=100

CMD="/test/prefetch-microbench/stream /ext2/largefile \
    0,$KB,$((1 * $KB)),$COUNT,$SLEEP \
    $((100 * $MB)),$KB,$((8 * $KB)),$COUNT,$SLEEP \
    $((200 * $MB)),$KB,$((16 * $KB)),$COUNT,$SLEEP \
    $((300 * $MB)),$KB,$((100 * $KB)),$COUNT,$SLEEP"

echo $CMD
command time $CMD
