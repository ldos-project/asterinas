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

# Create necessary directories
mkdir -p "$INITRAMFS_DIR/usr/sbin"
mkdir -p "$INITRAMFS_DIR/usr/bin"
mkdir -p "$INITRAMFS_DIR/etc/ssh"
mkdir -p "$INITRAMFS_DIR/lib/x86_64-linux-gnu"
mkdir -p "$INITRAMFS_DIR/var/empty"

# Temporary file to collect all unique libraries
ALL_LIBS=$(mktemp)
trap "rm -f $ALL_LIBS" EXIT

# Function to collect libraries from a binary
collect_libs_from_binary() {
    local binary="$1"
    if [ ! -f "$binary" ]; then
        return
    fi
    
    # Collect libraries and append to file
    ldd "$binary" 2>/dev/null | awk '{print $3}' | grep -v '^$' | grep -v '^not' | while IFS= read -r lib; do
        if [ -n "$lib" ] && [ -f "$lib" ]; then
            echo "$lib" >> "$ALL_LIBS"
        fi
    done
}

# Collect libraries from all SSH binaries
SSH_BINARIES="/usr/sbin/sshd /usr/bin/ssh /usr/bin/scp /usr/bin/ssh-keygen /usr/bin/ssh-add"

for binary in $SSH_BINARIES; do
    if [ -f "$binary" ]; then
        collect_libs_from_binary "$binary"
    fi
done

# Copy all unique libraries
if [ -s "$ALL_LIBS" ]; then
    sort -u "$ALL_LIBS" | while IFS= read -r lib; do
        if [ -f "$lib" ]; then
            lib_rel_path="${lib#/}"
            lib_dest="$INITRAMFS_DIR/$lib_rel_path"
            mkdir -p "$(dirname "$lib_dest")"
            cp -L "$lib" "$lib_dest" 2>/dev/null || true
        fi
    done
fi

# Copy sshd binary
if [ -f /usr/sbin/sshd ]; then
    cp -L /usr/sbin/sshd "$INITRAMFS_DIR/usr/sbin/sshd"
    echo "Copied sshd binary"
fi

# Copy SSH client binaries (optional but useful)
for ssh_bin in /usr/bin/ssh /usr/bin/scp /usr/bin/ssh-keygen /usr/bin/ssh-add; do
    if [ -f "$ssh_bin" ]; then
        cp -L "$ssh_bin" "$INITRAMFS_DIR/usr/bin/$(basename "$ssh_bin")"
        echo "Copied $(basename "$ssh_bin")"
    fi
done

# Copy SSH configuration files
if [ -d /etc/ssh ]; then
    # Copy main config files
    for config in /etc/ssh/sshd_config /etc/ssh/ssh_config; do
        if [ -f "$config" ]; then
            cp "$config" "$INITRAMFS_DIR/etc/ssh/$(basename "$config")"
            echo "Copied $(basename "$config")"
        fi
    done
    
    # Configure sshd to bind to Asterinas network IP (10.0.2.15)
    # Asterinas configures network at kernel level, so we need to bind to specific IP
    SSHD_CONFIG="$INITRAMFS_DIR/etc/ssh/sshd_config"
    if [ -f "$SSHD_CONFIG" ]; then
        # Uncomment and set ListenAddress if it's commented, or add it if missing
        if grep -q "^#ListenAddress" "$SSHD_CONFIG" || grep -q "^ListenAddress" "$SSHD_CONFIG"; then
            # Replace existing ListenAddress line (commented or not)
            sed -i 's/^#*ListenAddress.*/ListenAddress 10.0.2.15/' "$SSHD_CONFIG"
        else
            # Add ListenAddress after Port directive or at the beginning
            if grep -q "^Port" "$SSHD_CONFIG"; then
                sed -i '/^Port/a ListenAddress 10.0.2.15' "$SSHD_CONFIG"
            else
                sed -i '1i ListenAddress 10.0.2.15' "$SSHD_CONFIG"
            fi
        fi
        echo "Configured sshd to listen on 10.0.2.15"
    fi
    
    # Copy host keys if they exist (they may need to be generated in the target system)
    for key in /etc/ssh/ssh_host_*_key*; do
        if [ -f "$key" ]; then
            cp "$key" "$INITRAMFS_DIR/etc/ssh/$(basename "$key")"
            echo "Copied host key: $(basename "$key")"
        fi
    done
fi

# Create empty directory for sshd (sshd needs /var/empty for privilege separation)
mkdir -p "$INITRAMFS_DIR/var/empty"

echo "SSH binaries and dependencies collected successfully"
