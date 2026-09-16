MANIFEST := contracts/Cargo.toml
# wasm32v1-none, not wasm32-unknown-unknown: recent Rust enables wasm features
# on the latter (reference-types) that the Soroban VM rejects at deploy time.
TARGET   := wasm32v1-none

default: build

all: fmt-check lint test build

build:
	cargo build --manifest-path $(MANIFEST) --target $(TARGET) --release

test:
	cargo test --manifest-path $(MANIFEST)

lint:
	cargo clippy --manifest-path $(MANIFEST) --all-targets -- -D warnings

fmt:
	cargo fmt --manifest-path $(MANIFEST) --all

fmt-check:
	cargo fmt --manifest-path $(MANIFEST) --all -- --check

audit:
	cargo audit --file contracts/Cargo.lock

sizes: build
	@ls -l contracts/target/$(TARGET)/release/*.wasm | awk '{printf "%-40s %8d bytes\n", $$9, $$5}'

clean:
	cargo clean --manifest-path $(MANIFEST)

.PHONY: default all build test lint fmt fmt-check audit sizes clean
