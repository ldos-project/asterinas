# SPDX-License-Identifier: MPL-2.0

BINARY_CACHE_DIR := /opt/linux_binary_cache
VDSO_DIR := ../target
VDSO_LIB := $(VDSO_DIR)/vdso64.so
MKFILE_PATH := $(abspath $(lastword $(MAKEFILE_LIST)))
CUR_DIR := $(patsubst %/,%,$(dir $(MKFILE_PATH)))
ATOMIC_WGET := $(CUR_DIR)/../tools/atomic_wget.sh
BUILD_DIR := $(CUR_DIR)/build
INITRAMFS := $(BUILD_DIR)/initramfs
INITRAMFS_FILELIST := $(BUILD_DIR)/initramfs.filelist
INITRAMFS_SKIP_GZIP ?= 0
ifeq ($(INITRAMFS_SKIP_GZIP),1)
INITRAMFS_IMAGE := $(BUILD_DIR)/initramfs.cpio
else
INITRAMFS_IMAGE := $(BUILD_DIR)/initramfs.cpio.gz
endif
EXT2_IMAGE := $(BUILD_DIR)/ext2.img
EXFAT_IMAGE := $(BUILD_DIR)/exfat.img
RAID1_IMAGE0 := $(BUILD_DIR)/raid1_0.img
RAID1_IMAGE1 := $(BUILD_DIR)/raid1_1.img

INITRAMFS_EMPTY_DIRS := \
	$(INITRAMFS)/root \
	$(INITRAMFS)/tmp \
	$(INITRAMFS)/opt \
	$(INITRAMFS)/proc \
	$(INITRAMFS)/dev \
	$(INITRAMFS)/sys \
	$(INITRAMFS)/ext2 \
	$(INITRAMFS)/exfat \
	$(INITRAMFS)/raid1
INITRAMFS_ALL_DIRS := \
	$(INITRAMFS)/etc \
	$(INITRAMFS)/lib/x86_64-linux-gnu \
	$(INITRAMFS)/lib64 \
	$(INITRAMFS)/bin \
	$(INITRAMFS)/sbin \
	$(INITRAMFS)/usr/bin \
	$(INITRAMFS)/usr/local \
	$(INITRAMFS)/benchmark \
	$(INITRAMFS_EMPTY_DIRS)

INITRAMFS_ALL_DIRS += $(INITRAMFS)/etc/os-release
INITRAMFS_ALL_DIRS += $(INITRAMFS)/bin/python3
INITRAMFS_ALL_DIRS += $(INITRAMFS)/bin/numactl
INITRAMFS_ALL_DIRS += $(INITRAMFS)/usr/bin
INITRAMFS_ALL_DIRS += $(INITRAMFS)/usr/lib/python3.10
INITRAMFS_ALL_DIRS += $(INITRAMFS)/usr/local/lib/python3.10/dist-packages

# Include test as target if BENCHMARK is not set.
ifeq ($(BENCHMARK), none)
INITRAMFS_ALL_DIRS += $(INITRAMFS)/test
endif

SYSCALL_TEST_DIR := $(INITRAMFS)/opt/syscall_test

.PHONY: all
all: build

$(INITRAMFS)/lib/x86_64-linux-gnu: | $(VDSO_LIB)
	@mkdir -p $@
	@cp -L /lib/x86_64-linux-gnu/libc.so.6 $@
	@cp -L /lib/x86_64-linux-gnu/libstdc++.so.6 $@
	@cp -L /lib/x86_64-linux-gnu/libm.so.6 $@
	@cp -L /lib/x86_64-linux-gnu/libgcc_s.so.1 $@
	@cp -L /lib/x86_64-linux-gnu/libpthread.so.0 $@
	@# required for benchmarks
	@cp -L /lib/x86_64-linux-gnu/libcrypt.so.1 $@
	@cp -L /lib/x86_64-linux-gnu/libdl.so.2 $@
	@cp -L /lib/x86_64-linux-gnu/libz.so.1 $@
	@cp -L /lib/x86_64-linux-gnu/libmvec.so.1 $@
	@# required for LMbench-network
	@cp -L /lib/x86_64-linux-gnu/libtirpc.so.3 $@
	@cp -L /lib/x86_64-linux-gnu/libgssapi_krb5.so.2 $@
	@cp -L /lib/x86_64-linux-gnu/libkrb5.so.3 $@
	@cp -L /lib/x86_64-linux-gnu/libk5crypto.so.3 $@
	@cp -L /lib/x86_64-linux-gnu/libcom_err.so.2 $@
	@cp -L /lib/x86_64-linux-gnu/libkrb5support.so.0 $@
	@cp -L /lib/x86_64-linux-gnu/libkeyutils.so.1 $@
	@cp -L /lib/x86_64-linux-gnu/libresolv.so.2 $@
	@# required for LevelDB-db_bench_sqlite3
# 	@cp -L /lib/x86_64-linux-gnu/libsqlite3.so $@
	@# required for memcached
	@cp -L /lib/x86_64-linux-gnu/libevent-2.1.so.7 $@
	@# required for VDSO
	@cp -L $(VDSO_LIB) $@
	@# required for netlink test
	@cp -L /lib/x86_64-linux-gnu/libnl-3.so.200 $@
	@cp -L /lib/x86_64-linux-gnu/libnl-route-3.so.200 $@
	@# required for tao_bench (custom build-deps)
	@cp -L /usr/local/DCPerf/benchmarks/tao_bench/build-deps/lib/libcrypto.so.3 $@
	@cp -L /usr/local/DCPerf/benchmarks/tao_bench/build-deps/lib/libevent_openssl-2.1.so.6 $@
	@cp -L /usr/local/DCPerf/benchmarks/tao_bench/build-deps/lib/libevent-2.1.so.6 $@
	@cp -L /usr/local/DCPerf/benchmarks/tao_bench/build-deps/lib/libssl.so.3 $@
	@cp -L /lib/x86_64-linux-gnu/libsmartcols.so.1 $@
	@# required for tao_bench_server (system libs)
	@cp -L /lib/x86_64-linux-gnu/libdouble-conversion.so.3 $@
	@cp -L /lib/x86_64-linux-gnu/liblz4.so.1 $@
	@cp -L /lib/x86_64-linux-gnu/libglog.so.0 $@
	@cp -L /lib/x86_64-linux-gnu/libunwind.so.8 $@
	@cp -L /lib/x86_64-linux-gnu/libgflags.so.2.2 $@
	@cp -L /lib/x86_64-linux-gnu/liblzma.so.5 $@
	@cp -L /lib/x86_64-linux-gnu/libffi.so.8 $@
	@# required for python3.10
	@cp -L /lib/x86_64-linux-gnu/libexpat.so.1 $@
	@# required for numactl
	@cp -L /lib/x86_64-linux-gnu/libnuma.so.1 $@

$(INITRAMFS)/bin/python3: | $(INITRAMFS)
	@mkdir -p $(INITRAMFS)/bin
	@cp -L /usr/bin/python3.10 $@

$(INITRAMFS)/bin/numactl: | $(INITRAMFS)
	@mkdir -p $(INITRAMFS)/bin
	@cp -L /usr/bin/numactl $@

$(INITRAMFS)/usr/lib/python3.10: | $(INITRAMFS)
	@mkdir -p $@
	@cp -rL /usr/lib/python3.10/. $@/

$(INITRAMFS)/usr/local/lib/python3.10/dist-packages: | $(INITRAMFS)
	@mkdir -p $@
	@cp -rL /usr/local/lib/python3.10/dist-packages/tabulate $@/
	@cp -rL /usr/local/lib/python3.10/dist-packages/click $@/
	@cp -rL /usr/local/lib/python3.10/dist-packages/yaml $@/
	@cp -rL /usr/local/lib/python3.10/dist-packages/pandas $@/
	@cp -rL /usr/local/lib/python3.10/dist-packages/numpy $@/
	@cp -rL /usr/local/lib/python3.10/dist-packages/numpy.libs $@/
	@cp -rL /usr/local/lib/python3.10/dist-packages/pytz $@/
	@cp -rL /usr/local/lib/python3.10/dist-packages/dateutil $@/
	@cp -rL /usr/local/lib/python3.10/dist-packages/six.py $@/

$(INITRAMFS)/usr/local/DCPerf: | $(INITRAMFS)
# 	@cp -rL /usr/local/DCPerf $(INITRAMFS)/usr/local/DCPerf
	@mkdir -p $(INITRAMFS)/usr/local/DCPerf/benchmarks/tao_bench
# 	@cp -L /usr/local/DCPerf/benchmarks/tao_bench/tao_bench_server $(INITRAMFS)/usr/local/DCPerf/benchmarks/tao_bench/
# 	@cp -L /usr/local/DCPerf/benchmarks/tao_bench/tao_bench_client $(INITRAMFS)/usr/local/DCPerf/benchmarks/tao_bench/
# 	@cp -L /usr/local/DCPerf/benchmarks/tao_bench/leader_sizes.json $(INITRAMFS)/usr/local/DCPerf/benchmarks/tao_bench/
# 	@cp -L /usr/local/DCPerf/benchmarks/tao_bench/db_items.json $(INITRAMFS)/usr/local/DCPerf/benchmarks/tao_bench/
# 	@cp -rL /usr/local/DCPerf/benchmarks/tao_bench/certs $(INITRAMFS)/usr/local/DCPerf/benchmarks/tao_bench/
# 	@cp -L /usr/local/DCPerf/benchmarks/tao_bench/memtier_benchmark $(INITRAMFS)/usr/local/DCPerf/benchmarks/tao_bench/
# 	@cp -rL /usr/local/DCPerf/benchpress $(INITRAMFS)/usr/local/DCPerf/
# 	@cp -rL /usr/local/DCPerf/packages $(INITRAMFS)/usr/local/DCPerf/
# 	@cp -L /usr/local/DCPerf/benchpress_cli.py $(INITRAMFS)/usr/local/DCPerf/
# 	@cp -L /usr/local/DCPerf/run_tao_bench.py $(INITRAMFS)/usr/local/DCPerf/
# 	@cp -L /usr/local/DCPerf/packages/tao_bench/run_standalone.py $(INITRAMFS)/usr/local/DCPerf/packages/tao_bench

$(INITRAMFS)/etc/os-release: | $(INITRAMFS)
	@mkdir -p $(INITRAMFS)/etc
	@echo 'NAME="Ubuntu"\nVERSION_ID="22.04"\nID=ubuntu' > $@

$(VDSO_LIB): | $(VDSO_DIR) $(BINARY_CACHE_DIR)/vdso64.so
	@# TODO: use a custom compiled vdso.so file in the future.
	@cp $(BINARY_CACHE_DIR)/vdso64.so $@
	
$(VDSO_DIR):
	@mkdir -p $@
	
$(BINARY_CACHE_DIR)/vdso64.so:
	@mkdir -p $(BINARY_CACHE_DIR)
	@$(ATOMIC_WGET) $@ "https://raw.githubusercontent.com/asterinas/linux_vdso/2a6d2db/vdso64.so"

$(INITRAMFS)/lib64:
	@mkdir -p $@
	@cp -L /lib64/ld-linux-x86-64.so.2 $@

$(INITRAMFS)/etc:
	@mkdir -p $@
	@cp $(CUR_DIR)/etc/* $@

# Install busybox into /bin and /usr/bin.
$(INITRAMFS)/bin:
	@mkdir -p $@
	@/bin/busybox --install -s $@
	@cp -L /usr/local/bin/lscpu $(INITRAMFS)/bin/

$(INITRAMFS)/sbin:
	@mkdir -p $@
	@cp /sbin/ldconfig $@
	@cp /sbin/ldconfig.real $@

$(INITRAMFS)/usr/bin: | $(INITRAMFS)/bin
	@mkdir -p $@
	@cp /usr/bin/busybox $@

$(INITRAMFS)/usr/local:
	@mkdir -p $@

.PHONY: $(INITRAMFS)/test
$(INITRAMFS)/test:
	@$(MAKE) --no-print-directory -C apps

$(INITRAMFS)/benchmark: | $(INITRAMFS)/benchmark/bin
	@mkdir -p $@
	@cp -L /usr/local/benchmark/test_hugepages $@
	@cp -L /usr/local/benchmark/test_hugepages.c $@

$(INITRAMFS)/benchmark/bin:
	@mkdir -p $@

# Make necessary directories.
$(INITRAMFS_EMPTY_DIRS):
	@mkdir -p $@

.PHONY: $(SYSCALL_TEST_DIR)
$(SYSCALL_TEST_DIR):
	@$(MAKE) --no-print-directory -C syscall_test

.PHONY: $(INITRAMFS_IMAGE)
$(INITRAMFS_IMAGE): $(INITRAMFS_FILELIST)
	@if ! cmp -s $(INITRAMFS_FILELIST) $(INITRAMFS_FILELIST).previous || ! test -f $@; then \
		echo "Generating the initramfs image..."; \
		cp -f $(INITRAMFS_FILELIST) $(INITRAMFS_FILELIST).previous; \
		( \
			cd $(INITRAMFS); \
			# `$(INITRAMFS_FILELIST)` contains files' last modification \
			# time in the first column, and files' relative path to \
			# `$(INITRAMFS)` in the second column. This prunes the first \
			# column and passes the second column to `cpio`. \
			cut -d " " -f 2- $(INITRAMFS_FILELIST) | \
				cpio -o -H newc | \
					if [ "$(INITRAMFS_SKIP_GZIP)" != 1 ]; then \
						gzip; \
					else \
						cat; \
					fi \
		) > $@; \
	fi

.PHONY: $(INITRAMFS_FILELIST)
# If the BUILD_SYSCALL_TEST variable is set, we should depend on the
# sub make output to do incremental building.
ifeq ($(BUILD_SYSCALL_TEST), 1)
$(INITRAMFS_FILELIST): | $(INITRAMFS_ALL_DIRS) $(SYSCALL_TEST_DIR)
else
$(INITRAMFS_FILELIST): | $(INITRAMFS_ALL_DIRS)
endif
	@(cd $(INITRAMFS); find . -printf "%T@ %p\n") > $(INITRAMFS_FILELIST)

$(EXT2_IMAGE):
	@dd if=/dev/zero of=$(EXT2_IMAGE) bs=2G count=1
	@mke2fs $(EXT2_IMAGE)

$(EXFAT_IMAGE):
	@dd if=/dev/zero of=$(EXFAT_IMAGE) bs=64M count=1
	@mkfs.exfat $(EXFAT_IMAGE)

$(RAID1_IMAGE0) $(RAID1_IMAGE1):
	@dd if=/dev/zero of=$(RAID1_IMAGE0) bs=128M count=1
	@mke2fs $(RAID1_IMAGE0)
	@dd if=$(RAID1_IMAGE0) of=$(RAID1_IMAGE1)

.PHONY: build
build: $(INITRAMFS_IMAGE) $(EXT2_IMAGE) $(EXFAT_IMAGE) $(RAID1_IMAGE0) $(RAID1_IMAGE1)

.PHONY: format
format:
	@$(MAKE) --no-print-directory -C apps format

.PHONY: check
check:
	@$(MAKE) --no-print-directory -C apps check

.PHONY: clean
clean:
	@rm -rf $(BUILD_DIR)
