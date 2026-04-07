#! /usr/bin/bash

for f in pmu pagefault rss connect; do
  sudo echo format $f
  sudo dd if=/dev/zero of="$f"_datacapture.img bs=2M count=512
done
