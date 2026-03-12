# Symphony Rust — Task Breakdown

## Phase 1: Foundation

### Story 1: Project Skeleton
- [x] T1.1: `cargo init rust --name symphony` with workspace Cargo.toml [P]
- [ ] T1.2: Add dependencies to Cargo.toml (tokio, serde, serde_json, serde_yaml, clap, tracing, reqwest, axum, liquid, notify, chrono, uuid, thiserror, anyhow, async-trait, mockall)
- [ ] T1.3: Create module structure (`src/lib.rs`, all `mod.rs` files, empty modules)
- [ ] T1.4: Configure `clippy.toml`, `rustfmt.toml`, release profile with LTO
- [ ] T1.5: Add `Makefile` with build/test/lint/fmt targets

### Story 2: Config Schema
Depends on: S1
- [ ] T2.1: Define `TrackerConfig` struct with serde defaults and `$VAR` resolution
- [ ] T2.2: Define `PollingConfig`, `WorkspaceConfig`, `HooksConfig`, `AgentConfig`, `ServerConfig`
- [ ] T2.3: Implement `~` home expansion and `$VAR` env resolution for path fields
- [ ] T2.4: Implement config validation (tracker.kind=`"github"`, api_key/`GITHUB_TOKEN`, repo, agent.command)
- [ ] T2.5: Implement `max_concurrent_agents_by_state` normalization (lowercase keys, ignore invalid)
- [ ] T2.6: Write tests for all defaults, env resolution, validation errors
  - Files: `src/config/mod.rs`, `src/config/schema.rs`, `tests/config_test.rs`

### Story 3: Workflow Loader
Depends on: S1
- [ ] T3.1: Implement `WORKFLOW.md` parser (YAML front matter + prompt body split)
- [ ] T3.2: Handle edge cases (no front matter, empty prompt, non-map YAML)
- [ ] T3.3: Implement `WorkflowStore` with `notify` file watcher and last-known-good fallback
- [ ] T3.4: Implement reload channel notification to orchestrator
- [ ] T3.5: Write tests for parsing, errors, reload behavior
  - Files: `src/workflow/mod.rs`, `src/workflow/store.rs`, `tests/workflow_test.rs`

### Story 4: Prompt Builder
Depends on: S3
- [ ] T4.1: Implement Liquid template rendering with strict mode
- [ ] T4.2: Implement `issue` object serialization for template context (including nested labels, blockers)
- [ ] T4.3: Implement `attempt` variable (null on first run, integer on retry)
- [ ] T4.4: Implement fallback prompt for empty template body
- [ ] T4.5: Write tests for rendering, strict variable checking, strict filter checking
  - Files: `src/prompt.rs`, `tests/prompt_test.rs`

## Phase 2: Tracker Integration

### Story 5: Tracker Trait + Memory Implementation
Depends on: S1
- [ ] T5.1: Define `Tracker` async trait with 3 required methods [P]
- [ ] T5.2: Define `Issue` struct matching SPEC §4.1.1 (all fields)
- [ ] T5.3: Define `BlockerRef` struct
- [ ] T5.4: Implement `MemoryTracker` for tests (in-memory issue store)
- [ ] T5.5: Write tests for memory tracker operations
  - Files: `src/tracker/mod.rs`, `src/tracker/memory.rs`, `tests/tracker_test.rs`

### Story 6: GitHub Issues Client
Depends on: S2, S5
- [ ] T6.1: Implement candidate issue fetch via GitHub REST API (`GET /repos/{owner}/{repo}/issues`) with pagination
- [ ] T6.2: Implement issue state refresh by IDs (batch fetch specific issues by number)
- [ ] T6.3: Implement terminal-state issue fetch (closed issues for startup cleanup)
- [ ] T6.4: Implement issue normalization (labels lowercase, map GitHub labels to state names, priority from labels)
- [ ] T6.5: Implement blocker detection (linked issues, "blocked by" references in body/comments)
- [ ] T6.6: Implement assignee-based filtering (`tracker.assignee`, `"me"` resolution via authenticated user)
- [ ] T6.7: Implement label-based state mapping (e.g., `todo` label → `"Todo"` state for dispatch)
- [ ] T6.8: Implement error mapping (transport, HTTP status, rate limiting, malformed payloads)
- [ ] T6.9: Write tests with mock HTTP server (wiremock or similar)
  - Files: `src/tracker/github/client.rs`, `src/tracker/github/adapter.rs`, `src/tracker/github/issue.rs`

## Phase 3: Workspace Management

### Story 7: Path Safety
Depends on: S1
- [ ] T7.1: Implement workspace key sanitization (`[A-Za-z0-9._-]` only, replace others with `_`) [P]
- [ ] T7.2: Implement path canonicalization (symlink-aware, absolute paths)
- [ ] T7.3: Implement root containment check (workspace path must be under workspace root)
- [ ] T7.4: Write tests for sanitization, canonicalization, containment violations
  - Files: `src/workspace/path_safety.rs`

### Story 8: Workspace Manager + Hooks
Depends on: S2, S7
- [ ] T8.1: Implement `create_for_issue` (create dir if new, reuse if exists, track `created_now`)
- [ ] T8.2: Implement `remove_workspace` with `before_remove` hook
- [ ] T8.3: Implement hook execution (`sh -lc <script>` with workspace cwd)
- [ ] T8.4: Implement hook timeout enforcement (`hooks.timeout_ms`)
- [ ] T8.5: Implement hook failure semantics (after_create fatal, before_run fatal, after_run/before_remove ignored)
- [ ] T8.6: Implement remote workspace ops via SSH
- [ ] T8.7: Write tests for creation, reuse, hooks, timeouts, failures
  - Files: `src/workspace/mod.rs`, `src/workspace/hooks.rs`, `tests/workspace_test.rs`

## Phase 4: Orchestrator Core

### Story 9: Orchestrator State + Dispatch
Depends on: S2, S5, S8
- [ ] T9.1: Define `OrchestratorState` struct with all fields from SPEC §4.1.8
- [ ] T9.2: Implement orchestrator message loop with `tokio::select!`
- [ ] T9.3: Implement tick handler (reconcile → validate → fetch → sort → dispatch)
- [ ] T9.4: Implement candidate eligibility check (SPEC §8.2 — all 6 conditions)
- [ ] T9.5: Implement dispatch sort order (priority asc, created_at asc, identifier asc)
- [ ] T9.6: Implement concurrency control (global + per-state + per-host)
- [ ] T9.7: Implement `Todo` blocker check (skip if any blocker non-terminal)
- [ ] T9.8: Implement worker spawn via `JoinSet`
- [ ] T9.9: Write tests for dispatch order, eligibility, concurrency limits, blockers
  - Files: `src/orchestrator/mod.rs`, `src/orchestrator/state.rs`, `tests/orchestrator_test.rs`

### Story 10: Reconciliation + Stall Detection
Depends on: S9
- [ ] T10.1: Implement stall detection (elapsed since last event > `stall_timeout_ms`)
- [ ] T10.2: Implement tracker state refresh reconciliation (terminal → stop+cleanup, active → update, other → stop)
- [ ] T10.3: Implement graceful handling when state refresh fails (keep workers running)
- [ ] T10.4: Implement startup terminal workspace cleanup
- [ ] T10.5: Write tests for stall detection, state transitions, cleanup, refresh failures
  - Files: `src/orchestrator/mod.rs`, `tests/orchestrator_test.rs`

### Story 11: Retry Queue + Backoff
Depends on: S9
- [ ] T11.1: Implement retry entry creation with timer scheduling
- [ ] T11.2: Implement normal-exit continuation retry (1000ms, attempt=1)
- [ ] T11.3: Implement failure-driven exponential backoff (`min(10000 * 2^(n-1), max)`)
- [ ] T11.4: Implement retry timer handler (fetch candidates, check eligibility, dispatch or requeue)
- [ ] T11.5: Implement claim release on retry for non-active/missing issues
- [ ] T11.6: Write tests for backoff formula, continuation retries, slot exhaustion requeue
  - Files: `src/orchestrator/mod.rs`, `tests/orchestrator_test.rs`

## Phase 5: Agent Integration

### Story 12: Copilot CLI ACP Protocol Client
Depends on: S1
- [ ] T12.1: Implement subprocess launch (`copilot --acp --stdio` with piped stdio)
- [ ] T12.2: Implement JSON-RPC 2.0 message framing (newline-delimited JSON on stdout)
- [ ] T12.3: Implement `initialize` request + response parsing with `read_timeout_ms`
- [ ] T12.4: Implement `initialized` notification
- [ ] T12.5: Implement `session/create` request + session ID extraction (ACP equivalent of `thread/start`)
- [ ] T12.6: Implement `session/message/send` request + streaming events (ACP equivalent of `turn/start`)
- [ ] T12.7: Implement streaming event processing (completion, failure, cancellation)
- [ ] T12.8: Implement permission auto-handling (auto-approve when policy is `"auto-approve"`)
- [ ] T12.9: Implement unsupported dynamic tool call rejection
- [ ] T12.10: Implement user-input-required hard failure (unattended mode)
- [ ] T12.11: Implement stderr handling (log diagnostics, ignore for protocol)
- [ ] T12.12: Implement turn timeout enforcement
- [ ] T12.13: Write tests with mock subprocess (echo-back process)
  - Files: `src/agent/acp_client.rs`, `tests/acp_client_test.rs`

### Story 13: Agent Runner
Depends on: S4, S8, S12
- [ ] T13.1: Implement full agent run lifecycle (workspace → hooks → session → turns → cleanup)
- [ ] T13.2: Implement continuation turn loop (re-check state, continue if active, up to max_turns)
- [ ] T13.3: Implement first-turn vs continuation-turn prompt difference
- [ ] T13.4: Implement error handling (workspace fail, hook fail, session fail, turn fail)
- [ ] T13.5: Implement `after_run` hook on all exit paths (best-effort)
- [ ] T13.6: Write tests for full lifecycle, continuation, error paths
  - Files: `src/agent/mod.rs`, `tests/agent_test.rs`

### Story 14: Token Accounting + Events
Depends on: S9, S13
- [ ] T14.1: Implement Codex event forwarding from worker to orchestrator
- [ ] T14.2: Implement token extraction from agent events (absolute totals, delta tracking)
- [ ] T14.3: Implement rate-limit snapshot extraction
- [ ] T14.4: Implement aggregate runtime seconds accounting
- [ ] T14.5: Implement session_id composition (`<thread_id>-<turn_id>`)
- [ ] T14.6: Write tests for token accumulation, delta correctness, rate-limit tracking

## Phase 6: Observability & Dashboard

### Story 15: Structured Logging
Depends on: S1
- [ ] T15.1: Configure `tracing-subscriber` with structured JSON output [P]
- [ ] T15.2: Implement file sink with log rotation (tracing-appender)
- [ ] T15.3: Add span context macros for `issue_id`, `issue_identifier`, `session_id`
- [ ] T15.4: Write tests for logging configuration
  - Files: `src/logging.rs`

### Story 16: Terminal Dashboard
Depends on: S9
- [ ] T16.1: Implement snapshot request from orchestrator
- [ ] T16.2: Implement rich terminal rendering (running sessions, retry queue, token totals, rate limits)
- [ ] T16.3: Implement render throttling (configurable refresh interval)
- [ ] T16.4: Implement humanized Codex event summaries
- [ ] T16.5: Write snapshot tests with expected terminal output
  - Files: `src/dashboard.rs`, `tests/dashboard_test.rs`

### Story 17: HTTP Server + API
Depends on: S9
- [ ] T17.1: Implement axum server startup (bind loopback, configurable port) [P]
- [ ] T17.2: Implement `GET /api/v1/state` endpoint
- [ ] T17.3: Implement `GET /api/v1/:issue_identifier` endpoint (404 for unknown)
- [ ] T17.4: Implement `POST /api/v1/refresh` endpoint (202 Accepted)
- [ ] T17.5: Implement JSON error envelope (`{"error":{"code":"...","message":"..."}}`)
- [ ] T17.6: Implement `405 Method Not Allowed` for unsupported methods
- [ ] T17.7: Implement `GET /` HTML dashboard page
- [ ] T17.8: Write tests for all API endpoints, error cases, dashboard rendering
  - Files: `src/server/mod.rs`, `src/server/api.rs`, `src/server/dashboard.rs`, `tests/api_test.rs`

## Phase 7: Extensions

### Story 18: SSH Worker Extension
Depends on: S8, S12
- [ ] T18.1: Implement SSH command builder (host, workspace path, codex command)
- [ ] T18.2: Implement remote subprocess launch via SSH stdio
- [ ] T18.3: Implement per-host concurrency tracking
- [ ] T18.4: Implement least-loaded host selection
- [ ] T18.5: Implement host preference on retries
- [ ] T18.6: Write tests for host selection, concurrency caps, SSH command generation
  - Files: `src/ssh.rs`, `tests/ssh_test.rs`

### Story 19: github_graphql Dynamic Tool
Depends on: S6, S12
- [ ] T19.1: Implement tool spec advertisement during session startup
- [ ] T19.2: Implement query/variables input parsing and validation
- [ ] T19.3: Implement GraphQL execution via GitHub API using configured `GITHUB_TOKEN` auth
- [ ] T19.4: Implement success/failure result formatting
- [ ] T19.5: Implement multi-operation rejection
- [ ] T19.6: Write tests for valid queries, errors, invalid inputs, missing auth
  - Files: `src/agent/dynamic_tool.rs`, `tests/dynamic_tool_test.rs`

## Phase 8: CLI & Integration

### Story 20: CLI Entry Point
Depends on: S9, S15, S17
- [ ] T20.1: Implement clap argument parser (positional workflow path, --port, --logs-root, guardrails flag)
- [ ] T20.2: Implement default workflow path (`./WORKFLOW.md`)
- [ ] T20.3: Implement startup validation and clean error reporting
- [ ] T20.4: Implement graceful shutdown (SIGINT/SIGTERM handling)
- [ ] T20.5: Implement exit codes (0 normal, 1 failure)
- [ ] T20.6: Write tests for arg parsing, missing flag rejection, nonexistent paths
  - Files: `src/main.rs`, `src/cli.rs`, `tests/cli_test.rs`

### Story 21: End-to-End Integration Tests
Depends on: All
- [ ] T21.1: Build mock GitHub API server for integration tests
- [ ] T21.2: Build mock Copilot CLI ACP process for integration tests
- [ ] T21.3: Write full lifecycle test (poll → dispatch → run → complete → retry)
- [ ] T21.4: Write config reload test (change WORKFLOW.md mid-run)
- [ ] T21.5: Write reconciliation test (terminal state stops worker)
- [ ] T21.6: Write gated live E2E test with real Linear + Codex (`SYMPHONY_RUN_LIVE_E2E=1`)
  - Files: `tests/e2e_test.rs`, `tests/fixtures/`

## Summary

- **21 stories**, **~105 tasks**
- **8 phases** with clear dependency ordering
- **[P]** marks tasks safe for parallel execution within a story
- Estimated test count: ~200 tests (matching Elixir's 188 + Rust-specific edge cases)
