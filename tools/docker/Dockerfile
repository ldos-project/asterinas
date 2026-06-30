# SPDX-License-Identifier: MPL-2.0

ARG BASE_VERSION
FROM ldosproject/nix:${BASE_VERSION} AS build-base

SHELL ["/bin/bash", "-c"]

ARG DEBIAN_FRONTEND=noninteractive

#= Install linux-modules for vdso and linux config =================================
# TODO: This is very wasteful and maybe fragile. We should have our own custom vDSO.
# TODO: We should be able to use a much smaller Linux config. We stick with this for the moment since it is similar to upstream.

FROM build-base AS build-linux

RUN apt-get update && apt-get install -y --no-install-recommends \
    linux-modules-6.17.0-35-generic
    # fakeroot \
    # build-essential \
    # ncurses-dev \
    # xz-utils \
    # libssl-dev \
    # bc \
    # flex \
    # libelf-dev \
    # bison \

# WORKDIR /tmp

# RUN wget https://cdn.kernel.org/pub/linux/kernel/v5.x/linux-5.15.105.tar.xz && \
#     echo "01b537650332d2852722a626169cf7e5e798d11f9b578171b477868555f5e44f  linux-5.15.105.tar.xz" | sha256sum -c - 

# RUN tar xf linux-5.15.105.tar.xz

# WORKDIR /tmp/linux-5.15.105

# # NOTE: This MAY pull the default configuration the linux-modules-6.8.0-60-generic package above.
# RUN make defconfig && \
#     scripts/config --disable SYSTEM_REVOCATION_KEYS && \
#     scripts/config --disable SYSTEM_TRUSTED_KEYS && \
#     scripts/config --enable EXT2_FS && \
#     scripts/config --enable VIRTIO_PCI && \
#     scripts/config --enable VIRTIO_MMIO && \
#     scripts/config --enable VIRTIO_NET && \
#     scripts/config --enable VIRTIO_BLK && \
#     make olddefconfig

# RUN make -j $(nproc) bzImage

RUN mkdir /opt/linux_binary_cache && \
    cp /usr/lib/modules/6.17.0-35-generic/vdso/vdso64.so /opt/linux_binary_cache/vdso64.so
    # cp arch/x86/boot/bzImage /opt/linux_binary_cache/vmlinuz && \
    # cp .config /opt/linux_binary_cache/vmlinuz.config

#= Build syscall test =========================================================

FROM build-base AS build-gvisor

# Install required packages for bazel
RUN apt update && apt install -y curl gnupg2 git \
    python-is-python3 python3 python3-setuptools python3-pip \
    build-essential crossbuild-essential-arm64 qemu-user-static \
    openjdk-11-jdk-headless zip unzip \
    apt-transport-https ca-certificates gnupg-agent \
    software-properties-common \
    pkg-config libffi-dev patch diffutils libssl-dev iptables kmod \
    clang crossbuild-essential-amd64 erofs-utils busybox-static libbpf-dev \
    iproute2 netcat-openbsd libnuma-dev libc6-dev-i386 golang-go

ARG TARGETARCH

# Install bazel
RUN set -eux; \
    case "${TARGETARCH}" in \
        amd64|arm64) deb="bazelisk-${TARGETARCH}.deb" ;; \
        *) echo "Unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    wget -O "${deb}" "https://github.com/bazelbuild/bazelisk/releases/download/v1.27.0/${deb}"; \
    dpkg -i "${deb}"; \
    rm -f "${deb}"

# Build the gvisor syscall test binaries
WORKDIR /root
RUN git clone -b release-20251215.0 --single-branch --depth 1 https://github.com/google/gvisor.git
WORKDIR /root/gvisor
RUN bazel build --test_tag_filters=native //test/syscalls/... && \
    mkdir -p /root/syscall_test_bins && \
    cp bazel-bin/test/syscalls/linux/*_test /root/syscall_test_bins

#= The final stages to produce the Asterinas development image ====================

FROM build-base

# Install all Asterinas dependent packages
RUN apt update && apt-get install -y --no-install-recommends \
    bridge-utils \
    clang-format       `# formatting general tests` \
    cpio \
    cpuid \
    dosfstools \
    exfatprogs \
    file \
    grub-efi-amd64-bin \
    grub-efi-amd64-dbg \
    iproute2 \
    iptables \
    jq \
    net-tools \
    netcat-openbsd \
    openssh-server \
    parted \
    pkg-config \
    socat \
    strace \
    sudo \
    unzip \
    vim \
    zip
# Clean apt cache
RUN apt clean && rm -rf /var/lib/apt/lists/*

# Install sctrace
RUN ln -s /root/asterinas/tools/sctrace.sh /usr/local/bin/sctrace

# Set the `ASTER_SCML` env var to make sctrace easier to use for Asterinas developers.
# For more information, check out the README of sctrace.
RUN echo 'export ASTER_SCML=$(find /root/asterinas/book/src/kernel/linux-compatibility/ -name "*.scml")' >> /root/.bashrc

# Copy the gvisor syscall test binaries
COPY --from=build-gvisor /root/syscall_test_bins /root/syscall_test_bins
ENV GVISOR_PREBUILT_DIR=/root/syscall_test_bins

# Copy linux binaries
COPY --from=build-linux /opt/linux_binary_cache /opt/linux_binary_cache
ENV VDSO_LIBRARY_DIR="/opt/linux_binary_cache"

# Add the path of Asterinas tools
ENV PATH="/root/asterinas/target/bin:${PATH}"

VOLUME [ "/root/asterinas" ]

WORKDIR /root/asterinas
