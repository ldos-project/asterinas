# SPDX-License-Identifier: MPL-2.0

MAIN_MAKEFILE := $(firstword $(MAKEFILE_LIST))
INCLUDE_MAKEFILE := $(lastword $(MAKEFILE_LIST))
CUR_DIR := $(shell dirname $(realpath $(MAIN_MAKEFILE)))
INCLUDE_MAKEFILE_DIR := $(shell dirname $(realpath $(INCLUDE_MAKEFILE)))
COMMON_PREFIX := $(shell dirname $(INCLUDE_MAKEFILE_DIR))
CUR_DIR_NAME := $(subst $(COMMON_PREFIX)/,,$(CUR_DIR))

BUILD_DIR ?= $(INCLUDE_MAKEFILE_DIR)/../../../build
OUTPUT_DIR ?= $(BUILD_DIR)/initramfs/test
OBJ_OUTPUT_DIR := $(OUTPUT_DIR)/$(CUR_DIR_NAME)
DEP_OUTPUT_DIR := $(BUILD_DIR)/dep/$(CUR_DIR_NAME)
C_SRCS := $(wildcard *.c)
C_OBJS := $(addprefix $(OBJ_OUTPUT_DIR)/,$(C_SRCS:%.c=%))
C_DEPS := $(addprefix $(DEP_OUTPUT_DIR)/,$(C_SRCS:%.c=%.d))
ASM_SRCS := $(wildcard *.S)
ASM_OBJS := $(addprefix $(OBJ_OUTPUT_DIR)/,$(ASM_SRCS:%.S=%))
SCRIPTS := $(addprefix $(OBJ_OUTPUT_DIR)/, $(wildcard *.sh))
# Cargo discovers each `src/bin/*.rs` file as a binary target; mirror that convention here.
RUST_BIN_SRCS := $(wildcard src/bin/*.rs)
RUST_BINS := $(patsubst src/bin/%.rs,%,$(RUST_BIN_SRCS))
# Cargo discovers each `tests/*.rs` file as an integration-test target.
RUST_TEST_SRCS := $(wildcard tests/*.rs)
RUST_TESTS := $(patsubst tests/%.rs,%,$(RUST_TEST_SRCS))
RUST_SRCS := $(shell if test -f Cargo.toml; then find src tests -type f -name "*.rs" 2>/dev/null; fi)
RUST_BIN_OBJS := $(addprefix $(OBJ_OUTPUT_DIR)/,$(RUST_BINS))
RUST_TEST_OBJS := $(addprefix $(OBJ_OUTPUT_DIR)/,$(RUST_TESTS))
RUST_TARGET_DIR := $(BUILD_DIR)/rust/$(CUR_DIR_NAME)
RUST_TARGET_ARG := $(if $(RUST_TARGET),--target $(RUST_TARGET))
RUST_PROFILE_DIR := $(RUST_TARGET_DIR)/$(if $(RUST_TARGET),$(RUST_TARGET)/)release

CC ?= gcc
C_FLAGS += -Wall -Werror

HOST_PLATFORM ?= x86_64-linux
TEST_PLATFORM ?= asterinas

ifeq ($(TEST_PLATFORM),asterinas)
C_FLAGS += -D__asterinas__
endif

ifneq ($(C_OBJS_FILTER),)
    FILTERED_C_OBJS := $(addprefix $(OBJ_OUTPUT_DIR)/,$(C_OBJS_FILTER))
    C_OBJS := $(filter-out $(FILTERED_C_OBJS),$(C_OBJS))
endif

ifneq ($(ASM_OBJS_FILTER),)
    FILTERED_ASM_OBJS := $(addprefix $(OBJ_OUTPUT_DIR)/,$(ASM_OBJS_FILTER))
    ASM_OBJS := $(filter-out $(FILTERED_ASM_OBJS),$(ASM_OBJS))
endif

ifneq ($(SCRIPTS_FILTER),)
    FILTERED_SCRIPTS := $(addprefix $(OBJ_OUTPUT_DIR)/,$(SCRIPTS_FILTER))
    SCRIPTS := $(filter-out $(FILTERED_SCRIPTS),$(SCRIPTS))
endif

ifneq ($(SUBDIRS_FILTER),)
    SUBDIRS := $(filter-out $(SUBDIRS_FILTER),$(SUBDIRS))
endif

.PHONY: all
all: $(C_OBJS) $(ASM_OBJS) $(RUST_BIN_OBJS) $(RUST_TEST_OBJS) $(SUBDIRS) $(SCRIPTS)

$(OBJ_OUTPUT_DIR) $(DEP_OUTPUT_DIR):
	@mkdir -p $@

$(OBJ_OUTPUT_DIR)/%: %.c | $(OBJ_OUTPUT_DIR) $(DEP_OUTPUT_DIR)
	@$(CC) $(C_FLAGS) $< -o $@ $(EXTRA_C_FLAGS) \
		-MMD -MF $(DEP_OUTPUT_DIR)/$*.d
	@echo "CC <= $@"

-include $(C_DEPS)

$(OBJ_OUTPUT_DIR)/%: %.S | $(OBJ_OUTPUT_DIR)
	@$(CC) $(C_FLAGS) $(EXTRA_C_FLAGS) $< -o $@
	@echo "CC <= $@"

$(OBJ_OUTPUT_DIR)/%.sh: %.sh | $(OBJ_OUTPUT_DIR)
	@cp $< $@

ifneq ($(wildcard Cargo.toml),)
$(RUST_BIN_OBJS): $(OBJ_OUTPUT_DIR)/%: Cargo.toml Cargo.lock $(RUST_SRCS) | $(OBJ_OUTPUT_DIR)
	@cargo build --locked --release --manifest-path Cargo.toml \
		--target-dir $(RUST_TARGET_DIR) $(RUST_TARGET_ARG) --bin $*
	@cp $(RUST_PROFILE_DIR)/$* $@
	@echo "CARGO <= $@"

$(RUST_TEST_OBJS): $(OBJ_OUTPUT_DIR)/%: Cargo.toml Cargo.lock $(RUST_SRCS) | $(OBJ_OUTPUT_DIR)
	@cargo test --locked --release --no-run --manifest-path Cargo.toml \
		--target-dir $(RUST_TARGET_DIR) $(RUST_TARGET_ARG) --test $*
	@set -e; test_binary=; \
	for candidate in $(RUST_PROFILE_DIR)/deps/$*-*; do \
		if test -f "$$candidate" && test -x "$$candidate" && \
			{ test -z "$$test_binary" || test "$$candidate" -nt "$$test_binary"; }; then \
			test_binary="$$candidate"; \
		fi; \
	done; \
	test -n "$$test_binary"; \
	cp "$$test_binary" $@
	@echo "CARGO TEST <= $@"
endif

.PHONY: $(SUBDIRS)
$(SUBDIRS):
	@make --no-print-directory -C $@
