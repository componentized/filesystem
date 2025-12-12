SHELL := /bin/bash

export RUST_BACKTRACE ?= 1
export WASMTIME_BACKTRACE_DETAILS ?= 1

COMPONENTS = $(shell ls -1 components)

.PHONY: all
all: components

.PHONY: clean
clean:
	cargo clean
	rm -rf lib/*.wasm
	rm -rf lib/*.wasm.md

.PHONY: components
components: $(foreach component,$(COMPONENTS),lib/$(component).wasm $(foreach component,$(COMPONENTS),lib/$(component).debug.wasm))

define BUILD_COMPONENT

lib/$1.wasm: Cargo.toml Cargo.lock wit/deps $(shell find components/$1 -type f)
	cargo component build -p $1 --target wasm32-wasip2 --release
	cp target/wasm32-wasip2/release/$(subst -,_,$1).wasm lib/$1.wasm
	cp components/$1/README.md lib/$1.wasm.md

lib/$1.debug.wasm: Cargo.toml Cargo.lock wit/deps $(shell find components/$1 -type f)
	cargo component build -p $1 --target wasm32-wasip2
	cp target/wasm32-wasip2/debug/$(subst -,_,$1).wasm lib/$1.debug.wasm
	cp components/$1/README.md lib/$1.debug.wasm.md

endef

$(foreach component,$(COMPONENTS),$(eval $(call BUILD_COMPONENT,$(component))))


.PHONY: wit
wit: wit/deps

wit/deps: wkg.toml $(shell find wit -type f -name "*.wit" -not -path "deps")
	wkg wit fetch

.PHONY: publish
publish: $(shell find lib -type f -name "*.wasm" | sed -e 's:^lib/:publish-:g')

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

	wkg oci push \
        --annotation "org.opencontainers.image.title=${COMPONENT}" \
        --annotation "org.opencontainers.image.description=${DESCRIPTION}" \
        --annotation "org.opencontainers.image.version=${VERSION}" \
        --annotation "org.opencontainers.image.source=https://github.com/componentized/filesystem.git" \
        --annotation "org.opencontainers.image.revision=${REVISION}" \
        --annotation "org.opencontainers.image.licenses=Apache-2.0" \
        "${REPOSITORY}/${COMPONENT}:${TAG}" \
        "lib/${FILE}"
