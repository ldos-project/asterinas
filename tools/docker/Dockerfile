# SPDX-License-Identifier: MPL-2.0

ARG BASE_VERSION
FROM ldosproject/osdk:${BASE_VERSION} AS build-base

SHELL ["/bin/bash", "-c"]

ARG DEBIAN_FRONTEND=noninteractive

#= Install packages for Docker building ====================================

# Please keep the list sorted by name
RUN apt update && apt-get install -y --no-install-recommends \
    apache2-utils \
    clang \
    cmake \
    curl \
    gnupg \
    libevent-dev \
    libslirp-dev \
    libssl-dev \
    jq \
    tcl-dev \
    unzip \
    zip

# Install python dependencies for DCPerf
RUN pip install numpy==1.23.5 && pip install pyyaml click tabulate pandas

    # Install all build dependencies required by DCPerf / TAO Bench
RUN apt-get update && apt-get install -y --no-install-recommends \
    autoconf \
    automake \
    bison \
    flex \
    python3-dev \
    libpcre3-dev \
    libzstd-dev \
    liblz4-dev \
    xz-utils \
    numactl \
    libsnappy-dev \
    libaio-dev \
    libunwind-dev \
    libghc-double-conversion-dev \
    libsodium-dev \
    libfmt-dev \
    libgoogle-glog-dev \
    libgflags-dev \
    libtool \
    dmidecode \
    lshw \
    lz4 \
    xzip \
    iperf3

#= Install linux-modules for vdso and linux config =================================
# TODO: This is very wasteful and maybe fragile. We should have our own custom vDSO.
# TODO: We should be able to use a much smaller Linux config. We stick with this for the moment since it is similar to upstream.

FROM build-base AS build-linux

RUN apt-get install -y --no-install-recommends \
    fakeroot \
    build-essential \
    ncurses-dev \
    xz-utils \
    libssl-dev \
    bc \
    flex \
    libelf-dev \
    bison \
    linux-modules-6.8.0-60-generic

WORKDIR /tmp

RUN wget https://cdn.kernel.org/pub/linux/kernel/v5.x/linux-5.15.105.tar.xz && \
    echo "01b537650332d2852722a626169cf7e5e798d11f9b578171b477868555f5e44f  linux-5.15.105.tar.xz" | sha256sum -c - 

RUN tar xf linux-5.15.105.tar.xz

WORKDIR /tmp/linux-5.15.105

# NOTE: This MAY pull the default configuration the linux-modules-6.8.0-60-generic package above.
RUN make defconfig && \
    scripts/config --disable SYSTEM_REVOCATION_KEYS && \
    scripts/config --disable SYSTEM_TRUSTED_KEYS && \
    scripts/config --enable EXT2_FS && \
    scripts/config --enable VIRTIO_PCI && \
    scripts/config --enable VIRTIO_MMIO && \
    scripts/config --enable VIRTIO_NET && \
    scripts/config --enable VIRTIO_BLK && \
    make olddefconfig

RUN make -j $(nproc) bzImage

RUN mkdir /opt/linux_binary_cache && \
    cp /usr/lib/modules/6.8.0-60-generic/vdso/vdso64.so /opt/linux_binary_cache/vdso64.so && \
    cp arch/x86/boot/bzImage /opt/linux_binary_cache/vmlinuz && \
    cp .config /opt/linux_binary_cache/vmlinuz.config

#= Build benchmark =========================================================

FROM build-base AS build-benchmarks

# Download the source files of benchmarks
WORKDIR /root
RUN apt update && apt install -y automake \
    libtool \
    pkg-config \
    libntirpc-dev

RUN wget https://github.com/akopytov/sysbench/archive/1.0.20.tar.gz \
    && tar -zxvf 1.0.20.tar.gz \
    && rm 1.0.20.tar.gz
RUN git clone https://github.com/nicktehrany/membench.git
RUN git clone https://github.com/esnet/iperf.git
RUN git clone https://github.com/kdlucas/byte-unixbench.git
RUN git clone https://github.com/asterinas/lmbench.git
RUN wget https://www.iozone.org/src/current/iozone3_506.tar
RUN tar -x -f iozone3_506.tar
RUN git clone -b fio-3.37 https://github.com/axboe/fio.git
RUN git clone https://git.kernel.org/pub/scm/linux/kernel/git/clrkwllms/rt-tests.git
RUN git clone https://git.kernel.org/pub/scm/linux/kernel/git/mason/schbench.git
RUN wget https://launchpad.net/libmemcached/1.0/1.0.18/+download/libmemcached-1.0.18.tar.gz \
    && tar -zxvf libmemcached-1.0.18.tar.gz \
    && rm libmemcached-1.0.18.tar.gz

# Build sysbench
WORKDIR /root/sysbench-1.0.20
RUN ./autogen.sh \
    && ./configure --without-mysql --prefix=/usr/local/benchmark/sysbench \
    && make -j \
    && make install

# Build membench
WORKDIR /root/membench
RUN make -j \
    && mkdir /usr/local/benchmark/membench \
    && cp membench /usr/local/benchmark/membench/

# Build iperf
WORKDIR /root/iperf
RUN ./configure --prefix=/usr/local/benchmark/iperf \
    && make -j \
    && make install

# Build lmbench
WORKDIR /root/lmbench
RUN make -j \
    && mv bin/x86_64-linux-gnu /usr/local/benchmark/lmbench \
    && rm /usr/local/benchmark/lmbench/*.o \
    && rm /usr/local/benchmark/lmbench/*.a

# Build unixbench
WORKDIR /root/byte-unixbench/UnixBench
RUN make UB_GCC_OPTIONS=-mno-sse2 -j && mv pgms /usr/local/benchmark/unixbench

# Build iozone
WORKDIR /root/iozone3_506/src/current
RUN CFLAGS=-static make linux-AMD64 \
    && cp iozone /usr/local/benchmark/

# Build fio
WORKDIR /root/fio
RUN ./configure --disable-shm --prefix=/usr/local/benchmark/fio \
    # Remove this when we support syscall timerfd_create and fadvise
    && sed -i -e '/#define CONFIG_HAVE_TIMERFD_CREATE/d' -e '/#define CONFIG_POSIX_FADVISE/d' config-host.h \
    && make -j \
    && make install

# Build hackbench
WORKDIR /root/rt-tests/src/hackbench
RUN make hackbench \
    && cp hackbench /usr/local/benchmark

# Build schbench
WORKDIR /root/schbench
RUN make \
    && cp schbench /usr/local/benchmark/

# Build memaslap for memcached
WORKDIR /root/libmemcached-1.0.18
RUN LDFLAGS='-lpthread' CPPFLAGS='-fcommon -fpermissive' CFLAGS='-fpermissive -fcommon' \
    ./configure --enable-memaslap --prefix=/usr/local/benchmark/libmemcached \
    && CPPFLAGS='-fcommon' make -j \
    && make install

# Clear cached files
WORKDIR /root
RUN rm -rf sysbench-1.0.20 \
    membench \
    iperf \
    lmbench \
    byte-unixbench \
    iozone3_506.tar \
    iozone3_506 \
    fio \
    hackbench \
    schbench \
    libmemcached-1.0.18

#= Install applications =======================================================

FROM build-base AS build-applications

# Install SQLite
WORKDIR /root
RUN apt-get install -y sqlite sqlite3
RUN wget https://www.sqlite.org/2024/sqlite-amalgamation-3460100.zip \
    && unzip sqlite-amalgamation-3460100.zip \
    && cd sqlite-amalgamation-3460100 \
    && gcc -g -shared -fPIC -c sqlite3.c \
    && gcc -g -shared -fPIC -o libsqlite3.so sqlite3.o \
    && mv ./libsqlite3.so /lib/x86_64-linux-gnu/ \
    && mv ./sqlite3.h /usr/include/x86_64-linux-gnu/ \
    && mv ./sqlite3ext.h /usr/include/x86_64-linux-gnu/

RUN rm -rf sqlite-amalgamation-3460100.zip \
    sqlite-amalgamation-3460100

# Install SQLite-speedtest
WORKDIR /root
RUN git clone --branch version-3.46.1 https://github.com/sqlite/sqlite.git
RUN cd sqlite \
    && mkdir bld \
    && cd bld \
    && ../configure --enable-all \
    && make speedtest1 \
    && cp ./speedtest1 /usr/local

RUN rm -rf sqlite

# Instal LevelDB 1.23
WORKDIR /root
RUN mkdir -p /usr/local/leveldb/benchmark/
RUN git clone -b 1.23 --recurse-submodules https://github.com/google/leveldb.git \
    && cd leveldb \
    && mkdir -p build \
    && cd build \
    && cmake -DCMAKE_BUILD_TYPE=Release .. \
    && cmake --build . \
    && make install \
    && mv ./db_bench /usr/local/leveldb/benchmark/ \
    && mv ./db_bench_sqlite3 /usr/local/leveldb/benchmark/

RUN rm -rf 1.23.tar.gz \
    leveldb-1.23

# Install Redis-7.0.15
WORKDIR /root
RUN wget https://download.redis.io/releases/redis-7.0.15.tar.gz \
    && tar -xzvf redis-7.0.15.tar.gz \
    && cd redis-7.0.15 \
    && make -j \
    && make PREFIX=/usr/local/redis install

RUN rm -rf redis-7.0.15.tar.gz \
    redis-7.0.15

# Install Nginx only with http enabled
WORKDIR /root
RUN wget https://nginx.org/download/nginx-1.26.2.tar.gz \
    && tar -xzvf nginx-1.26.2.tar.gz \
    && cd nginx-1.26.2 \
    && ./configure --with-cc-opt="-O2" --with-ld-opt="-static" --without-select_module --without-poll_module \
    --without-quic_bpf_module --without-http_charset_module --without-http_gzip_module --without-http_ssi_module \
    --without-http_userid_module --without-http_access_module --without-http_auth_basic_module --without-http_mirror_module \
    --without-http_geo_module --without-http_map_module --without-http_split_clients_module --without-http_referer_module \
    --without-http_rewrite_module --without-http_proxy_module --without-http_fastcgi_module --without-http_uwsgi_module \
    --without-http_scgi_module --without-http_grpc_module --without-http_memcached_module --without-http_limit_conn_module \
    --without-http_limit_req_module --without-http_empty_gif_module --without-http_browser_module --without-http_upstream_hash_module \
    --without-http_upstream_ip_hash_module --without-http_upstream_least_conn_module --without-http_upstream_random_module \
    --without-http_upstream_keepalive_module --without-http_upstream_zone_module --without-http-cache --without-mail_pop3_module \
    --without-mail_imap_module --without-mail_smtp_module --without-stream_limit_conn_module --without-stream_access_module \
    --without-stream_geo_module --without-stream_map_module --without-stream_split_clients_module --without-stream_return_module \
    --without-stream_pass_module --without-stream_set_module --without-stream_upstream_hash_module --without-stream_upstream_least_conn_module \
    --without-stream_upstream_random_module --without-stream_upstream_zone_module --without-pcre --without-pcre2

WORKDIR /root/nginx-1.26.2
RUN make -j \
    && make install

WORKDIR /root
RUN rm -rf nginx-1.26.2.tar.gz \
    nginx-1.26.2

# Install Memcached v1.6.32
WORKDIR /root
RUN wget https://www.memcached.org/files/memcached-1.6.32.tar.gz \
    && tar -xzvf memcached-1.6.32.tar.gz \
    && cd memcached-1.6.32 \
    && ./configure --prefix=/usr/local/memcached \
    && make -j \
    && make install

RUN rm -rf memcached-1.6.32.tar.gz \
    memcached-1.6.32

# Install Apache httpd v2.4.62
WORKDIR /root
RUN apt-get update && apt-get install -y --no-install-recommends libapr1-dev libaprutil1-dev libpcre2-dev
RUN wget https://archive.apache.org/dist/httpd/httpd-2.4.63.tar.bz2 \
    && tar xvf httpd-2.4.63.tar.bz2 \
    && cd httpd-2.4.63 \
    && ./configure \
    --disable-authn-file \
    --disable-authz-groupfile \
    --disable-authz-user \
    --disable-access-compat \
    --disable-auth-basic \
    --disable-reqtimeout \
    --disable-filter \
    --disable-charset-lite \
    --disable-mime \
    --disable-log-config \
    --disable-env \
    --disable-headers \
    --disable-setenvif \
    --disable-version \
    --disable-status \
    --disable-autoindex \
    --disable-dir \
    --disable-alias \
    && make -j \
    && make install

RUN rm -rf httpd-2.4.63.tar.bz2 \
    httpd-2.4.63

# Install wrk
WORKDIR /root
RUN git clone https://github.com/wg/wrk.git \
    && cd wrk \
    && make -j \
    && mkdir -p /usr/local/wrk \
    && cp wrk /usr/local/wrk

RUN rm -rf wrk

FROM build-base AS build-dcperf-1
# Install tao_bench_standalone
WORKDIR /usr/local
RUN git clone https://github.com/facebookresearch/DCPerf.git
WORKDIR /usr/local/DCPerf
RUN sed -i 's/sudo //g' ./packages/tao_bench/install_tao_bench_x86_64.sh
RUN ./benchpress_cli.py install tao_bench_standalone

# DCPerf installation
FROM build-dcperf-1 AS build-dcperf

### Trying this method of only installing binaries ###

# Get rid of this once the directory is actually being cloned and created
# RUN mkdir -p /usr/local/DCPerf/benchmarks/tao_bench && mkdir -p /usr/local/DCPerf/benchpress
# RUN mkdir -p /usr/local/DCPerf/benchmarks/tao_bench/certs && mkdir -p /usr/local/DCPerf/packages/tao_bench

# COPY DCPerf/benchmarks/tao_bench/tao_bench_client /usr/local/DCPerf/benchmarks/tao_bench
# COPY DCPerf/benchmarks/tao_bench/tao_bench_server /usr/local/DCPerf/benchmarks/tao_bench
# COPY DCPerf/benchpress /usr/local/DCPerf/benchpress
# COPY DCPerf/benchpress_cli.py /usr/local/DCPerf
# COPY DCPerf/benchmarks/tao_bench/leader_sizes.json /usr/local/DCPerf/benchmarks/tao_bench/
# COPY DCPerf/benchmarks/tao_bench/db_items.json /usr/local/DCPerf/benchmarks/tao_bench/
# COPY DCPerf/benchmarks/tao_bench/certs /usr/local/DCPerf/benchmarks/tao_bench/certs
# COPY DCPerf/benchmarks/tao_bench/memtier_client/memtier_benchmark /usr/local/DCPerf/benchmarks/tao_bench/
# # COPY run_tao_bench.py /usr/local/DCPerf/
# COPY DCPerf_Container/run_tao_bench.py /usr/local/DCPerf/
# COPY DCPerf/packages/tao_bench/run_standalone.py /usr/local/DCPerf/packages/tao_bench
# COPY DCPerf/packages /usr/local/DCPerf/packages
# RUN python3 - << 'EOF'
# content = open('/usr/local/DCPerf/packages/tao_bench/run_autoscale.py').read()
# old = 'def find_numa_nodes():\n    numa_nodes = {}'
# new = 'def find_numa_nodes():\n    if not os.path.exists("/sys/devices/system/node"):\n        return {"0": "0"}\n    numa_nodes = {}'
# open('/usr/local/DCPerf/packages/tao_bench/run_autoscale.py', 'w').write(content.replace(old, new))
# EOF
# RUN python3 - << 'EOF'
# import re

# # Fix run_standalone.py
# content = open('/usr/local/DCPerf/packages/tao_bench/run_standalone.py').read()
# content = content.replace(
#     'cmd = [f"{TAO_BENCH_DIR}/run_autoscale.py --real"]',
#     'cmd = [f"python3 {TAO_BENCH_DIR}/run_autoscale.py --real"]'
# )
# content = content.replace(
#     'cmd = str(BENCHPRESS_ROOT) + client[1:]',
#     'cmd = "python3 " + str(BENCHPRESS_ROOT) + client[1:]'
# )
# open('/usr/local/DCPerf/packages/tao_bench/run_standalone.py', 'w').write(content)

# # Fix run_autoscale.py - add python3 before run.py calls
# content = open('/usr/local/DCPerf/packages/tao_bench/run_autoscale.py').read()
# content = content.replace(
#     'f"{TAO_BENCH_DIR}/run.py"',
#     'f"python3 {TAO_BENCH_DIR}/run.py"'
# )
# open('/usr/local/DCPerf/packages/tao_bench/run_autoscale.py', 'w').write(content)
# EOF

### End binary method ###

### Try moving everything into the container ###
# Install tao_bench_standalone
# WORKDIR /usr/local
# RUN git clone https://github.com/facebookresearch/DCPerf.git
# WORKDIR /usr/local/DCPerf
# RUN sed -i 's/sudo //g' ./packages/tao_bench/install_tao_bench_x86_64.sh
# RUN ./benchpress_cli.py install tao_bench_standalone
# COPY tao_bench_client tao_bench_server /usr/local/DCPerf
# RUN mkdir /usr/local/DCPerf
# COPY hello.txt /usr/local/DCPerf
# COPY tao_bench_client /usr/local/DCPerf
# COPY tao_bench_server /usr/local/DCPerf
# RUN ls /usr/local/DCPerf
# This method does not install everything (for debugging purposes)
# RUN mkdir DCPerf_Container
# COPY DCPerf_Container /usr/local/DCPerf_Container

RUN rm -rf /usr/local/DCPerf/benchmarks/tao_bench/build-folly
# RUN rm -rf /usr/local/DCPerf/benchmarks/tao_bench/build-deps
RUN rm -rf /usr/local/DCPerf/packages/spark_standalone
RUN rm -rf /usr/local/DCPerf/packages/mediawiki
RUN rm -rf /usr/local/DCPerf/packages/feedsim
RUN rm -rf /usr/local/DCPerf/packages/video_transcode_bench
RUN rm -rf /usr/local/DCPerf/packages/django_workload
RUN rm -rf /usr/local/DCPerf/packages/wdl_bench
RUN rm -rf /usr/local/DCPerf/benchmarks/tao_bench/openssl/test
RUN cat > /usr/bin/lscpu << 'EOF'
#!/bin/sh
echo "Architecture:            x86_64"
echo "Vendor ID:               GenuineIntel"
echo "Model name:              Intel(R) Xeon(R) CPU E5-2620 v4 @ 2.10GHz"
echo "CPU(s):                  32"
echo "Thread(s) per core:      2"
echo "Core(s) per socket:      8"
echo "Socket(s):               2"
echo "NUMA node(s):            2"
echo "NUMA node0 CPU(s):       0,2,4,6,8,10,12,14,16,18,20,22,24,26,28,30"
echo "NUMA node1 CPU(s):       1,3,5,7,9,11,13,15,17,19,21,23,25,27,29,31"
EOF
RUN chmod +x /usr/bin/lscpu

RUN sed -i 's/run_cmd("lscpu")/run_cmd("\/bin\/lscpu")/g' \
    /usr/local/DCPerf/benchpress/cli/commands/system_check.py

RUN sed -i 's/\["lscpu"\]/["\/bin\/lscpu"]/g' \
    /usr/local/DCPerf/benchpress/lib/sys_specs.py

RUN sed -i 's/"lscpu", stdout/"\/bin\/lscpu", stdout/g' \
    /usr/local/DCPerf/benchpress/plugins/hooks/perf_monitors/topdown.py

RUN rm -f \
    /usr/local/DCPerf/benchpress/cli/commands/__pycache__/system_check.cpython-310.pyc \
    /usr/local/DCPerf/benchpress/plugins/hooks/perf_monitors/__pycache__/topdown.cpython-310.pyc \
    /usr/local/DCPerf/benchpress/lib/__pycache__/sys_specs.cpython-310.pyc

RUN python3 - << 'EOF'
content = open('/usr/local/DCPerf/benchpress/lib/sys_specs.py').read()

content = content.replace(
    'def get_dmidecode_data():\n    dmidecode_data = dmidecode.parse()\n    return dmidecode_data',
    'def get_dmidecode_data():\n    return {}'
)

content = content.replace(
    'def get_dpkg_packages():\n    dpkg_p = subprocess.Popen(',
    'def get_dpkg_packages():\n    return []\n    dpkg_p = subprocess.Popen('
)

open('/usr/local/DCPerf/benchpress/lib/sys_specs.py', 'w').write(content)
EOF

RUN rm -f /usr/local/DCPerf/benchpress/lib/__pycache__/sys_specs.cpython-310.pyc

RUN python3 - << 'EOF'
content = open('/usr/local/DCPerf/benchpress/lib/sys_specs.py').read()

replacements = [
    ('def get_dmidecode_data():\n    dmidecode_data = dmidecode.parse()\n    return dmidecode_data',
     'def get_dmidecode_data():\n    return {}'),
    ('def get_dpkg_packages():\n    dpkg_p = subprocess.Popen(',
     'def get_dpkg_packages():\n    return []\n    dpkg_p = subprocess.Popen('),
    ('def get_rpm_packages():\n    rpm_p = subprocess.Popen(',
     'def get_rpm_packages():\n    return []\n    rpm_p = subprocess.Popen('),
    ('def get_sysctl_data():\n    sysctl_p = subprocess.Popen(',
     'def get_sysctl_data():\n    return {}\n    sysctl_p = subprocess.Popen('),
    ('def get_hw_data():\n    hw_p = subprocess.Popen(',
     'def get_hw_data():\n    return {}\n    hw_p = subprocess.Popen('),
]

for old, new in replacements:
    content = content.replace(old, new)

open('/usr/local/DCPerf/benchpress/lib/sys_specs.py', 'w').write(content)
EOF

RUN python3 - << 'EOF'
content = open('/usr/local/DCPerf/packages/tao_bench/run_autoscale.py').read()
content = content.replace(
    'def find_numa_nodes():\n    numa_nodes = {}',
    'def find_numa_nodes():\n    if not os.path.exists("/sys/devices/system/node"):\n        return {"0": list(range(32))}\n    numa_nodes = {}'
)
open('/usr/local/DCPerf/packages/tao_bench/run_autoscale.py', 'w').write(content)
EOF

# Fix run_standalone.py
RUN python3 - << 'EOF'
content = open('/usr/local/DCPerf/packages/tao_bench/run_standalone.py').read()
content = content.replace(
    'cmd = [f"{TAO_BENCH_DIR}/run_autoscale.py --real"]',
    'cmd = [f"python3 {TAO_BENCH_DIR}/run_autoscale.py --real"]'
)
content = content.replace(
    'cmd = str(BENCHPRESS_ROOT) + client[1:]',
    'cmd = "python3 " + str(BENCHPRESS_ROOT) + client[1:]'
)
open('/usr/local/DCPerf/packages/tao_bench/run_standalone.py', 'w').write(content)

# Fix run_autoscale.py - add python3 before run.py calls
content = open('/usr/local/DCPerf/packages/tao_bench/run_autoscale.py').read()
content = content.replace(
    'f"{TAO_BENCH_DIR}/run.py"',
    'f"python3 {TAO_BENCH_DIR}/run.py"'
)
open('/usr/local/DCPerf/packages/tao_bench/run_autoscale.py', 'w').write(content)
EOF

RUN python3 - << 'EOF'
content = open('/usr/local/DCPerf/packages/tao_bench/run_autoscale.py').read()
content = content.replace(
    '        os.path.join(TAO_BENCH_DIR, "run.py"),\n        "server",',
    '        "python3",\n        os.path.join(TAO_BENCH_DIR, "run.py"),\n        "server",'
)
open('/usr/local/DCPerf/packages/tao_bench/run_autoscale.py', 'w').write(content)
EOF

# fix all get affinity issues
RUN python3 - << 'EOF'
content = open('/usr/local/DCPerf/packages/tao_bench/run.py').read()
content = content.replace(
    '        n_threads = len(os.sched_getaffinity(0)) - 6\n        if n_threads <= 0:\n            n_threads = int(len(os.sched_getaffinity(0)) * 0.8)',
    '        n_threads = max(32 - 6, 1)'
)
open('/usr/local/DCPerf/packages/tao_bench/run.py', 'w').write(content)
EOF

RUN python3 - << 'EOF'
content = open('/usr/local/DCPerf/packages/tao_bench/run_autoscale.py').read()
content = content.replace(
    'args.client_cores = len(os.sched_getaffinity(0))',
    'args.client_cores = 32'
)
open('/usr/local/DCPerf/packages/tao_bench/run_autoscale.py', 'w').write(content)
EOF

RUN python3 - << 'EOF'
content = open('/usr/local/DCPerf/packages/tao_bench/run.py').read()

# Fix affinitize_nic
content = content.replace(
    'def affinitize_nic(args):\n    n_cores = len(os.sched_getaffinity(0))',
    'def affinitize_nic(args):\n    n_cores = 32'
)

# Fix run_server 
content = content.replace(
    'def run_server(args):\n    n_cores = len(os.sched_getaffinity(0))',
    'def run_server(args):\n    n_cores = 32'
)

# Fix client n_threads
content = content.replace(
    '        n_threads = len(os.sched_getaffinity(0)) - 6\n        if n_threads <= 0:\n            n_threads = int(len(os.sched_getaffinity(0)) * 0.8)',
    '        n_threads = max(32 - 6, 1)'
)

open('/usr/local/DCPerf/packages/tao_bench/run.py', 'w').write(content)
EOF

RUN python3 - << 'EOF'
content = open('/usr/local/DCPerf/packages/tao_bench/run_autoscale.py').read()

# Fix client_cores; number returned is too low. make it match lscpu file
content = content.replace(
    'args.client_cores = len(os.sched_getaffinity(0))',
    'args.client_cores = 32'
)

# Fix n_cores and core_list, number returned is negative otherwise
content = content.replace(
    '    n_cores = len(os.sched_getaffinity(0))\n    core_list = list(os.sched_getaffinity(0))',
    '    n_cores = 32\n    core_list = list(range(32))'
)

open('/usr/local/DCPerf/packages/tao_bench/run_autoscale.py', 'w').write(content)
EOF

# # fix localhost issue; localhost not configured in asterinas so explicitly write address
# RUN python3 - << 'EOF'
# content = open('/usr/local/DCPerf/packages/tao_bench/run_standalone.py').read()
# content = content.replace(
#     'args.server_hostname = "localhost"',
#     'args.server_hostname = "127.0.0.1"'
# )
# open('/usr/local/DCPerf/packages/tao_bench/run_standalone.py', 'w').write(content)
# EOF

# # run autoscale takes -c as an argument, not cpu-list
# RUN python3 - << 'EOF'
# content = open('/usr/local/DCPerf/packages/tao_bench/run_autoscale.py').read()
# content = content.replace(
#     '"--cpu-list"',
#     '"-c"'
# )
# open('/usr/local/DCPerf/packages/tao_bench/run_autoscale.py', 'w').write(content)
# EOF

# fix run.py to explicitly use 127.0.0.1
RUN python3 - << 'EOF'
content = open('/usr/local/DCPerf/packages/tao_bench/run.py').read()
content = content.replace(
    '"-p",\n        str(port_num),',
    '"-p",\n        str(port_num),\n        "-l",\n        "127.0.0.1",'
)
open('/usr/local/DCPerf/packages/tao_bench/run.py', 'w').write(content)
EOF

RUN cp /usr/local/DCPerf/benchmarks/tao_bench/openssl/libcrypto.so.3 \
       /usr/local/DCPerf/benchmarks/tao_bench/build-deps/lib/libcrypto.so.3

RUN mkdir -p /usr/local/benchmark
COPY test_hugepages.c /usr/local/benchmark
WORKDIR /usr/local/benchmark
RUN gcc -o test_hugepages test_hugepages.c


#= Build syscall test =========================================================

FROM build-base AS build-bazel

# Install bazel, which is required by the system call test suite from Gvisor project
RUN mkdir -p /root/bazel
COPY tools/install_bazel.sh /root/bazel/
WORKDIR /root/bazel
RUN ./install_bazel.sh
RUN apt clean && rm -rf /var/lib/apt/lists/*
WORKDIR /root
RUN rm -rf bazel

FROM build-bazel AS build-gvisor

# Build the gvisor syscall test binaries
COPY test/syscall_test/gvisor /root/gvisor
WORKDIR /root/gvisor
RUN export BUILD_DIR=build && \
    make ${BUILD_DIR}/syscall_test_bins

#= Build syscall test of Linux Test Project====================================

FROM build-base AS build-ltp

WORKDIR /root
RUN apt update && apt-get install -y --no-install-recommends \
    autoconf \
    automake \
    pkg-config
RUN git clone --single-branch -b v20250130 https://github.com/asterinas/ltp.git
WORKDIR /root/ltp
RUN make autotools \
    && ./configure --prefix=/opt/ltp \
    && make -C testcases/kernel/syscalls \
    && make -C testcases/lib \
    && make -C runtest \
    && make -C pan \
    && make -C testcases/kernel/syscalls install \
    && make -C testcases/lib install \
    && make -C runtest install \
    && make -C pan install \
    && install -m 00755 ./runltp /opt/ltp/runltp \
    && install -m 00444 ./VERSION /opt/ltp/Version \
    && install -m 00755 ./ver_linux /opt/ltp/ver_linux \
    && install -m 00755 ./IDcheck.sh /opt/ltp/IDcheck.sh

#= Build busybox ==============================================================

FROM build-base AS build-busybox

RUN apt clean && rm -rf /var/lib/apt/lists/*

FROM build-busybox AS busybox

WORKDIR /root
RUN wget -O busybox.tar.bz2 https://busybox.net/downloads/busybox-1.35.0.tar.bz2 \
    && mkdir /root/busybox \
    && tar xf busybox.tar.bz2 --strip-components=1 -C /root/busybox \
    && rm busybox.tar.bz2
WORKDIR /root/busybox
RUN make defconfig \
    && sed -i "s/# CONFIG_STATIC is not set/CONFIG_STATIC=y/g" .config \
    && sed -i "s/# CONFIG_FEATURE_SH_STANDALONE is not set/CONFIG_FEATURE_SH_STANDALONE=y/g" .config \
    && make -j

#= The final stages to produce the Asterinas development image ====================

FROM build-base

# Install all Asterinas dependent packages
RUN apt update && apt-get install -y --no-install-recommends \
    bridge-utils \
    clang-format       `# formatting general tests` \
    cpio \
    cpuid \
    exfatprogs \
    file \
    grub-efi-amd64-bin \
    grub-efi-amd64-dbg \
    iptables \
    iproute2 \
    libnl-3-dev         `# dependency for netlink socket` \
    libnl-route-3-dev   `# dependency for netlink route socket` \
    net-tools \
    openssh-server \
    pkg-config \
    socat \
    strace \
    sudo \
    unzip \
    vim \
    zip
# Clean apt cache
RUN apt clean && rm -rf /var/lib/apt/lists/*

# Prepare the system call test suite

# Install Busybox built from the previous stages
COPY --from=busybox /root/busybox/busybox /bin/busybox

# Install benchmarks built from the previous stages

# Install applications built from the previous stages

COPY --from=build-dcperf /usr/local/DCPerf /usr/local/DCPerf
COPY --from=build-dcperf /usr/bin/lscpu /usr/local/bin/lscpu
COPY --from=build-dcperf /usr/local/benchmark /usr/local/benchmark

COPY --from=build-linux /opt/linux_binary_cache /opt/linux_binary_cache

# Add the path of Asterinas tools
ENV PATH="/root/asterinas/target/bin:${PATH}"

VOLUME [ "/root/asterinas" ]

WORKDIR /root/asterinas
