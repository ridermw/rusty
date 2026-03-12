# Symphony Rust Constitution

## Core Principles

### I. Spec Fidelity
The Rust implementation must achieve full conformance with `SPEC.md` Sections 18.1 (Core Conformance) and 18.2 (Extension Conformance). The spec is the source of truth for behavior; the Elixir reference implementation is the source of truth for practical patterns. When in doubt, the spec wins.

### II. Idiomatic Rust
Use Rust's type system to encode invariants at compile time. Prefer `enum` over stringly-typed state, `Result<T, E>` over exceptions, and ownership over shared mutability. Use `tokio` for the async runtime. Avoid `unsafe` unless strictly necessary for FFI or performance-critical paths with documented safety proofs.

### III. Trait-Based Abstraction
Every external integration boundary (tracker, agent runner, workspace manager) must be defined as a trait. This enables testing via mock implementations and future extensibility (e.g., GitHub Issues tracker, different coding agents). The Elixir implementation's `Behaviour` pattern maps directly to Rust traits.

### IV. Test-Driven Development
Tests must be written before or alongside implementation. The test matrix in SPEC.md Section 17 defines the minimum coverage surface. Unit tests use mock trait implementations. Integration tests use a `MemoryTracker` and mock Codex process. The Elixir suite's 188 tests across 7 major test areas define the coverage baseline.

### V. Observability First
Structured logging with `tracing` is mandatory for all orchestrator decisions, agent lifecycle events, and error paths. Every log must include `issue_id`, `issue_identifier`, and `session_id` where applicable. The optional HTTP dashboard and terminal status surface must never affect orchestrator correctness.

### VI. Zero-Downtime Config Reload
`WORKFLOW.md` changes must be detected via filesystem watch and re-applied without restart. Invalid reloads must preserve last-known-good configuration. This is a core conformance requirement, not optional.

## Technology Stack

- **Language**: Rust 2021 edition, stable toolchain
- **Async Runtime**: `tokio` (multi-threaded)
- **HTTP Server**: `axum` (for optional dashboard/API)
- **Serialization**: `serde` + `serde_json` + `serde_yaml`
- **Templating**: `liquid` (Liquid-compatible, matching spec requirement)
- **HTTP Client**: `reqwest` (for GitHub REST/GraphQL API)
- **Logging**: `tracing` + `tracing-subscriber`
- **File Watching**: `notify`
- **CLI**: `clap`
- **Process Management**: `tokio::process`
- **Testing**: built-in `#[test]` + `tokio::test` + `mockall` for trait mocking

## Quality Gates

- `cargo clippy -- -D warnings` must pass with zero warnings
- `cargo test` must pass all tests
- `cargo fmt --check` must show no formatting issues
- No `unwrap()` in production code paths (use `?` or explicit error handling)
- All public APIs must have doc comments
- All error types must implement `std::error::Error` and provide actionable messages

## Governance

This constitution governs all development on the Rust Symphony implementation. It supersedes ad-hoc decisions. Amendments require updating this document and the spec feature files.

**Version**: 1.0.0 | **Ratified**: 2026-03-12 | **Last Amended**: 2026-03-12
