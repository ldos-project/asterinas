#!/bin/sh

set -e

# Script to start the dropbear ssh server
if [ -x /usr/sbin/dropbear ]; then
	/usr/sbin/dropbear -p 10.0.2.15:22 &
	echo "Dropbear started (PID $!)."
	lsof -p $!
else
	echo "WARNING: dropbear not found, should not use run_dropbear!!!"
fi

exec sh