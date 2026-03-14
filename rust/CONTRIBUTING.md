# Contributing to Rusty

Thanks for your interest in contributing! This guide covers the basic dev setup to get you building, testing, and running Rusty locally.

## Prerequisites

- [Rust](https://rustup.rs/) stable toolchain
- [GitHub CLI](https://cli.github.com/) (`gh`)
- [Copilot CLI](https://docs.github.com/en/copilot/github-copilot-in-the-cli) (`copilot`)
- GitHub authentication via `GITHUB_TOKEN`, `GH_TOKEN`, or `gh auth login`

## Clone

```bash
git clone https://github.com/ridermw/rusty.git
cd rusty/rust
```

## Build

```bash
# Debug build (fast compile)
cargo build

# Release build (optimized)
cargo build --release
```

## Test

```bash
# Run all tests
cargo test

# Lint
cargo clippy -- -D warnings

# Format check
cargo fmt --check
```

Or use the Makefile:

```bash
make check   # fmt + lint + test
```

## Run Locally

1. Verify your environment:

   ```bash
   cargo run -- setup
   ```

2. Start the daemon:

   ```bash
   cargo run -- run --yolo
   ```

   Or with the web dashboard:

   ```bash
   cargo run -- run --yolo --port 4000
   ```

## Workflow

1. Create a feature branch from `main`
2. Make your changes
3. Run `make check` (or `cargo fmt && cargo clippy -- -D warnings && cargo test`)
4. Commit and open a pull request
