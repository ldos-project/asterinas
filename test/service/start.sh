#!/bin/sh

set -e

# Script to setup the initial processes in asterinas
sh /service/start_dropbear.sh
exec sh