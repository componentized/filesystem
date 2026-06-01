SHELL := /bin/bash

export RUST_BACKTRACE ?= 1
export WASMTIME_BACKTRACE_DETAILS ?= 1
export WKG_CONFIG_FILE := $(abspath .config/wasm-pkg/config.toml)

# `make` without a goal runs all, otherwise the first target of an included file would run
.DEFAULT_GOAL := all

# tools first, the others use the variables it defines for each tool when their rules are read
include make/tools.mk
include make/wit.mk
include make/components.mk
include make/publish.mk


.PHONY: all
all: tools wit components test

.PHONY: clean
clean: clean-components clean-wit 
	@:

.PHONY: clean-all
clean-all: clean-components clean-tools clean-wit
	cargo clean

define TEST_COMPONENT

lib/tests/$1.wasm: Cargo.toml Cargo.lock wit/deps $(shell find tests/$1 -type f)
	cargo component build -p $1 --target wasm32-wasip2 --release
	cp target/wasm32-wasip2/release/$1.wasm lib/tests/$1.wasm

lib/tests/$1.debug.wasm: Cargo.toml Cargo.lock wit/deps $(shell find tests/$1 -type f)
	cargo component build -p $1 --target wasm32-wasip2
	cp target/wasm32-wasip2/debug/$1.wasm lib/tests/$1.debug.wasm

endef

$(foreach component,$(TEST_COMPONENTS),$(eval $(call TEST_COMPONENT,$(component))))

lib/tests/logging-to-stdout.wasm:
	wkg oci pull ghcr.io/componentized/logging/to-stdout:v0.2.1 -o "lib/tests/logging-to-stdout.wasm"

lib/tests/permit.wasm: lib/gate.wasm lib/latch-permit-all.wasm
	wac plug lib/gate.wasm \
		--plug lib/latch-permit-all.wasm \
		-o lib/tests/permit.wasm

lib/tests/filesystem-cli-permit.wasm: lib/tests/filesystem-cli.wasm lib/tests/permit.wasm lib/tests/logging-to-stdout.wasm
	wac plug lib/tests/filesystem-cli.wasm \
		--plug <( \
			wac plug lib/tests/permit.wasm \
				--plug lib/tests/logging-to-stdout.wasm \
		) \
		-o lib/tests/filesystem-cli-permit.wasm

lib/tests/deny.wasm: lib/gate.wasm lib/latch-deny-all.wasm
	wac plug lib/gate.wasm \
		--plug lib/latch-deny-all.wasm \
		-o lib/tests/deny.wasm

lib/tests/filesystem-cli-deny.wasm: lib/tests/filesystem-cli.wasm lib/tests/deny.wasm lib/tests/logging-to-stdout.wasm
	wac plug lib/tests/filesystem-cli.wasm \
		--plug <( \
			wac plug lib/tests/deny.wasm \
				--plug lib/tests/logging-to-stdout.wasm \
		) \
		-o lib/tests/filesystem-cli-deny.wasm

lib/tests/readonly.wasm: tests/readonly.wac lib/gate.wasm lib/latch-n2.wasm lib/latch-readonly.wasm lib/latch-permit-all.wasm
	wac compose -o lib/tests/readonly.wasm \
		-d componentized:gate="lib/gate.wasm" \
		-d componentized:latch-n2="lib/latch-n2.wasm" \
		-d componentized:latch-readonly="lib/latch-readonly.wasm" \
		-d componentized:latch-permit="lib/latch-permit-all.wasm" \
		tests/readonly.wac

lib/tests/filesystem-cli-readonly.wasm: lib/tests/filesystem-cli.wasm lib/tests/readonly.wasm lib/tests/logging-to-stdout.wasm
	wac plug lib/tests/filesystem-cli.wasm \
		--plug <( \
			wac plug lib/tests/readonly.wasm \
				--plug lib/tests/logging-to-stdout.wasm \
		) \
		-o lib/tests/filesystem-cli-readonly.wasm

.PHONY: tests
tests: $(foreach component,$(TEST_COMPONENTS),lib/tests/$(component).wasm) lib/tests/filesystem-cli-permit.wasm lib/tests/filesystem-cli-deny.wasm lib/tests/filesystem-cli-readonly.wasm

.PHONY: test
test: components
	cargo test --workspace
