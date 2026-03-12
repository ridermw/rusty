.PHONY: build test lint fmt check clean

build:
cargo build

release:
cargo build --release

test:
cargo test

lint:
cargo clippy -- -D warnings

fmt:
cargo fmt

check: fmt lint test

clean:
cargo clean
