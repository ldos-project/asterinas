# SPDX-License-Identifier: MPL-2.0

# =========================== Makefile options. ===============================

# Global build options.
ARCH ?= x86_64
BENCHMARK ?= none
BOOT_METHOD ?= grub-rescue-iso
BOOT_PROTOCOL ?= multiboot2
BUILD_SYSCALL_TEST ?= 0
ENABLE_KVM ?= 1
KVM_EXISTS = $(shell test -c /dev/kvm && echo 1 || echo 0)
INTEL_TDX ?= 0
MEM ?= 8G
OVMF ?= on
RELEASE ?= 0
RELEASE_LTO ?= 0
LOG_LEVEL ?= error
SCHEME ?= ""
SMP ?= 1
OSTD_TASK_STACK_SIZE_IN_PAGES ?= 64
FEATURES ?=
ENABLE_RAID_TEST ?= 0
NO_DEFAULT_FEATURES ?= 0
# End of global build options.

# GDB debugging and profiling options.
GDB_TCP_PORT ?= 1234
GDB_PROFILE_FORMAT ?= flame-graph
GDB_PROFILE_COUNT ?= 200
GDB_PROFILE_INTERVAL ?= 0.1
# End of GDB options.

# The Makefile provides a way to run arbitrary tests in the kernel
# mode using the kernel command line.
# Here are the options for the auto test feature.
AUTO_TEST ?= none
EXTRA_BLOCKLISTS_DIRS ?= ""
SYSCALL_TEST_WORKDIR ?= /tmp
# End of auto test features.

# Network settings
# NETDEV possible values are user,tap
NETDEV ?= user
VHOST ?= off
# End of network settings

# Docker settings
# Name that the long-running container will use
DOCKER_CONTAINER_NAME ?= mariposa-$(shell whoami)
# End of docker settings

# Rust cache
DOCKER_RUST_CACHE_LOCATION ?= $(shell pwd)/.cache

# ========================= End of Makefile options. ==========================

SHELL := /bin/bash

CARGO_OSDK := ~/.cargo/bin/cargo-osdk

# Common arguments for `cargo osdk` `build`, `run` and `test` commands.
CARGO_OSDK_COMMON_ARGS := --target-arch=$(ARCH)
# The build arguments also apply to the `cargo osdk run` command.
CARGO_OSDK_BUILD_ARGS := --kcmd-args="ostd.log_level=$(LOG_LEVEL)"
CARGO_OSDK_TEST_ARGS :=

# Rust Cache
CARGO_CACHE := $(DOCKER_RUST_CACHE_LOCATION)/cargo
RUSTUP_CACHE := $(DOCKER_RUST_CACHE_LOCATION)/rustup

# Persistent Bash history - .bash_history is persisted between container invocations to make
# development easier.
BASH_HISTORY := $(DOCKER_RUST_CACHE_LOCATION)/.bash_history

# Docker Configs
DOCKER_TAG := ldosproject/asterinas
DOCKER_IMAGE := $(shell cat DOCKER_IMAGE_VERSION)
DOCKER_IMAGE_TAG := $(DOCKER_TAG):$(DOCKER_IMAGE)
DOCKER_RUN_ARGS := --privileged --device=/dev/kvm
DOCKER_MOUNTS := -v $(shell pwd):/root/asterinas -v $(CARGO_CACHE):/root/.cargo -v $(RUSTUP_CACHE):/root/.rustup
DOCKER_MOUNTS += -v $(BASH_HISTORY):/root/.bash_history

ifeq ($(AUTO_TEST), syscall)
BUILD_SYSCALL_TEST := 1
CARGO_OSDK_BUILD_ARGS += --kcmd-args="SYSCALL_TEST_SUITE=$(SYSCALL_TEST_SUITE)"
CARGO_OSDK_BUILD_ARGS += --kcmd-args="SYSCALL_TEST_WORKDIR=$(SYSCALL_TEST_WORKDIR)"
CARGO_OSDK_BUILD_ARGS += --kcmd-args="EXTRA_BLOCKLISTS_DIRS=$(EXTRA_BLOCKLISTS_DIRS)"
CARGO_OSDK_BUILD_ARGS += --init-args="/opt/syscall_test/run_syscall_test.sh"
else ifeq ($(AUTO_TEST), test)
	ifneq ($(SMP), 1)
		CARGO_OSDK_BUILD_ARGS += --kcmd-args="BLOCK_UNSUPPORTED_SMP_TESTS=1"
	endif
CARGO_OSDK_BUILD_ARGS += --init-args="/test/run_general_test.sh"
else ifeq ($(AUTO_TEST), raid)
CARGO_OSDK_BUILD_ARGS += --init-args="/test/raid1.sh"
else ifeq ($(AUTO_TEST), boot)
CARGO_OSDK_BUILD_ARGS += --init-args="/test/boot_hello.sh"
else ifeq ($(AUTO_TEST), vsock)
export VSOCK=on
CARGO_OSDK_BUILD_ARGS += --init-args="/test/run_vsock_test.sh"
endif

ifeq ($(RELEASE_LTO), 1)
CARGO_OSDK_COMMON_ARGS += --profile release-lto
OSTD_TASK_STACK_SIZE_IN_PAGES = 8
else ifeq ($(RELEASE), 1)
CARGO_OSDK_COMMON_ARGS += --release
OSTD_TASK_STACK_SIZE_IN_PAGES = 8
endif

# If the BENCHMARK is set, we will run the benchmark in the kernel mode.
ifneq ($(BENCHMARK), none)
CARGO_OSDK_BUILD_ARGS += --init-args="/benchmark/common/bench_runner.sh $(BENCHMARK) asterinas"
endif

# If INITARGS is set, it will be passed as arguments to the init process in the VM. By default it is a command to be
# launched instead of the shell.
INITARGS ?=
CARGO_OSDK_BUILD_ARGS += $(foreach ARG, $(INITARGS), --init-args="$(ARG)")

# If KCMDARGS is set, it will be passed as kernel command line args. In the kernel you can access these via KCmdlineArg.
KCMDARGS ?=
CARGO_OSDK_BUILD_ARGS += $(foreach ARG, $(KCMDARGS), --kcmd-args="$(ARG)")

ifeq ($(INTEL_TDX), 1)
BOOT_METHOD = grub-qcow2
BOOT_PROTOCOL = linux-efi-handover64
CARGO_OSDK_COMMON_ARGS += --scheme tdx
endif

ifeq ($(BOOT_PROTOCOL), linux-legacy32)
BOOT_METHOD = qemu-direct
OVMF = off
else ifeq ($(BOOT_PROTOCOL), multiboot)
BOOT_METHOD = qemu-direct
OVMF = off
endif

ifeq ($(ARCH), riscv64)
SCHEME = riscv
endif

ifneq ($(SCHEME), "")
CARGO_OSDK_COMMON_ARGS += --scheme $(SCHEME)
else
CARGO_OSDK_COMMON_ARGS += --boot-method="$(BOOT_METHOD)"
endif

# Feature for RAID test.
ifeq ($(ENABLE_RAID_TEST),1)
FEATURES := $(strip $(FEATURES) raid_test)
endif

ifdef FEATURES
CARGO_OSDK_COMMON_ARGS += --features="$(FEATURES)"
endif
ifeq ($(NO_DEFAULT_FEATURES), 1)
CARGO_OSDK_COMMON_ARGS += --no-default-features
endif

# To test the linux-efi-handover64 boot protocol, we need to use Debian's
# GRUB release, which is installed in /usr/bin in our Docker image.
ifeq ($(BOOT_PROTOCOL), linux-efi-handover64)
CARGO_OSDK_COMMON_ARGS += --grub-mkrescue=/usr/bin/grub-mkrescue --grub-boot-protocol="linux"
else ifeq ($(BOOT_PROTOCOL), linux-efi-pe64)
CARGO_OSDK_COMMON_ARGS += --grub-boot-protocol="linux"
else ifeq ($(BOOT_PROTOCOL), linux-legacy32)
CARGO_OSDK_COMMON_ARGS += --linux-x86-legacy-boot --grub-boot-protocol="linux"
else
CARGO_OSDK_COMMON_ARGS += --grub-boot-protocol=$(BOOT_PROTOCOL)
endif

ifeq ($(ENABLE_KVM), 1)
	ifeq ($(KVM_EXISTS), 1)
		ifeq ($(ARCH), x86_64)
			CARGO_OSDK_COMMON_ARGS += --qemu-args="-accel kvm"
		endif
	endif
endif

# Skip GZIP to make encoding and decoding of initramfs faster
ifeq ($(INITRAMFS_SKIP_GZIP),1)
CARGO_OSDK_INITRAMFS_OPTION := --initramfs=$(realpath test/build/initramfs.cpio)
CARGO_OSDK_COMMON_ARGS += $(CARGO_OSDK_INITRAMFS_OPTION)
endif

CARGO_OSDK_BUILD_ARGS += $(CARGO_OSDK_COMMON_ARGS)
CARGO_OSDK_TEST_ARGS += $(CARGO_OSDK_COMMON_ARGS)

# Pass make variables to all subdirectory makes
export

# Basically, non-OSDK crates do not depend on Aster Frame and can be checked
# or tested without OSDK.
NON_OSDK_CRATES := \
	ostd/libs/align_ext \
	ostd/libs/id-alloc \
	ostd/libs/linux-bzimage/builder \
	ostd/libs/linux-bzimage/boot-params \
	ostd/libs/ostd-macros \
	ostd/libs/ostd-test \
	kernel/libs/cpio-decoder \
	kernel/libs/int-to-c-enum \
	kernel/libs/int-to-c-enum/derive \
	kernel/libs/aster-rights \
	kernel/libs/aster-rights-proc \
	kernel/libs/jhash \
	kernel/libs/keyable-arc \
	kernel/libs/typeflags \
	kernel/libs/typeflags-util \
	kernel/libs/atomic-integer-wrapper

# In contrast, OSDK crates depend on OSTD (or being `ostd` itself)
# and need to be built or tested with OSDK.
OSDK_CRATES := \
	osdk/deps/frame-allocator \
	osdk/deps/heap-allocator \
	osdk/deps/test-kernel \
	ostd \
	ostd/libs/linux-bzimage/setup \
	ostd/tests/early-boot-test-kernel \
	kernel \
	kernel/comps/block \
	kernel/comps/console \
	kernel/comps/framebuffer \
	kernel/comps/input \
	kernel/comps/keyboard \
	kernel/comps/network \
	kernel/comps/raid \
	kernel/comps/softirq \
	kernel/comps/systree \
	kernel/comps/logger \
	kernel/comps/mlsdisk \
	kernel/comps/time \
	kernel/comps/virtio \
	kernel/libs/aster-util \
	kernel/libs/aster-bigtcp \
	kernel/libs/xarray

# OSDK dependencies
OSDK_SRC_FILES := \
	$(shell find osdk/Cargo.toml osdk/Cargo.lock osdk/src -type f)

.PHONY: all
all: build

# Install or update OSDK from source
# To uninstall, do `cargo uninstall cargo-osdk`
.PHONY: install_osdk
install_osdk:
	@# The `OSDK_LOCAL_DEV` environment variable is used for local development
	@# without the need to publish the changes of OSDK's self-hosted
	@# dependencies to `crates.io`.
	OSDK_LOCAL_DEV=1 cargo install cargo-osdk --path osdk

# This will install and update OSDK automatically
$(CARGO_OSDK): $(OSDK_SRC_FILES)
	@$(MAKE) --no-print-directory install_osdk

.PHONY: check_osdk
check_osdk:
	cd osdk && cargo clippy -- -D warnings

.PHONY: test_osdk
test_osdk:
	cd osdk && \
		OSDK_LOCAL_DEV=1 cargo build && \
		OSDK_LOCAL_DEV=1 cargo test

.PHONY: initramfs
initramfs:
	@$(MAKE) --no-print-directory -C test

.PHONY: build
build: initramfs $(CARGO_OSDK)
	cd kernel && cargo osdk build $(CARGO_OSDK_BUILD_ARGS)

.PHONY: tools
tools:
	cd kernel/libs/comp-sys && cargo install --path cargo-component

.PHONY: run
run: initramfs $(CARGO_OSDK)
	@[ $(ENABLE_KVM) -eq 1 ] && \
		([ $(KVM_EXISTS) -eq 1 ] || \
			echo Warning: KVM not present on your system)
	cd kernel && cargo osdk run $(CARGO_OSDK_BUILD_ARGS)

.PHONY: run_dropbear
run_dropbear: initramfs $(CARGO_OSDK)
	@[ $(ENABLE_KVM) -eq 1 ] && \
		([ $(KVM_EXISTS) -eq 1 ] || \
			echo Warning: KVM not present on your system)
	cd kernel && cargo osdk run $(CARGO_OSDK_BUILD_ARGS) --init-args="/service/start_dropbear.sh"

# Check the running status of auto tests from the QEMU log
ifeq ($(AUTO_TEST), syscall)
	@tail --lines 100 qemu.log | grep -q "^All syscall tests passed." \
		|| (echo "Syscall test failed" && exit 1)
else ifeq ($(AUTO_TEST), test)
	@tail --lines 100 qemu.log | grep -q "^All general tests passed." \
		|| (echo "General test failed" && exit 1)
else ifeq ($(AUTO_TEST), boot)
	@tail --lines 100 qemu.log | grep -q "^Successfully booted." \
		|| (echo "Boot test failed" && exit 1)
else ifeq ($(AUTO_TEST), raid)
	@tail --lines 100 qemu.log | grep -q "^All raid1 test passed" \
		|| (echo "RAID test failed" && exit 1)
else ifeq ($(AUTO_TEST), vsock)
	@tail --lines 100 qemu.log | grep -q "^Vsock test passed." \
		|| (echo "Vsock test failed" && exit 1)
endif

.PHONY:
kill_qemu:
	pkill qemu-system-x86

.PHONY: gdb_server
gdb_server: initramfs $(CARGO_OSDK)
	@[ $(ENABLE_KVM) -eq 1 ] && \
		([ $(KVM_EXISTS) -eq 1 ] || \
			echo Warning: KVM not present on your system)
	@cd kernel && cargo osdk run $(CARGO_OSDK_BUILD_ARGS) --gdb-server wait-client,vscode,addr=:$(GDB_TCP_PORT)

.PHONY: gdb_client
gdb_client: initramfs $(CARGO_OSDK)
	@[ $(ENABLE_KVM) -eq 1 ] && \
		([ $(KVM_EXISTS) -eq 1 ] || \
			echo Warning: KVM not present on your system)
	@cd kernel && cargo osdk debug $(CARGO_OSDK_BUILD_ARGS) --remote :$(GDB_TCP_PORT)

.PHONY: profile_server
profile_server: initramfs $(CARGO_OSDK)
	@[ $(ENABLE_KVM) -eq 1 ] && \
		([ $(KVM_EXISTS) -eq 1 ] || \
			echo Warning: KVM not present on your system)
	@cd kernel && cargo osdk run $(CARGO_OSDK_BUILD_ARGS) --gdb-server addr=:$(GDB_TCP_PORT)

.PHONY: profile_client
profile_client: initramfs $(CARGO_OSDK)
	@cd kernel && cargo osdk profile $(CARGO_OSDK_BUILD_ARGS) --remote :$(GDB_TCP_PORT) \
		--samples $(GDB_PROFILE_COUNT) --interval $(GDB_PROFILE_INTERVAL) --format $(GDB_PROFILE_FORMAT)



# For each non-OSDK crate, invoke a rule which runs tests
NON_OSDK_TEST_TARGETS := $(foreach crate, $(NON_OSDK_CRATES), $(subst /,@@,$(crate)))

test_%:
	cd $(subst @@,/,$*) && cargo test

.PHONY: test_non_osdk
test: $(addprefix test_, $(NON_OSDK_TEST_TARGETS))

# For each OSDK crate, invoke a rule which runs ktests using OSDK
OSDK_KTEST_TARGETS := $(foreach crate, $(OSDK_CRATES), $(subst /,@@,$(crate)))

ktest_ostd@@libs@@linux-bzimage@@setup:
	@true

ktest_%: initramfs $(CARGO_OSDK)
	@dir=$(subst @@,/,$*); \
	echo "cd $$dir && cargo osdk test $(CARGO_OSDK_TEST_ARGS)"; \
	(cd $$dir && cargo osdk test $(CARGO_OSDK_TEST_ARGS)) || exit 1; \
	tail --lines 10 qemu.log | grep -q "^\\[ktest runner\\] All crates tested." \
		|| (echo "Test failed" && exit 1); \

.PHONY: ktest
ktest: $(addprefix ktest_, $(OSDK_KTEST_TARGETS))

.PHONY: early_boot_test
early_boot_test: initramfs $(CARGO_OSDK)
	@dir=ostd/tests/early-boot-test-kernel; \
	echo "cd $$dir && cargo osdk run"; \
	(cd $$dir && cargo osdk run) || exit 1; \

# For each non-OSDK crate, invoke a rule which runs docs
NON_OSDK_DOCS_TARGETS := $(foreach crate, $(NON_OSDK_CRATES), $(subst /,@@,$(crate)))

docs_non_osdk_%:
	cd $(subst @@,/,$*) && cargo doc --no-deps

.PHONY: docs_non_osdk
docs_non_osdk: $(addprefix docs_non_osdk_, $(NON_OSDK_DOCS_TARGETS))

# For each OSDK crate, invoke a rule which runs docs using OSDK
OSDK_DOCS_TARGETS := $(foreach crate, $(OSDK_CRATES), $(subst /,@@,$(crate)))

docs_osdk_%: $(CARGO_OSDK)
	cd $(subst @@,/,$*) && cargo osdk doc --no-deps

.PHONY: docs_osdk
docs_osdk: $(addprefix docs_osdk_, $(OSDK_DOCS_TARGETS))

# Combine the two into a single `docs` target and build mdbook
.PHONY: docs
docs: docs_non_osdk docs_osdk
	@echo "Building mdBook"
	cd docs && mdbook build

.PHONY: format
format:
	./tools/format_all.sh
	@$(MAKE) --no-print-directory -C test format

.PHONY: format_check lint_check clippy_check c_code_check typo_check

format_check:
	@# Check formatting issues of the Rust code
	./tools/format_all.sh --check


# Check that the makefile includes all projects.
.PHONY: workspace_project_lint
workspace_project_coverage_check:
	@# Check if the combination of STD_CRATES and NON_OSDK_CRATES is the
	@# same as all workspace members
	@sed -n '/^\[workspace\]/,/^\[.*\]/{/members = \[/,/\]/p}' Cargo.toml | \
		grep -v "members = \[" | tr -d '", \]' | \
		sort > /tmp/all_crates
	@echo $(NON_OSDK_CRATES) $(OSDK_CRATES) | tr ' ' '\n' | sort > /tmp/combined_crates
	@diff -B /tmp/all_crates /tmp/combined_crates || \
		(echo "Error: The combination of STD_CRATES and NOSTD_CRATES" \
			"is not the same as all workspace members" && exit 1)
	@rm /tmp/all_crates /tmp/combined_crates

# The following rules use check a list of subdirectories with the same command. This complex pattern is used instead of
# a bash level loop because this works correctly with `--keep-going`. The bash loop would exit after the first failure.
# It also also makes make level parallelism work. The pattern is:
# 
# * Depend a set of rules with the names `prefix_escaped_directory_name`
# * Create a pattern rule `prefix_%` which unescapes the matched suffix (which is available as $*), and performs the
#   checks.

# TODO: This pattern could be applied to some of the loops above (test, ktest, docs), but checks are by far the most
# important because it allows showing ALL check failures, not just the first.

# Target names with `/` replaced with `@@` since is `/` is special in make.
NON_OSDK_CRATE_TARGETS := $(foreach crate, $(NON_OSDK_CRATES), $(subst /,@@,$(crate)))
OSDK_CRATE_TARGETS := $(foreach crate, $(OSDK_CRATES), $(subst /,@@,$(crate)))

# For each crate, invoke a rule which checks that it inherits lints from the workspace.
.PHONY: lint_check
lint_check: $(addprefix lint_check_, $(NON_OSDK_CRATE_TARGETS) $(OSDK_CRATE_TARGETS))

lint_check_%:
	@# Convert matched tail back to a proper path by replacing `@@` with `/`
	@dir=$(subst @@,/,$*); \
	if [[ "$$(tail -2 $$dir/Cargo.toml)" != "[lints]"$$'\n'"workspace = true" ]]; then \
		echo "Error: Workspace lints in $$dir are not enabled"; \
		exit 1; \
	fi

# For each non-OSDK crate, invoke a rule which runs clippy
.PHONY: clippy_check_non_osdk 
clippy_check_non_osdk: $(addprefix clippy_check_non_osdk_, $(NON_OSDK_CRATE_TARGETS))

clippy_check_non_osdk_%:
	cd $(subst @@,/,$*) && cargo clippy -- -D warnings

# For each OSDK crate, invoke a rule which runs clippy
.PHONY: clippy_check_osdk
clippy_check_osdk: $(addprefix clippy_check_osdk_, $(OSDK_CRATE_TARGETS))

clippy_check_osdk_%:  $(CARGO_OSDK)
	cd $(subst @@,/,$*) && cargo osdk clippy -- -- -D warnings

.PHONY: clippy_check
clippy_check: clippy_check_non_osdk clippy_check_osdk

c_code_check:
	@# Check formatting issues of the C code (regression tests)
	@$(MAKE) --no-print-directory -C test check

typos_check:
	@# Check typos
	typos

check: initramfs format_check workspace_project_coverage_check lint_check clippy_check c_code_check typo_check

# Here we build our mount cache
# TODO make this rule depend on the dockerfile version see #34
${DOCKER_RUST_CACHE_LOCATION}/generated:
	mkdir -p ${DOCKER_RUST_CACHE_LOCATION}
	docker create --name dummy $(DOCKER_IMAGE_TAG)
	docker cp dummy:/root/.cargo ${CARGO_CACHE}
	docker cp dummy:/root/.rustup ${RUSTUP_CACHE}
	# Persistent bash history
	touch ${BASH_HISTORY}
	# Clean up dummy container
	docker rm dummy
	# Touch a file at the end to create some degree of "atomicity". If the cache
	# location exists but hasn't been initialized yet, we still need to run this
	# rule to initialize it.
	touch $@

.PHONY: docker
docker: | ${DOCKER_RUST_CACHE_LOCATION}/generated
	docker run --rm -it $(DOCKER_RUN_ARGS) $(DOCKER_MOUNTS) $(DOCKER_IMAGE_TAG)

.PHONY: docker_start
docker_start: | ${DOCKER_RUST_CACHE_LOCATION}/generated
	docker ps -a | grep $(DOCKER_CONTAINER_NAME) || \
		docker run -d --name $(DOCKER_CONTAINER_NAME) $(DOCKER_RUN_ARGS) $(DOCKER_MOUNTS) $(DOCKER_IMAGE_TAG) sleep infinity

.PHONY: docker_attach
docker_attach: docker_start
	docker exec -it $(DOCKER_CONTAINER_NAME) /bin/bash -i

.PHONY: docker_rm
docker_rm:
	docker stop $(DOCKER_CONTAINER_NAME)
	docker rm $(DOCKER_CONTAINER_NAME)

.PHONY: clean
clean:
	@if find target test osdk/target docs '!' -writable 2>/dev/null | grep -q .; then \
			echo "ERROR: Found unwritable files in build directories"; \
			echo "ERROR: This must be run from the same environment as the build (generally inside the container)."; \
			echo "ERROR: You may be running it in the host environment after a build in docker. Run it in docker instead."; \
			echo "ERROR: (Note: Using "sudo make clean" will not work as the host environment may not have the correct tools.)"; \
			false; \
		else \
			true; \
		fi
	@echo "Cleaning up Asterinas workspace target files"
	cargo clean
	@echo "Cleaning up OSDK workspace target files"
	cd osdk && cargo clean
	@echo "Cleaning up documentation target files"
	cd docs && mdbook clean
	@echo "Cleaning up test target files"
	@$(MAKE) --no-print-directory -C test clean
	@echo "Uninstalling OSDK"
	rm -f $(CARGO_OSDK)

.PHONY: clean_cache
clean_cache:
	@[ '!' -f /.dockerenv ] || ( \
		echo "ERROR: Cleaning the Cargo and rustup caches from within the container is not allowed. Run it from the host environment."; \
		false \
		)
	@if find $(DOCKER_RUST_CACHE_LOCATION) '!' -writable 2>/dev/null | grep -q .; then \
			echo "ERROR: Found unwritable files in cache directory."; \
			echo "ERROR: The container has probably created root owned files in cache. To fix that, run this as root: sudo make clean_cache"; \
			false; \
		else \
			true; \
		fi
	rm -rf $(DOCKER_RUST_CACHE_LOCATION)

.PHONY: clean_all
clean_all: clean clean_cache
