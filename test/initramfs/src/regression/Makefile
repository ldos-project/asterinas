# SPDX-License-Identifier: MPL-2.0

HOST_PLATFORM ?= x86_64-linux

# Prevent the implicit rules from compiling ".c" or ".s" files automatically.
MAKEFLAGS += --no-builtin-rules

MKFILE_PATH := $(abspath $(lastword $(MAKEFILE_LIST)))
CUR_DIR := $(patsubst %/,%,$(dir $(MKFILE_PATH)))
BUILD_DIR ?= $(CUR_DIR)/../../build
OUTPUT_DIR ?= $(BUILD_DIR)/initramfs/test

# REMINDER: Whenever you change this list, update the corresponding `default.nix` file.
SUBDIRS := \
	device \
	fs \
	hello_world \
	io \
	ipc \
	memory \
	network \
	oqfs \
	process \
	scripts \
	security \
	time \
	raid1 \

ifeq ($(HOST_PLATFORM), x86_64-linux)
SUBDIRS += intel_tdx
endif

# The C header and source files of all regression tests.
C_SOURCES := $(shell find . -type f \( -name "*.c" -or -name "*.h" \))
# The Cargo manifests of all Rust regression tests.
RUST_MANIFESTS := $(shell find . -type f -name "Cargo.toml")

.PHONY: all
all: $(SUBDIRS)

$(OUTPUT_DIR):
	@mkdir -p $@

.PHONY: $(SUBDIRS)
$(SUBDIRS): $(OUTPUT_DIR)
	@make --no-print-directory BUILD_DIR=$(BUILD_DIR) OUTPUT_DIR=$(OUTPUT_DIR) -C $@

.PHONY: format
format:
	@echo "Fixing code format for regression tests..."
	@clang-format -i $(C_SOURCES)
	@for manifest in $(RUST_MANIFESTS); do cargo fmt --manifest-path $$manifest; done

.PHONY: check
check:
	@echo "Checking code format for regression tests..."
	@clang-format --dry-run --Werror $(C_SOURCES)
	@for manifest in $(RUST_MANIFESTS); do cargo fmt --check --manifest-path $$manifest; done
