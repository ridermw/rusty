# Symphony Rust

A Rust implementation of Symphony with full feature parity to the Elixir reference, adapted for **GitHub Issues** and **GitHub Copilot CLI in ACP mode**.

> [!WARNING]
> This implementation is specified but not yet built. See `.specify/features/symphony-rust/` for the full spec, plan, and task breakdown.

## How it differs from the Elixir reference

| Aspect | Elixir reference | Rust implementation |
|---|---|---|
| Issue tracker | Linear (GraphQL) | **GitHub Issues** (REST + GraphQL) |
| Auth | `LINEAR_API_KEY` | `GITHUB_TOKEN` |
| Tracker config | `tracker.project_slug` | `tracker.repo` (`owner/repo`) |
| Coding agent | Codex app-server | **Copilot CLI** (`copilot --acp --stdio`) |
| Agent protocol | JSON-RPC 2.0 over stdio | Same — JSON-RPC 2.0 over stdio (ACP) |
| Dynamic tool | `linear_graphql` | **`github_graphql`** |
| Runtime | Elixir/OTP/BEAM | **Single static binary**, no runtime deps |

Tracker and agent boundaries are traits — adding Linear, Codex, Jira, or any other adapter is straightforward without touching the orchestrator.

## Goals

1. **Single static binary** — no runtime dependencies, deploy anywhere
2. **Lower memory footprint** — important when running many concurrent agent sessions
3. **Broader adoption** — Rust is more widely known than Elixir in the coding agent ecosystem
4. **Full SPEC conformance** — both Core Conformance (§18.1) and Extension Conformance (§18.2)

## Design Principles

1. **Spec Fidelity** — `SPEC.md` is the source of truth for behavior; the Elixir implementation is the source of truth for practical patterns
2. **Idiomatic Rust** — enums over stringly-typed state, `Result<T,E>` over exceptions, ownership over shared mutability
3. **Trait-Based Abstraction** — every external boundary (`Tracker`, `WorkspaceManager`, `AgentSession`) is a trait
4. **Test-Driven Development** — tests before or alongside implementation; 188+ test baseline from the Elixir suite
5. **Observability First** — `tracing`-based structured logging with `issue_id`, `issue_identifier`, `session_id` on every relevant event
6. **Zero-Downtime Config Reload** — `WORKFLOW.md` changes detected and applied without restart (core conformance, not optional)

## Architecture

The orchestrator runs as a **single async task** owning all mutable state (no `Arc<Mutex<_>>`). Workers communicate back via `mpsc` channels. This preserves the single-authority invariant from the spec.

```
GitHub Issues API → tracker/github/  → Issue (normalized)
                                           ↓
                                  orchestrator/ (tokio task, mpsc channel)
                                           ↓
                                  agent/ (JoinSet task per issue)
                                    ├─ workspace/  (create, hooks, cleanup)
                                    ├─ prompt.rs   (Liquid template)
                                    └─ acp_client  (Copilot CLI, JSON-RPC stdio)
                                         └─ dynamic_tool  (github_graphql)

Config: WORKFLOW.md → workflow/ → config/ (typed accessors, hot reload via notify)
Web:    server/ (axum — optional, port-gated)
```

**Core traits:**

```rust
#[async_trait]
pub trait Tracker: Send + Sync {
    async fn fetch_candidate_issues(&self, config: &TrackerConfig) -> Result<Vec<Issue>>;
    async fn fetch_issue_states_by_ids(&self, ids: &[String]) -> Result<Vec<Issue>>;
    async fn fetch_issues_by_states(&self, states: &[String], config: &TrackerConfig) -> Result<Vec<Issue>>;
}

pub trait WorkspaceManager: Send + Sync { /* create, remove, run_hook */ }

#[async_trait]
pub trait AgentSession: Send { /* initialize, start_thread, start_turn, stream_events, stop */ }
```

## Technology Stack

| Purpose | Crate | Replaces (Elixir) |
|---|---|---|
| Async runtime | `tokio` | BEAM/OTP |
| HTTP server | `axum` | Phoenix/Bandit |
| HTTP client | `reqwest` | Req |
| Serialization | `serde` + `serde_json` + `serde_yaml` | Jason + YamlElixir |
| Templating | `liquid` | Solid |
| Logging | `tracing` + `tracing-subscriber` | Logger + :logger_disk_log |
| File watching | `notify` | GenServer polling |
| CLI | `clap` | OptionParser |
| Process mgmt | `tokio::process` | Port |
| Testing | `mockall` | ExUnit + custom mocks |

## Configuration

Same `WORKFLOW.md` format as the Elixir implementation (YAML front matter + Liquid prompt body), with GitHub-specific fields:

```yaml
---
tracker:
  kind: github
  repo: "owner/repo"           # required — replaces project_slug
  api_key: $GITHUB_TOKEN       # defaults to GITHUB_TOKEN env var
  active_states: ["open"]      # default
  terminal_states: ["closed"]  # default
  labels: []                   # optional label filters
workspace:
  root: ~/symphony-workspaces
hooks:
  after_create: |
    git clone git@github.com:owner/repo.git .
agent:
  max_concurrent_agents: 10
  max_turns: 20
  command: "copilot --acp --stdio"   # default
  approval_policy: "auto-approve"    # default
---

You are working on GitHub issue {{ issue.identifier }}.

Title: {{ issue.title }}
Body: {{ issue.description }}
```

**CLI:**

```bash
./symphony WORKFLOW.md --i-understand-that-this-will-be-running-without-the-usual-guardrails
./symphony WORKFLOW.md --port 4000 --logs-root ./log
```

## Implementation Phases

| Phase | Stories | Status |
|---|---|---|
| 1. Foundation | Project skeleton, config schema, workflow loader, prompt builder | Spec complete |
| 2. Tracker Integration | `Tracker` trait + `MemoryTracker`, GitHub Issues client | Spec complete |
| 3. Workspace Management | Path safety, workspace manager, hooks | Spec complete |
| 4. Orchestrator Core | State machine, dispatch, reconciliation, retry | Spec complete |
| 5. Agent Integration | Copilot CLI ACP client, `github_graphql` tool | Spec complete |
| 6. Observability | Structured logging, terminal dashboard, HTTP API | Spec complete |
| 7. Extensions | SSH worker pool, `github_graphql` dynamic tool | Spec complete |
| 8. CLI & Integration | Entry point, E2E tests, binary packaging | Spec complete |

## Quality Gates

- `cargo clippy -- -D warnings` — zero warnings
- `cargo test` — all tests pass
- `cargo fmt --check` — clean
- No `unwrap()` in production code paths
- All public APIs have doc comments

## Status

**Specification: complete.** Implementation has not started.

- Spec: `.specify/features/symphony-rust/spec.md`
- Plan: `.specify/features/symphony-rust/plan.md`
- Tasks: `.specify/features/symphony-rust/tasks.md`
- Constitution: `.specify/memory/constitution.md`

## License

This project is licensed under the [Apache License 2.0](../LICENSE).
