#!/bin/sh

set -e

# Script to start the dropbear ssh server
/usr/sbin/dropbear -p 10.0.2.15:22 &

echo "Dropbear started (PID $!)."
lsof -p $!