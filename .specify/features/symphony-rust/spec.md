# Symphony Rust — Feature Specification

## 1. Summary

Build a Rust implementation of Symphony with full feature parity to the Elixir reference implementation (`elixir/`). Symphony is a long-running automation service that polls an issue tracker, creates isolated per-issue workspaces, and runs coding agent sessions for each issue. The Rust version (`rust/`) will live alongside the Elixir version in the same repository.

**Key divergences from the Elixir reference:**
- **Tracker**: GitHub Issues (via GitHub REST/GraphQL API) instead of Linear. The `Tracker` trait enables future adapters.
- **Coding Agent**: GitHub Copilot CLI in ACP (Agent Client Protocol) mode instead of Codex app-server. Both use JSON-RPC 2.0 over stdio, so the protocol layer is structurally similar.
- **Dynamic Tool**: `github_graphql` tool (exposing GitHub GraphQL API to agent sessions) replaces `linear_graphql`.

## 2. Problem & Motivation

The existing Elixir implementation depends on the BEAM runtime, OTP, and Phoenix LiveView — a stack that is unfamiliar to many teams. A Rust implementation provides:

- **Single static binary**: No runtime dependencies; deploy anywhere
- **Lower memory footprint**: Important when running many concurrent agent sessions
- **Broader adoption**: Rust is more widely known than Elixir in the coding agent ecosystem
- **Performance**: Native async I/O, zero-cost abstractions, no GC pauses
- **The spec explicitly invites this**: README says "Tell your favorite coding agent to build Symphony in a programming language of your choice"

## 3. Scope

### 3.1 In Scope (Core Conformance — SPEC §18.1)

1. **Workflow Loader** — Parse `WORKFLOW.md` with YAML front matter + Liquid-compatible prompt template
2. **Config Layer** — Typed config with defaults, `$VAR` env resolution, `~` expansion, dynamic reload
3. **Issue Tracker Client** — GitHub Issues adapter (REST + GraphQL API) with candidate fetch, state refresh, terminal fetch, pagination. Pluggable `Tracker` trait for future adapters.
4. **Orchestrator** — Poll loop, dispatch, reconciliation, retry queue, concurrency control, stall detection
5. **Workspace Manager** — Deterministic per-issue paths, sanitization, root containment, lifecycle hooks
6. **Agent Runner** — Copilot CLI via ACP (Agent Client Protocol) JSON-RPC over stdio, session handshake, streaming turns, token accounting
7. **Prompt Builder** — Strict Liquid template rendering with `issue` and `attempt` variables
8. **CLI** — Positional workflow path, `--port`, `--logs-root`, guardrails acknowledgement flag
9. **Structured Logging** — `tracing`-based with issue/session context, file sink support
10. **Filesystem Watch** — Hot reload of `WORKFLOW.md` with last-known-good fallback

### 3.2 In Scope (Extension Conformance — SPEC §18.2)

11. **HTTP Server** — `axum`-based REST API (`/api/v1/state`, `/api/v1/<id>`, `/api/v1/refresh`)
12. **Web Dashboard** — Server-rendered HTML dashboard at `/` (equivalent to Phoenix LiveView dashboard)
13. **Terminal Dashboard** — Rich terminal status output (equivalent to `StatusDashboard`)
14. **`github_graphql` Dynamic Tool** — Client-side tool exposing GitHub GraphQL API to Copilot sessions (replaces `linear_graphql`)
15. **SSH Worker Extension** — Remote execution over SSH with per-host concurrency caps

### 3.3 Out of Scope

- Multi-tenant control plane or rich admin UI
- Persistent database for orchestrator state
- Distributed clustering
- Linear adapter (dropped; trait boundary enables future addition if needed)

## 4. User Stories & Acceptance Scenarios

### User Story 1 — Core Poll-Dispatch-Run Loop (Priority: P1) 🎯 MVP

An operator starts Symphony with a valid `WORKFLOW.md` and `GITHUB_TOKEN`. Symphony polls GitHub Issues, dispatches eligible issues to Copilot CLI agent sessions in isolated workspaces, and handles normal completion with continuation retries.

**Why P1**: This is the minimum viable product. Without poll→dispatch→run, nothing else matters.

**Independent Test**: Start Symphony with a mock GitHub API returning 2 open issues and a mock Copilot CLI process. Verify both issues get dispatched, run, and complete.

**Acceptance Scenarios**:
1. **Given** a valid `WORKFLOW.md` with `tracker.kind: github` and `tracker.repo: owner/repo`, **When** Symphony starts, **Then** it polls GitHub Issues every `polling.interval_ms` and dispatches eligible issues.
2. **Given** an open GitHub Issue in an active state, **When** it is dispatched, **Then** a workspace is created, the prompt is rendered, and a Copilot CLI session runs.
3. **Given** a Copilot CLI session completes normally, **When** the issue is still in an active state, **Then** a continuation retry is scheduled after 1000ms.
4. **Given** concurrency is at `max_concurrent_agents`, **When** a new eligible issue is found, **Then** it is skipped until a slot opens.

---

### User Story 2 — Config, Validation & Hot Reload (Priority: P2)

An operator edits `WORKFLOW.md` at runtime. Symphony detects the change, validates the new config, and applies it without restart. Invalid config preserves last-known-good.

**Why P2**: Hot reload is a core conformance requirement (SPEC §6.2) and essential for operational agility.

**Independent Test**: Start Symphony, modify `polling.interval_ms` in WORKFLOW.md, verify new interval takes effect without restart. Then introduce invalid YAML, verify last-known-good is kept and an error is logged.

**Acceptance Scenarios**:
1. **Given** a running Symphony instance, **When** `WORKFLOW.md` is edited with valid changes, **Then** config is re-applied to future ticks without restart.
2. **Given** a running Symphony instance, **When** `WORKFLOW.md` is edited with invalid YAML, **Then** the last-known-good config is preserved and an operator-visible error is emitted.
3. **Given** `tracker.api_key: $GITHUB_TOKEN`, **When** Symphony starts, **Then** it resolves the env var and uses the token for API auth.
4. **Given** `workspace.root: ~/workspaces`, **When** Symphony starts on Windows, **Then** `~` expands to `USERPROFILE` via `dirs::home_dir()`.

---

### User Story 3 — Reconciliation, Stall Detection & Retry (Priority: P3)

Symphony detects stalled sessions, reconciles running issues against tracker state, and retries failed runs with exponential backoff.

**Why P3**: Without reconciliation, stalled agents run forever and terminal issues never clean up. This is the resilience layer.

**Independent Test**: Start a session, stop sending events for `stall_timeout_ms`, verify the session is killed and a retry is scheduled with correct backoff.

**Acceptance Scenarios**:
1. **Given** a running agent session with no events for `stall_timeout_ms`, **When** reconciliation runs, **Then** the worker is killed and a retry is scheduled.
2. **Given** a running issue whose tracker state becomes terminal (closed), **When** reconciliation runs, **Then** the worker is stopped and the workspace is cleaned up.
3. **Given** a worker that exits abnormally, **When** the orchestrator processes the exit, **Then** an exponential-backoff retry is scheduled: `min(10000 * 2^(attempt-1), max_retry_backoff_ms)`.
4. **Given** a retry fires but the issue is no longer in active state, **When** the retry handler runs, **Then** the claim is released.

---

### User Story 4 — HTTP API & Dashboard (Priority: P4)

An operator opens a web dashboard or queries the REST API to monitor running sessions, retry queues, token consumption, and trigger manual refreshes.

**Why P4**: Observability is essential for operating multiple concurrent agent runs. Extension conformance per SPEC §13.7.

**Independent Test**: Start Symphony with `--port 4000`, hit `GET /api/v1/state`, verify JSON response with running/retrying counts.

**Acceptance Scenarios**:
1. **Given** Symphony running with `--port 4000`, **When** `GET /api/v1/state` is called, **Then** a JSON response with running sessions, retry queue, and token totals is returned.
2. **Given** a known running issue `ABC-123`, **When** `GET /api/v1/ABC-123` is called, **Then** issue-specific runtime details are returned.
3. **Given** an unknown issue identifier, **When** `GET /api/v1/XYZ-999` is called, **Then** a 404 with `{"error":{"code":"issue_not_found","message":"..."}}` is returned.
4. **Given** Symphony running, **When** `POST /api/v1/refresh` is called, **Then** an immediate poll cycle is triggered and `202 Accepted` is returned.

---

### User Story 5 — GitHub Issues Tracker Integration (Priority: P5)

Symphony fetches issues from GitHub, maps labels to workflow states, detects blockers from linked issues, and respects API rate limits with ETag caching.

**Why P5**: The tracker adapter is critical but most of its complexity is internal plumbing tested via US1's end-to-end flow. This story covers the GitHub-specific edge cases.

**Independent Test**: Call `fetch_candidate_issues` with a mock GitHub API returning issues with various labels, blockers, and pagination. Verify normalization.

**Acceptance Scenarios**:
1. **Given** issues with labels `todo`, `in-progress` and `tracker.state_labels` configured to map them, **When** candidate fetch runs, **Then** labels are resolved to configured state names via first-match lookup.
2. **Given** an issue blocked by a non-terminal linked issue, **When** dispatch eligibility is checked for a `Todo`-state issue, **Then** it is skipped.
3. **Given** a previous API response with an ETag, **When** the next poll sends `If-None-Match`, **Then** a `304 Not Modified` response does not count against the rate limit.
4. **Given** a `429 Too Many Requests` response with `X-RateLimit-Reset`, **When** the client receives it, **Then** it backs off until the reset time.

---

### User Story 6 — `github_graphql` Dynamic Tool (Priority: P6)

The Copilot CLI agent can execute raw GitHub GraphQL queries via a client-side tool, using Symphony's configured `GITHUB_TOKEN` auth.

**Why P6**: Extension conformance. Enables agents to query/mutate GitHub resources without needing their own auth setup.

**Independent Test**: Send a `tool/call` for `github_graphql` with a valid query, verify the GraphQL response is returned to the agent session.

**Acceptance Scenarios**:
1. **Given** a valid `query` and `variables`, **When** the tool is called, **Then** the GraphQL response is returned with `success: true`.
2. **Given** a query with top-level GraphQL `errors`, **When** the tool is called, **Then** `success: false` is returned with the error body preserved.
3. **Given** missing `GITHUB_TOKEN` auth, **When** the tool is called, **Then** a structured failure payload is returned without stalling the session.

### Edge Cases

- What happens when `WORKFLOW.md` is deleted while Symphony is running? → Last-known-good config persists; error logged every tick.
- What happens when GitHub API returns paginated results that change mid-pagination? → Accept partial staleness; reconciliation on next tick corrects.
- What happens when two Symphony instances poll the same repo? → No built-in leader election; duplicate dispatch possible. Document as known limitation.
- What happens when `copilot` binary is not in PATH? → `AgentError::NotFound` on first dispatch attempt; retried with backoff.
- What happens when a workspace directory is manually deleted while an agent is running? → Agent will fail; orchestrator retries with fresh workspace.
- What happens when hooks contain Windows-incompatible shell syntax on Windows? → `pwsh -Command` fails; `HookFailed` error, run attempt aborted per hook semantics.

## 5. Requirements

### Functional Requirements

- **FR-001**: System MUST poll GitHub Issues at configurable `polling.interval_ms` intervals.
- **FR-002**: System MUST dispatch eligible issues to Copilot CLI agent sessions respecting `max_concurrent_agents`.
- **FR-003**: System MUST create deterministic per-issue workspaces under `workspace.root`.
- **FR-004**: System MUST enforce workspace path containment under the configured root.
- **FR-005**: System MUST render prompts using Liquid-compatible strict template engine.
- **FR-006**: System MUST detect `WORKFLOW.md` changes and hot-reload config without restart.
- **FR-007**: System MUST preserve last-known-good config on invalid reload.
- **FR-008**: System MUST reconcile running issues against tracker state every tick.
- **FR-009**: System MUST terminate workers for terminal-state issues and clean workspaces.
- **FR-010**: System MUST schedule exponential-backoff retries on worker failures.
- **FR-011**: System MUST schedule continuation retries (1s) after normal worker exit.
- **FR-012**: System MUST kill stalled sessions after `stall_timeout_ms` of inactivity.
- **FR-013**: System MUST execute workspace hooks via platform-appropriate shell (pwsh on Windows, sh on Unix).
- **FR-014**: System MUST auto-approve agent permission requests when `approval_policy` is `"auto-approve"`.
- **FR-015**: System MUST hard-fail agent runs that request user input in unattended mode.
- **FR-016**: System MUST expose structured logs with `issue_id`, `issue_identifier`, `session_id` context.
- **FR-017**: System MUST support `$VAR` env indirection and `~` home expansion in config paths.
- **FR-018**: System MUST run natively on Windows without WSL (PowerShell 7+ required for hooks).
- **FR-019**: System MUST document minimum `GITHUB_TOKEN` scopes: `repo`, `read:discussion`, `project` (read/write Projects v2).
- **FR-020**: System MUST negotiate ACP protocol capabilities during `initialize` handshake, log the server's reported version, and fail with a clear error if the handshake fails. Proceed with warnings if unknown capabilities are reported.

### Key Entities

- **Issue**: Normalized tracker record (id, identifier, title, state, labels, blockers, priority, timestamps). Source: GitHub Issues API. Identifier format: `REPO-NUMBER` (e.g., `rusty-42`), derived from repo name in `tracker.repo` config.
- **Workspace**: Per-issue filesystem directory under `workspace.root`. Keyed by sanitized issue identifier (e.g., `rusty-42`).
- **RunAttempt**: One execution attempt for one issue — tracks phase, workspace, timing, error.
- **LiveSession**: Active Copilot CLI session metadata — session_id, token counters, last event.
- **RetryEntry**: Scheduled retry with attempt count, backoff delay, error reason.
- **WorkflowDefinition**: Parsed `WORKFLOW.md` — config map + prompt template string.

### Success Criteria

- **SC-001**: `cargo build --release` produces a single static binary under 50MB.
- **SC-002**: All SPEC §17.1-17.7 test scenarios pass (adapted for GitHub Issues + Copilot CLI).
- **SC-003**: `cargo clippy -- -D warnings` passes with zero warnings.
- **SC-004**: Binary runs natively on Windows 10+ without WSL, with PowerShell 7+ for hooks.
- **SC-005**: HTTP API returns JSON shapes compatible with SPEC §13.7 examples.
- **SC-006**: Config hot-reload applies within one poll interval without restart.
- **SC-007**: GitHub API rate usage stays under 2,000 req/hr with ETag caching at default 30s poll.

## 6. Architecture Overview

### 6.1 Crate Structure

```
rust/
├── Cargo.toml
├── src/
│   ├── main.rs              # CLI entry point
│   ├── lib.rs                # Library root
│   ├── cli.rs                # Argument parsing (clap)
│   ├── config/
│   │   ├── mod.rs            # Config types and accessors
│   │   └── schema.rs         # Typed config schema with defaults
│   ├── workflow/
│   │   ├── mod.rs            # Workflow loader (YAML + prompt)
│   │   └── store.rs          # Cached workflow with file watch + reload
│   ├── tracker/
│   │   ├── mod.rs            # Tracker trait definition
│   │   ├── github/
│   │   │   ├── mod.rs
│   │   │   ├── client.rs     # GitHub REST + GraphQL HTTP client
│   │   │   ├── adapter.rs    # Tracker trait impl for GitHub Issues
│   │   │   └── issue.rs      # Normalized issue struct + GitHub → Issue mapping
│   │   └── memory.rs         # In-memory tracker for tests
│   ├── orchestrator/
│   │   ├── mod.rs            # Poll loop, dispatch, reconciliation
│   │   └── state.rs          # Runtime state struct
│   ├── workspace/
│   │   ├── mod.rs            # Workspace lifecycle
│   │   ├── hooks.rs          # Shell hook execution
│   │   └── path_safety.rs    # Sanitization + root containment
│   ├── agent/
│   │   ├── mod.rs            # Agent runner (workspace + prompt + session)
│   │   ├── acp_client.rs     # Copilot CLI ACP JSON-RPC stdio client
│   │   └── dynamic_tool.rs   # github_graphql tool handler
│   ├── prompt.rs             # Liquid template rendering
│   ├── ssh.rs                # SSH command execution
│   ├── server/
│   │   ├── mod.rs            # axum HTTP server
│   │   ├── api.rs            # /api/v1/* handlers
│   │   └── dashboard.rs      # / HTML dashboard
│   ├── dashboard.rs          # Terminal status renderer
│   └── logging.rs            # tracing setup + file sink
├── tests/
│   ├── workflow_test.rs
│   ├── config_test.rs
│   ├── workspace_test.rs
│   ├── orchestrator_test.rs
│   ├── tracker_test.rs
│   ├── app_server_test.rs
│   ├── prompt_test.rs
│   ├── cli_test.rs
│   ├── api_test.rs
│   └── e2e_test.rs
└── README.md
```

### 6.2 Core Traits

```rust
/// Issue tracker abstraction (SPEC §11)
#[async_trait]
pub trait Tracker: Send + Sync {
    async fn fetch_candidate_issues(&self, config: &TrackerConfig) -> Result<Vec<Issue>>;
    async fn fetch_issue_states_by_ids(&self, ids: &[String]) -> Result<Vec<Issue>>;
    async fn fetch_issues_by_states(&self, states: &[String], config: &TrackerConfig) -> Result<Vec<Issue>>;
}

/// Workspace management abstraction (SPEC §9)
pub trait WorkspaceManager: Send + Sync {
    fn create_for_issue(&self, identifier: &str, config: &WorkspaceConfig) -> Result<Workspace>;
    fn remove_workspace(&self, identifier: &str, config: &WorkspaceConfig) -> Result<()>;
    fn run_hook(&self, hook: HookKind, workspace_path: &Path, script: &str, timeout: Duration) -> Result<()>;
}

/// Agent session abstraction (SPEC §10)
#[async_trait]
pub trait AgentSession: Send {
    async fn initialize(&mut self) -> Result<()>;
    async fn start_thread(&mut self, params: ThreadStartParams) -> Result<ThreadInfo>;
    async fn start_turn(&mut self, params: TurnStartParams) -> Result<TurnInfo>;
    async fn stream_events(&mut self) -> Result<Pin<Box<dyn Stream<Item = AgentEvent>>>>;
    async fn stop(&mut self) -> Result<()>;
}
```

### 6.3 Core Types (SPEC §4)

```rust
/// Normalized issue (SPEC §4.1.1)
pub struct Issue {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub description: Option<String>,
    pub priority: Option<i32>,
    pub state: String,
    pub branch_name: Option<String>,
    pub url: Option<String>,
    pub labels: Vec<String>,
    pub blocked_by: Vec<BlockerRef>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

/// Orchestrator runtime state (SPEC §4.1.8)
pub struct OrchestratorState {
    pub poll_interval_ms: u64,
    pub max_concurrent_agents: usize,
    pub running: HashMap<String, RunningEntry>,
    pub claimed: HashSet<String>,
    pub retry_attempts: HashMap<String, RetryEntry>,
    pub completed: HashSet<String>,
    pub agent_totals: TokenTotals,        // renamed from codex_totals
    pub agent_rate_limits: Option<serde_json::Value>,  // renamed from codex_rate_limits
}

/// Issue orchestration states (SPEC §7.1)
pub enum OrchestratorIssueState {
    Unclaimed,
    Claimed,
    Running,
    RetryQueued,
    Released,
}

/// Run attempt lifecycle (SPEC §7.2)
pub enum RunAttemptPhase {
    PreparingWorkspace,
    BuildingPrompt,
    LaunchingAgentProcess,
    InitializingSession,
    StreamingTurn,
    Finishing,
    Succeeded,
    Failed(String),
    TimedOut,
    Stalled,
    CanceledByReconciliation,
}
```

### 6.4 Concurrency Model

The Elixir implementation uses OTP GenServer + supervised Tasks. The Rust equivalent:

| Elixir Pattern | Rust Equivalent |
|---|---|
| `GenServer` (Orchestrator) | `tokio::spawn` + `mpsc` channel for state ownership |
| `Task.Supervisor` (worker tasks) | `tokio::JoinSet` with abort handles |
| `Process.monitor` / `{:DOWN, ...}` | `JoinSet::join_next()` for task completion |
| `Process.send_after` (timers) | `tokio::time::sleep` + `select!` |
| `Port.open` (Codex subprocess) | `tokio::process::Command` with piped stdio (Copilot CLI ACP) |
| `Phoenix.PubSub` (dashboard updates) | `tokio::sync::broadcast` channel |
| `GenServer.call` (snapshot) | `oneshot` channel request/response |

The orchestrator runs as a single async task that owns all mutable state (no `Arc<Mutex<_>>`). Workers communicate back via an `mpsc` channel. This preserves the single-authority invariant from the spec.

## 7. Detailed Component Specifications

### 7.1 Workflow Loader (SPEC §5)

- Parse `WORKFLOW.md`: detect `---` delimiters, extract YAML front matter, remainder is prompt body
- YAML must decode to a map; non-map YAML is `workflow_front_matter_not_a_map` error
- Missing file is `missing_workflow_file` error
- Return `WorkflowDefinition { config: serde_yaml::Value, prompt_template: String }`
- Unknown top-level keys are ignored (forward compatibility)

### 7.2 Config Schema (SPEC §6, adapted for GitHub Issues + Copilot CLI)

All fields from SPEC §6.4 adapted for GitHub Issues and Copilot CLI:
- `tracker.kind`: required (`"github"` — replaces `"linear"`)
- `tracker.endpoint`: default `https://api.github.com` (GitHub REST/GraphQL API base)
- `tracker.api_key`: supports `$VAR` indirection; canonical env `GITHUB_TOKEN`
- `tracker.repo`: required — `owner/repo` format (replaces `project_slug`)
- `tracker.active_states`: default `["open"]` (GitHub Issues are `open` or `closed`; labels can refine further)
- `tracker.terminal_states`: default `["closed"]`
- `tracker.labels`: optional list of label filters to scope which issues are eligible for dispatch
- `tracker.state_labels`: optional map of `label → state_name` (e.g., `{ "todo": "Todo", "in-progress": "In Progress", "human-review": "Human Review" }`). First matching label wins; no match falls back to `"open"` or `"closed"`. Enables Linear-style workflow states on GitHub Issues.
- `tracker.assignee`: optional — filter issues by assignee; `"me"` resolves via authenticated user
- `polling.interval_ms`: default `30000`
- `workspace.root`: default `<tmp>/symphony_workspaces`, supports `~` and `$VAR`
- `hooks.after_create`, `before_run`, `after_run`, `before_remove`: optional shell scripts
- `hooks.timeout_ms`: default `60000`
- `agent.max_concurrent_agents`: default `10`
- `agent.max_turns`: default `20`
- `agent.max_retry_backoff_ms`: default `300000`
- `agent.max_concurrent_agents_by_state`: default `{}`
- `agent.command`: default `"copilot --acp --stdio"` (replaces `codex.command`)
- `agent.turn_timeout_ms`: default `3600000`
- `agent.read_timeout_ms`: default `5000`
- `agent.stall_timeout_ms`: default `300000`
- `agent.approval_policy`: default `"auto-approve"` (Copilot CLI equivalent of Codex `"never"`)
- `server.port`: optional extension

**GitHub Issues State Mapping:**
GitHub Issues don't have rich workflow states like Linear. The adapter maps states as follows:
- If `tracker.state_labels` is configured, scan issue labels in order; first label matching a key in the map determines the state name (e.g., label `todo` → state `"Todo"`)
- If no label matches (or `state_labels` is not configured), fall back to `"open"` or `"closed"` based on the GitHub issue state
- `active_states` and `terminal_states` are matched against the resolved state name (case-insensitive)

**Issue Identifier Format:**
- `identifier` is formatted as `REPO-NUMBER` (e.g., `rusty-42`), derived from the repo name portion of `tracker.repo` and the issue number
- This format is safe for workspace directory names (no sanitization needed) and readable in logs/prompts

### 7.3 Orchestrator State Machine (SPEC §7-8)

The orchestrator loop:
1. **Reconcile** running issues (stall detection + tracker state refresh)
2. **Validate** dispatch config
3. **Fetch** candidate issues from tracker
4. **Sort** by priority (asc), then created_at (oldest first), then identifier (lexicographic)
5. **Dispatch** eligible issues until slots exhausted

Candidate eligibility (SPEC §8.2):
- Has `id`, `identifier`, `title`, `state`
- State in `active_states` and not in `terminal_states`
- Not in `running` or `claimed`
- Global + per-state concurrency slots available
- `Todo` state: no non-terminal blockers

Retry backoff (SPEC §8.4):
- Normal exit: 1000ms continuation retry
- Failure: `min(10000 * 2^(attempt-1), max_retry_backoff_ms)`
- No attempt count cap (spec-conformant). Retries continue until issue leaves active state or operator intervenes.
- Log warnings at attempt thresholds 5, 10, and 20 for operator visibility.

### 7.4 Agent Runner Protocol (Copilot CLI via ACP — adapted from SPEC §10)

JSON-RPC 2.0 over stdio with Copilot CLI in ACP mode (`copilot --acp --stdio`):

The Agent Client Protocol (ACP) is structurally similar to the Codex app-server protocol — both use newline-delimited JSON-RPC 2.0 over stdio. Key differences:

| Codex App-Server | Copilot CLI ACP | Notes |
|---|---|---|
| `codex app-server` | `copilot --acp --stdio` | Launch command |
| `initialize` / `initialized` | `initialize` / `initialized` | Same handshake pattern |
| `thread/start` | `session/create` | Session creation |
| `turn/start` | `session/message/send` | Turn initiation |
| `turn/completed` | Streaming completion events | Turn lifecycle |
| `item/tool/call` | `tool/call` | Dynamic tool invocation |
| approval requests | permission requests | Agent asking for approval |

Session flow:
1. Launch `copilot --acp --stdio` as subprocess with piped stdio (direct launch, no shell wrapper)
2. Send `initialize` with desired capabilities → wait for response; log server-reported version/capabilities; warn on unknown capabilities; fail with clear error if handshake fails
3. Send `initialized` notification
4. Send `session/create` → get session ID (equivalent to `thread/start`)
5. Send `session/message/send` with rendered prompt → stream events
6. Handle completion, failure, cancellation events
7. Support continuation messages on same session up to `max_turns`
8. Auto-approve permission requests when policy is `"auto-approve"`
9. Handle dynamic tool calls (`github_graphql`)
10. Hard-fail on user input requests in unattended mode

The `AgentSession` trait abstracts over both protocols, so the orchestrator doesn't care whether Copilot CLI or Codex is running underneath.

### 7.5 HTTP API (SPEC §13.7)

- `GET /` — HTML dashboard
- `GET /api/v1/state` — JSON system state snapshot
- `GET /api/v1/:issue_identifier` — JSON issue detail
- `POST /api/v1/refresh` — Trigger immediate poll (202 Accepted)
- `405` for unsupported methods
- JSON error envelope: `{"error":{"code":"...","message":"..."}}`

### 7.6 SSH Worker Extension (SPEC Appendix A) — DEFERRED

- `worker.ssh_hosts` list of remote hosts
- `worker.max_concurrent_agents_per_host` per-host cap
- Launch Copilot CLI via SSH stdio instead of local subprocess
- Workspace paths interpreted on remote host
- Prefer previously-used host on retries
- Least-loaded host selection for new dispatches

## 8. Elixir Parity Mapping (adapted for GitHub Issues + Copilot CLI)

| Elixir Module | Rust Module | Notes |
|---|---|---|
| `SymphonyElixir.Application` | `main.rs` + `lib.rs` | OTP supervisor → tokio task spawning |
| `SymphonyElixir.CLI` | `cli.rs` | OptionParser → clap |
| `SymphonyElixir.Orchestrator` | `orchestrator/mod.rs` | GenServer → channel-based state loop |
| `SymphonyElixir.Orchestrator.State` | `orchestrator/state.rs` | Struct with HashMap/HashSet |
| `SymphonyElixir.AgentRunner` | `agent/mod.rs` | Supervised Task → JoinSet task |
| `SymphonyElixir.Workspace` | `workspace/mod.rs` | Direct filesystem ops |
| `SymphonyElixir.PathSafety` | `workspace/path_safety.rs` | Symlink-aware canonicalization |
| `SymphonyElixir.Config` | `config/mod.rs` | Ecto embedded schema → serde structs |
| `SymphonyElixir.Config.Schema` | `config/schema.rs` | Ecto changesets → serde defaults + validation |
| `SymphonyElixir.Workflow` | `workflow/mod.rs` | YAML parse + prompt split |
| `SymphonyElixir.WorkflowStore` | `workflow/store.rs` | GenServer → notify file watcher + channel |
| `SymphonyElixir.PromptBuilder` | `prompt.rs` | Solid → liquid-rust |
| `SymphonyElixir.Tracker` | `tracker/mod.rs` | Behaviour → trait |
| `SymphonyElixir.Linear.Client` | `tracker/github/client.rs` | Linear GraphQL → GitHub REST/GraphQL via `reqwest` |
| `SymphonyElixir.Linear.Adapter` | `tracker/github/adapter.rs` | Tracker trait impl for GitHub Issues |
| `SymphonyElixir.Linear.Issue` | `tracker/github/issue.rs` | Struct + From impl (GitHub → normalized Issue) |
| `SymphonyElixir.Tracker.Memory` | `tracker/memory.rs` | Test double |
| `SymphonyElixir.Codex.AppServer` | `agent/acp_client.rs` | Codex app-server → Copilot CLI ACP client |
| `SymphonyElixir.Codex.DynamicTool` | `agent/dynamic_tool.rs` | `linear_graphql` → `github_graphql` |
| `SymphonyElixir.SSH` | `ssh.rs` | SSH command builder |
| `SymphonyElixir.StatusDashboard` | `dashboard.rs` | Terminal renderer |
| `SymphonyElixir.HttpServer` | `server/mod.rs` | Phoenix → axum |
| `SymphonyElixir.LogFile` | `logging.rs` | :logger_disk_log → tracing-appender |
| `SymphonyElixirWeb.*` | `server/*.rs` | Phoenix LiveView → axum + HTML templates |

## 9. Test Strategy (SPEC §17)

### 8.1 Core Conformance Tests
- Workflow parsing (front matter, prompt, errors, reload)
- Config defaults, env resolution, validation
- Workspace safety (sanitization, root containment, hooks)
- Orchestrator dispatch (sort order, eligibility, blockers, concurrency)
- Orchestrator reconciliation (stall detection, terminal/active state handling)
- Retry queue (exponential backoff, continuation retries, cap)
- ACP protocol (handshake, session lifecycle, approvals, tool calls)
- Prompt rendering (strict mode, issue/attempt variables)
- CLI argument parsing and startup

### 8.2 Extension Tests
- HTTP API endpoints (state, issue detail, refresh, error envelopes)
- Dashboard rendering
- `github_graphql` dynamic tool
- SSH host selection and remote execution

### 8.3 Integration Tests
- End-to-end with mock GitHub API server + mock Copilot CLI process
- Gated live E2E tests with `SYMPHONY_RUN_LIVE_E2E=1`
