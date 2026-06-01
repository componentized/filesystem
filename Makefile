SHELL := /bin/bash

export RUST_BACKTRACE ?= 1
export WASMTIME_BACKTRACE_DETAILS ?= 1

COMPONENTS = $(shell ls -1 components)
TEST_COMPONENTS = $(shell ls -1 tests | grep -v '\.wac')

.PHONY: all
all: components

.PHONY: clean
clean:
	cargo clean
	rm -rf lib/*.wasm
	rm -rf lib/*.wasm.md
	rm -rf lib/tests/*.wasm
	rm -rf lib/tests/*.wasm.md

.PHONY: components
components: $(foreach component,$(COMPONENTS),lib/$(component).wasm) $(foreach component,$(COMPONENTS),lib/$(component).debug.wasm)

define BUILD_COMPONENT

.PHONY: components/$1
components/$1: lib/$1.wasm lib/$1.debug.wasm

lib/$1.wasm: Cargo.toml Cargo.lock wit/deps $(shell find components/$1 -type f)
	cargo build -p $1 --target wasm32-unknown-unknown --release
	wasm-tools component new target/wasm32-unknown-unknown/release/$(subst -,_,$1).wasm -o lib/$1.wasm
	cp components/$1/README.md lib/$1.wasm.md

lib/$1.debug.wasm: Cargo.toml Cargo.lock wit/deps $(shell find components/$1 -type f)
	cargo build -p $1 --target wasm32-unknown-unknown
	wasm-tools component new target/wasm32-unknown-unknown/debug/$(subst -,_,$1).wasm -o lib/$1.debug.wasm
	cp components/$1/README.md lib/$1.debug.wasm.md

endef

$(foreach component,$(COMPONENTS),$(eval $(call BUILD_COMPONENT,$(component))))

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

.PHONY: wit
wit: wit/deps

wit/deps: wkg.toml $(shell find wit -type f -name "*.wit" -not -path "deps")
	wkg wit fetch

.PHONY: publish
publish: $(shell find lib -type f -name "*.wasm" -maxdepth 1 | sed -e 's:^lib/:publish-:g')

.PHONY: publish-%
publish-%:
ifndef VERSION
	$(error VERSION is undefined)
endif
ifndef REPOSITORY
	$(error REPOSITORY is undefined)
endif
	@$(eval FILE := $(@:publish-%=%))
	@$(eval COMPONENT := $(FILE:%.wasm=%))
	@$(eval DESCRIPTION := $(shell head -n 3 "lib/${FILE}.md" | tail -n 1))
	@$(eval REVISION := $(shell git rev-parse HEAD)$(shell git diff --quiet HEAD && echo "+dirty"))
	@$(eval TAG := $(shell echo "${VERSION}" | sed 's/[^a-zA-Z0-9_.\-]/--/g'))

	@echo "::group::${FILE} -> ${REPOSITORY}/${COMPONENT}:${TAG}"
	@DIGEST=$$( \
		wkg oci push \
			--annotation "org.opencontainers.image.title=${COMPONENT}" \
			--annotation "org.opencontainers.image.description=${DESCRIPTION}" \
			--annotation "org.opencontainers.image.version=${VERSION}" \
			--annotation "org.opencontainers.image.source=https://github.com/${GITHUB_REPOSITORY}.git" \
			--annotation "org.opencontainers.image.revision=${REVISION}" \
			--annotation "org.opencontainers.image.licenses=Apache-2.0" \
			"${REPOSITORY}/${COMPONENT}:${TAG}" \
			"lib/${FILE}" \
			2>&1 \
			| tee /dev/stderr \
			| grep -o 'sha256:[a-f0-9]\{64\}' \
	) ; \
	cosign sign --yes "${REPOSITORY}/${COMPONENT}:${TAG}@$${DIGEST}"
	@echo "::endgroup::"
