#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

# This script is used to generate QEMU arguments for OSDK.
# Usage: `qemu_args.sh [scheme]`
#  - scheme: "normal", "test", "microvm" or "iommu";
# Other arguments are configured via environmental variables:
#  - OVMF: "on" or "off";
#  - BOOT_METHOD: "qemu-direct", "grub-rescue-iso" or "grub-qcow2";
#  - BOOT_PROTOCOL: "multiboot", "multiboot2", "linux-legacy32", "linux-efi-pe64" or "linux-efi-handover64";
#  - NETDEV: "user" or "tap";
#  - VHOST: "off" or "on";
#  - VSOCK: "off" or "on";
#  - CONSOLE: "hvc0" to enable virtio console;
#  - SMP: number of CPUs;
#  - MEM: amount of memory, e.g. "8G";
#  - VNC_PORT: VNC port, default is "42";
#  - RAID_DEVICES: comma-separated list of three block devices for RAID, e.g.
#    "/dev/sda,/dev/sdb,/dev/sdc"; if unset or invalid, image files are used.

OVMF=${OVMF:-"on"}
VHOST=${VHOST:-"off"}
VSOCK=${VSOCK:-"off"}
NETDEV=${NETDEV:-"user"}
CONSOLE=${CONSOLE:-"hvc0"}
# Configure RAID drive sources. Set RAID_DEVICES to a comma-separated list of
# exactly three existing block devices (e.g.
# RAID_DEVICES=/dev/nvme0n1p1,/dev/nvme1n1p1,/dev/nvme2n1p1) to pass them directly to the guest.
# If this variable is unset, fallback to use three disk images. 
# Else if any of the path is invalid, will throw an error and not launching QEMU. 
if [ -n "${RAID_DEVICES:-}" ]; then
    IFS=',' read -r RAID_DEV_0 RAID_DEV_1 RAID_DEV_2 RAID_DEV_EXTRA <<< "$RAID_DEVICES"
    if [ -z "$RAID_DEV_0" ] || [ -z "$RAID_DEV_1" ] || [ -z "$RAID_DEV_2" ] || [ -n "$RAID_DEV_EXTRA" ]; then
        echo "[$1] Error: RAID_DEVICES='$RAID_DEVICES' must be exactly three comma-separated paths." 1>&2
        exit 1
    fi
    for RAID_DEV in "$RAID_DEV_0" "$RAID_DEV_1" "$RAID_DEV_2"; do
        if [ ! -e "$RAID_DEV" ]; then
            echo "[$1] Error: RAID member device '$RAID_DEV' does not exist." 1>&2
            exit 1
        fi
    done
    RAID_CACHE="directsync"
else
    echo "[$1] No RAID_DEVICES specified, using disk images instead." 1>&2
    RAID_IMG_DIR="./test/initramfs/build"
    mkdir -p "$RAID_IMG_DIR"
    RAID_DEV_0="$RAID_IMG_DIR/raid0.img"
    if [ ! -f "$RAID_DEV_0" ]; then
        echo "[$1] Creating 1GB ext2 image $RAID_DEV_0" 1>&2
        truncate -s 1G "$RAID_DEV_0"
        mkfs.ext2 -q -F "$RAID_DEV_0"
    fi
    RAID_DEV_1="$RAID_IMG_DIR/raid1.img"
    RAID_DEV_2="$RAID_IMG_DIR/raid2.img"
    for RAID_IMG in "$RAID_DEV_1" "$RAID_DEV_2"; do
        if [ ! -f "$RAID_IMG" ]; then
            echo "[$1] Copying $RAID_DEV_0 to $RAID_IMG" 1>&2
            cp "$RAID_DEV_0" "$RAID_IMG"
        fi
    done
    RAID_CACHE="writeback"
fi

SSH_RAND_PORT=${SSH_PORT:-22}
NGINX_RAND_PORT=${NGINX_PORT:-8080}
REDIS_RAND_PORT=${REDIS_PORT:-6379}
IPERF_RAND_PORT=${IPERF_PORT:-5201}
LMBENCH_TCP_LAT_RAND_PORT=${LMBENCH_TCP_LAT_PORT:-31234}
LMBENCH_TCP_BW_RAND_PORT=${LMBENCH_TCP_BW_PORT:-31236}
MEMCACHED_RAND_PORT=${MEMCACHED_PORT:-11211}

# Optional QEMU arguments. Opt in them manually if needed.
# QEMU_OPT_ARG_DUMP_PACKETS="-object filter-dump,id=filter0,netdev=net01,file=virtio-net.pcap"

if [ "$NETDEV" = "user" ]; then
    echo "[$1] Forwarded QEMU guest port: $SSH_RAND_PORT->22; $NGINX_RAND_PORT->8080 $REDIS_RAND_PORT->6379 $IPERF_RAND_PORT->5201 $LMBENCH_TCP_LAT_RAND_PORT->31234 $LMBENCH_TCP_BW_RAND_PORT->31236 $MEMCACHED_RAND_PORT->11211" 1>&2
    NETDEV_ARGS="-netdev user,id=net01,hostfwd=tcp::$SSH_RAND_PORT-:22,hostfwd=tcp::$NGINX_RAND_PORT-:8080,hostfwd=tcp::$REDIS_RAND_PORT-:6379,hostfwd=tcp::$IPERF_RAND_PORT-:5201,hostfwd=tcp::$LMBENCH_TCP_LAT_RAND_PORT-:31234,hostfwd=tcp::$LMBENCH_TCP_BW_RAND_PORT-:31236,hostfwd=tcp::$MEMCACHED_RAND_PORT-:11211"
    VIRTIO_NET_FEATURES=",mrg_rxbuf=off,ctrl_rx=off,ctrl_rx_extra=off,ctrl_vlan=off,ctrl_vq=off,ctrl_guest_offloads=off,ctrl_mac_addr=off,event_idx=off,queue_reset=off,guest_announce=off,indirect_desc=off"
elif [ "$NETDEV" = "tap" ]; then
    THIS_SCRIPT_DIR=$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )
    QEMU_IFUP_SCRIPT_PATH=$THIS_SCRIPT_DIR/net/qemu-ifup.sh
    QEMU_IFDOWN_SCRIPT_PATH=$THIS_SCRIPT_DIR/net/qemu-ifdown.sh
    NETDEV_ARGS="-netdev tap,id=net01,script=$QEMU_IFUP_SCRIPT_PATH,downscript=$QEMU_IFDOWN_SCRIPT_PATH,vhost=$VHOST"
    VIRTIO_NET_FEATURES=",csum=off,guest_csum=off,ctrl_guest_offloads=off,guest_tso4=off,guest_tso6=off,guest_ecn=off,guest_ufo=off,host_tso4=off,host_tso6=off,host_ecn=off,host_ufo=off,mrg_rxbuf=off,ctrl_vq=off,ctrl_rx=off,ctrl_vlan=off,ctrl_rx_extra=off,guest_announce=off,ctrl_mac_addr=off,host_ufo=off,guest_uso4=off,guest_uso6=off,host_uso=off"
else 
    echo "Invalid netdev" 1>&2
    NETDEV_ARGS="-nic none"
fi

if [ "$CONSOLE" = "hvc0" ]; then
    # Kernel logs are printed to all consoles. Redirect serial output to a file to avoid duplicate logs.
    CONSOLE_ARGS="-device virtconsole,chardev=mux -serial file:qemu-serial.log"
else
    CONSOLE_ARGS="-serial chardev:mux"
fi

if [ "$1" = "riscv" ]; then
    # NOTE: The `/etc/profile.d/init.sh` assumes that `ext2.img` appears as the first block device (`/dev/vda`).
    # The ordering below ensures `x1` (ext2.img) is discovered before `x0`, maintaining this assumption.
    # TODO: Once UUID-based mounting is implemented, this strict ordering will no longer be required.
    QEMU_ARGS="\
        -cpu rv64,svpbmt=true \
        -machine virt \
        -m ${MEM-:8G} \
        -smp ${SMP-:1} \
        --no-reboot \
        -nographic \
        -display none \
        -monitor chardev:mux \
        -chardev stdio,id=mux,mux=on,signal=off,logfile=qemu.log \
        -drive if=none,format=raw,id=x0,file=./test/initramfs/build/ext2.img \
        -drive if=none,format=raw,id=x1,file=./test/initramfs/build/exfat.img \
        -drive if=none,format=raw,id=d0,file=./test/initramfs/build/capture.img \
        -drive if=none,format=raw,id=d1,file=./test/initramfs/build/capture_legacy.img \
        -drive if=none,format=raw,id=r0,file=$RAID_DEV_0,cache=$RAID_CACHE \
        -drive if=none,format=raw,id=r1,file=$RAID_DEV_1,cache=$RAID_CACHE \
        -drive if=none,format=raw,id=r2,file=$RAID_DEV_2,cache=$RAID_CACHE \
        -device virtio-blk-device,drive=x1 \
        -device virtio-blk-device,drive=x0 \
        -device virtio-keyboard-device \
        -device virtio-serial-device \
        $CONSOLE_ARGS \
    "
    echo $QEMU_ARGS
    exit 0
fi

if [ "$1" = "tdx" ]; then
    TDX_OBJECT='{ "qom-type": "tdx-guest", "id": "tdx0", "sept-ve-disable": true, "quote-generation-socket": { "type": "vsock", "cid": "2", "port": "4050" } }'

    QEMU_ARGS="\
        -m ${MEM:-8G} \
        -smp ${SMP:-1} \
        -vga none \
        -nographic \
        -monitor pty \
        -nodefaults \
        -bios /root/ovmf/release/OVMF.fd \
        -cpu host,-kvm-steal-time,pmu=off \
        -machine q35,kernel-irqchip=split,confidential-guest-support=tdx0 \
        -object '$TDX_OBJECT' \
        -device virtio-net-pci,netdev=net01,disable-legacy=on,disable-modern=off$VIRTIO_NET_FEATURES \
        -device virtio-keyboard-pci,disable-legacy=on,disable-modern=off \
        $NETDEV_ARGS \
        $QEMU_OPT_ARG_DUMP_PACKETS \
        -chardev stdio,id=mux,mux=on,logfile=qemu.log \
        -device virtio-serial,romfile= \
        $CONSOLE_ARGS \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -monitor chardev:mux \
        -d guest_errors \
    "
    echo $QEMU_ARGS
    exit 0
fi

COMMON_QEMU_ARGS="\
    -cpu Icelake-Server,+x2apic,pmu=on \
    -smp ${SMP:-1} \
    -m ${MEM:-8G} \
    --no-reboot \
    -nographic \
    -display vnc=0.0.0.0:${VNC_PORT:-42} \
    -monitor chardev:mux \
    -chardev stdio,id=mux,mux=on,signal=off,logfile=qemu.log \
    $NETDEV_ARGS \
    $QEMU_OPT_ARG_DUMP_PACKETS \
    -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
    -drive if=none,format=raw,id=x0,file=./test/initramfs/build/ext2.img \
    -drive if=none,format=raw,id=x1,file=./test/initramfs/build/exfat.img \
    -drive if=none,format=raw,id=d0,file=./test/initramfs/build/capture.img \
    -drive if=none,format=raw,id=d1,file=./test/initramfs/build/capture_legacy.img \
    -drive if=none,format=raw,id=r0,file=$RAID_DEV_0,cache=$RAID_CACHE \
    -drive if=none,format=raw,id=r1,file=$RAID_DEV_1,cache=$RAID_CACHE \
    -drive if=none,format=raw,id=r2,file=$RAID_DEV_2,cache=$RAID_CACHE \
"

if [ "$1" = "iommu" ]; then
    if [ "$OVMF" = "off" ]; then
        echo "Warning: OVMF is off, enabling it for IOMMU support." 1>&2
        OVMF="on"
    fi
    IOMMU_DEV_EXTRA=",iommu_platform=on,ats=on"
    IOMMU_EXTRA_ARGS="\
        -device intel-iommu,intremap=on,device-iotlb=on \
        -device ioh3420,id=pcie.0,chassis=1 \
    "
    # TODO: Add support for enabling IOMMU on AMD platforms
fi

if [ "$1" = "microvm" ]; then
    QEMU_ARGS="\
        $COMMON_QEMU_ARGS \
        -machine microvm,rtc=on \
        -nodefaults \
        -no-user-config \
        -device virtio-blk-device,drive=x0,serial=vext2 \
        -device virtio-blk-device,drive=x1,serial=vexfat \
        -device virtio-blk-device,drive=r0,serial=raid0 \
        -device virtio-blk-device,drive=r1,serial=raid1 \
        -device virtio-blk-device,drive=d0,serial=capture \
        -device virtio-blk-device,drive=d1,serial=capture_legacy \
        -device virtio-keyboard-device \
        -device virtio-net-device,netdev=net01 \
        -device virtio-serial-device \
        $CONSOLE_ARGS \
    "
else
    QEMU_ARGS="\
        $COMMON_QEMU_ARGS \
        -machine q35,kernel-irqchip=split \
        -device virtio-blk-pci,bus=pcie.0,addr=0x6,drive=x0,serial=vext2,disable-legacy=on,disable-modern=off,queue-size=64,num-queues=1,request-merging=off,backend_defaults=off,discard=off,write-zeroes=off,event_idx=off,indirect_desc=off,queue_reset=off$IOMMU_DEV_EXTRA \
        -device virtio-blk-pci,bus=pcie.0,addr=0x7,drive=x1,serial=vexfat,disable-legacy=on,disable-modern=off,queue-size=64,num-queues=1,request-merging=off,backend_defaults=off,discard=off,write-zeroes=off,event_idx=off,indirect_desc=off,queue_reset=off$IOMMU_DEV_EXTRA \
        -device virtio-blk-pci,bus=pcie.0,addr=0x8,drive=r0,serial=raid0,disable-legacy=on,disable-modern=off,queue-size=64,num-queues=1,request-merging=off,backend_defaults=off,discard=off,write-zeroes=off,event_idx=off,indirect_desc=off,queue_reset=off$IOMMU_DEV_EXTRA \
        -device virtio-blk-pci,bus=pcie.0,addr=0x9,drive=r1,serial=raid1,disable-legacy=on,disable-modern=off,queue-size=64,num-queues=1,request-merging=off,backend_defaults=off,discard=off,write-zeroes=off,event_idx=off,indirect_desc=off,queue_reset=off$IOMMU_DEV_EXTRA \
        -device virtio-blk-pci,bus=pcie.0,addr=0xa,drive=r2,serial=raid2,disable-legacy=on,disable-modern=off,queue-size=64,num-queues=1,request-merging=off,backend_defaults=off,discard=off,write-zeroes=off,event_idx=off,indirect_desc=off,queue_reset=off$IOMMU_DEV_EXTRA \
        -device virtio-blk-pci,bus=pcie.0,addr=0xb,drive=d0,serial=capture,disable-legacy=on,disable-modern=off,queue-size=64,num-queues=1,request-merging=off,backend_defaults=off,discard=off,write-zeroes=off,event_idx=off,indirect_desc=off,queue_reset=off$IOMMU_DEV_EXTRA \
        -device virtio-blk-pci,bus=pcie.0,addr=0xc,drive=d1,serial=capture_legacy,disable-legacy=on,disable-modern=off,queue-size=64,num-queues=1,request-merging=off,backend_defaults=off,discard=off,write-zeroes=off,event_idx=off,indirect_desc=off,queue_reset=off$IOMMU_DEV_EXTRA \
        -object rng-random,id=rng0,filename=/dev/urandom \
        -device virtio-rng-pci,bus=pcie.0,addr=0xd,disable-legacy=on,disable-modern=off,rng=rng0,event_idx=off,indirect_desc=off,queue_reset=off$IOMMU_DEV_EXTRA \
        -device virtio-net-pci,netdev=net01,disable-legacy=on,disable-modern=off$VIRTIO_NET_FEATURES$IOMMU_DEV_EXTRA \
        -device virtio-serial-pci,disable-legacy=on,disable-modern=off$IOMMU_DEV_EXTRA \
        $CONSOLE_ARGS \
        $IOMMU_EXTRA_ARGS \
    "
fi

if [ "$VSOCK" = "on" ]; then
    # RAND_CID=$(shuf -i 3-65535 -n 1)
    RAND_CID=3
    echo "[$1] Launched QEMU VM with CID $RAND_CID" 1>&2
    if [ "$1" = "microvm" ]; then
        QEMU_ARGS="$QEMU_ARGS \
            -device vhost-vsock-device,guest-cid=$RAND_CID \
        "
    else
        QEMU_ARGS="$QEMU_ARGS \
            -device vhost-vsock-pci,id=vhost-vsock-pci0,guest-cid=$RAND_CID,disable-legacy=on,disable-modern=off$IOMMU_DEV_EXTRA \
        "
    fi
fi

# When using qemu-direct boot, OVMF depends on the boot protocol:
# linux-efi-* protocols require OVMF; other protocols (e.g. multiboot) do not.
if [ "$BOOT_METHOD" = "qemu-direct" ]; then
    if [ "$BOOT_PROTOCOL" = "linux-efi-pe64" ] || [ "$BOOT_PROTOCOL" = "linux-efi-handover64" ]; then
        OVMF="on"
    else
        OVMF="off"
    fi
fi

# When using `grub-rescue-iso` or `grub-qcow2` boot, OVMF must be enabled.
# Currently, the project's `grub-mkrescue` (in container image) only contained
# `x86_64-efi` platform modules — no `i386-pc`. This meant the generated ISO/qcow2
# could only be loaded by OVMF.
if [ "$BOOT_METHOD" = "grub-rescue-iso" ] || [ "$BOOT_METHOD" = "grub-qcow2" ]; then
    OVMF="on"
fi

if [ "$OVMF" = "on" ]; then
    if [ "$1" = "microvm" ]; then
        QEMU_ARGS="${QEMU_ARGS} \
            -bios /root/ovmf/release/microvm/MICROVM.fd \
        "
    else
        QEMU_ARGS="${QEMU_ARGS} \
            -bios /root/ovmf/release/OVMF.fd \
        "
    fi
fi

echo $QEMU_ARGS
