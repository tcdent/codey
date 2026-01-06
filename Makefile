.PHONY: build run release profile clean patch

# Set to 0 to use upstream crates: make SIMD=0 build
SIMD ?= 1

ifeq ($(SIMD),1)
PATCH_DEPS := lib/ratatui-core/.patched lib/genai/.patched .cargo/config.toml
else
PATCH_DEPS := lib/genai/.patched .cargo/config.toml
endif

build: $(PATCH_DEPS)
	cargo build

run: $(PATCH_DEPS)
	cargo run

release: $(PATCH_DEPS)
ifdef CARGO_BUILD_TARGET
	cargo build --release --target $(CARGO_BUILD_TARGET)
else
	cargo build --release
endif

profile: release
	samply record ./target/release/codey

clean: clean-config
	cargo clean
	rm -rf lib/ratatui-core lib/genai

clean-config:
	rm -f .cargo/config.toml
	rmdir .cargo 2>/dev/null || true

patch: lib/ratatui-core/.patched lib/genai/.patched .cargo/config.toml

lib/ratatui-core/.patched: lib/patches/ratatui-core/*
	./lib/patches/ratatui-core/apply.sh

lib/genai/.patched: lib/patches/genai/*
	./lib/patches/genai/apply.sh

.cargo/config.toml: lib/ratatui-core/.patched lib/genai/.patched
	mkdir -p .cargo
	echo '[patch.crates-io]' > $@
ifeq ($(SIMD),1)
	echo 'ratatui-core = { path = "lib/ratatui-core" }' >> $@
endif
	echo 'genai = { path = "lib/genai" }' >> $@
