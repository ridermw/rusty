.PHONY: build release test lint fmt check clean setup

build:
	cargo build
	@echo "Binary: target/debug/rusty"

release:
	cargo build --release
	@echo "Binary: target/release/rusty"
	@echo "Copy WORKFLOW.md to the release directory if needed:"
	@echo "  cp WORKFLOW.md target/release/"

test:
	cargo test

lint:
	cargo clippy -- -D warnings

fmt:
	cargo fmt

check: fmt lint test

clean:
	cargo clean

setup: build
	@echo "Running setup check..."
	target/debug/rusty setup
