#!/bin/bash
# SPDX-License-Identifier: MPL-2.0
#
# Script to collect sshd and all its dependencies for inclusion in initramfs

set -e

INITRAMFS_DIR="$1"
if [ -z "$INITRAMFS_DIR" ]; then
    echo "Usage: $0 <initramfs_dir>" >&2
    exit 1
fi

# Collect libraries from all SSH binaries
SSH_PREFIX="/root/asterinas/openssh/dropbear-2025.89"
SSH_BINARIES="$SSH_PREFIX/dropbear $SSH_PREFIX/dbclient $SSH_PREFIX/dbexec $SSH_PREFIX/dropbearkey $SSH_PREFIX/dropbearconvert $SSH_PREFIX/scp"

# Copy sshd binary
if [ -f $SSH_PREFIX/dropbear ]; then
    cp $SSH_PREFIX/dropbear "$INITRAMFS_DIR/usr/sbin/"
    echo "Copied dropbear binary"
fi

# Copy dropbearkey binary
if [ -f $SSH_PREFIX/dropbearkey ]; then
    cp $SSH_PREFIX/dropbearkey "$INITRAMFS_DIR/usr/bin/"
    echo "Copied dropbearkey binary"
fi

# Copy SCP
if [ -f $SSH_PREFIX/scp ]; then
    cp $SSH_PREFIX/scp "$INITRAMFS_DIR/bin/"
    echo "Copied scp binary"
fi

echo "SSH binaries and dependencies collected successfully"
