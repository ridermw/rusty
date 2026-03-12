# Symphony Rust — Implementation Plan

## Architecture Decisions

### AD-1: Single-Task Orchestrator with Channel Communication
The orchestrator owns all mutable state in a single `tokio` task. Workers communicate back via `mpsc` channels. This mirrors the Elixir GenServer's serial message processing and eliminates lock contention. Snapshot requests use `oneshot` channels for request-response.

### AD-2: Trait-Based Boundaries
External integrations (Tracker, WorkspaceManager, AgentSession) are defined as traits. Production code uses concrete types; tests use mock implementations via `mockall`. This enables testing without network or subprocess dependencies.

### AD-3: Typed State Machine
Orchestrator issue states and run attempt phases are Rust enums, enforcing valid transitions at compile time. The Elixir implementation uses atoms/strings for these — Rust enums are strictly better.

### AD-4: Error Taxonomy
A unified `SymphonyError` enum covers all error categories from SPEC §14.1 (workflow, workspace, agent, tracker, observability). Each variant carries enough context for structured logging and retry decisions.

### AD-5: Axum over Actix/Warp
Axum is chosen for the HTTP server because it's built on `tokio` and `tower`, shares the same async runtime as the orchestrator, and has excellent ergonomics for JSON APIs and HTML serving.

### AD-6: Liquid Templating
The `liquid` crate provides Liquid-compatible template rendering, matching the spec's requirement for strict variable/filter checking. The Elixir implementation uses `Solid` (also Liquid-compatible).
### AD-7: GitHub Issues as Primary Tracker (replaces Linear)
GitHub Issues uses REST + GraphQL APIs instead of Linear's pure GraphQL. Key differences:
- Issues have `open`/`closed` states (not rich workflow states like Linear's `Todo`/`In Progress`/etc.)
- Labels serve as the state refinement layer — e.g., a `todo` label maps to the `"Todo"` active state
- Blockers are detected via linked issues and body/comment references, not native relation types
- Pagination uses Link headers (REST) or cursor-based (GraphQL), not Linear's node-based pagination
- Auth is via `GITHUB_TOKEN` env var

### AD-8: Copilot CLI ACP as Coding Agent (replaces Codex App-Server)
Copilot CLI's ACP (Agent Client Protocol) uses the same JSON-RPC 2.0 over stdio transport as Codex app-server but with slightly different method names (`session/create` vs `thread/start`, `session/message/send` vs `turn/start`). The `AgentSession` trait abstracts over both, so a future `CodexSession` adapter could be added without changing the orchestrator.


### AD-9: Windows-Native Shell Strategy (pwsh on Windows, sh on Unix)
The SPEC says "execute in a local shell context appropriate to the host OS." A `ShellExecutor` trait provides platform-specific hook execution: `sh -lc <script>` on Unix, `pwsh -Command <script>` on Windows (PowerShell 7+ required). Agent launch (Copilot CLI) uses direct subprocess invocation without a shell wrapper — simpler and cross-platform.

### AD-10: Domain-Scoped Error Hierarchy
Error types are scoped per domain (`TrackerError`, `WorkspaceError`, `AgentError`, `ConfigError`) and wrapped by a top-level `SymphonyError`. This preserves type-level information about what can fail at each call site, enabling precise `match` handling and better structured logging.

### AD-11: Direct Agent Launch (No Shell Wrapper)
Unlike the Elixir reference which wraps Codex in `bash -lc`, the Rust impl launches `copilot --acp --stdio` directly via `tokio::process::Command`. No shell wrapper needed — Copilot CLI is a standalone binary. This is simpler, faster, and cross-platform.


Establish the project skeleton, config parsing, workflow loading, and prompt rendering. These have no external dependencies and can be fully unit tested.

### Phase 2: Tracker Integration (Stories 5-6)
Build the GitHub Issues client and tracker trait. Requires HTTP client but can be tested with mock servers.

### Phase 3: Workspace Management (Stories 7-8)
Filesystem operations, hook execution, path safety. Requires temp directories for testing.

### Phase 4: Orchestrator Core (Stories 9-11)
The main poll loop, dispatch logic, reconciliation, and retry queue. This is the heart of the system — highest complexity, most test cases.

### Phase 5: Agent Integration (Stories 12-14)
Copilot CLI ACP protocol, subprocess management, event streaming, token accounting. Requires mock subprocess for testing.

### Phase 6: Observability & Dashboard (Stories 15-17)
Structured logging, terminal dashboard, HTTP server with API and web dashboard.

### Phase 7: Extensions (Stories 18-19)
SSH worker support, `github_graphql` dynamic tool.

### Phase 8: CLI & Integration (Stories 20-21)
CLI entry point, end-to-end integration tests.

## Dependency Graph

```
Phase 1: Foundation
  ├── S1: Project skeleton
  ├── S2: Config schema (depends on S1)
  ├── S3: Workflow loader (depends on S1)
  └── S4: Prompt builder (depends on S3)

Phase 2: Tracker
  ├── S5: Tracker trait + memory impl (depends on S1)
  └── S6: GitHub Issues client (depends on S2, S5)

Phase 3: Workspace
  ├── S7: Path safety + sanitization (depends on S1)
  └── S8: Workspace manager + hooks (depends on S2, S7)

Phase 4: Orchestrator
  ├── S9: Orchestrator state + dispatch (depends on S2, S5, S8)
  ├── S10: Reconciliation + stall detection (depends on S9)
  └── S11: Retry queue + backoff (depends on S9)

Phase 5: Agent
  ├── S12: Copilot CLI ACP client (depends on S1)
  ├── S13: Agent runner (depends on S4, S8, S12)
  └── S14: Token accounting + events (depends on S9, S13)

Phase 6: Observability
  ├── S15: Structured logging (depends on S1)
  ├── S16: Terminal dashboard (depends on S9)
  └── S17: HTTP server + API (depends on S9)

Phase 7: Extensions
  ├── S18: SSH workers (depends on S8, S12)
  └── S19: github_graphql tool (depends on S6, S12)

Phase 8: CLI & E2E
  ├── S20: CLI entry point (depends on S9, S15, S17)
  └── S21: Integration tests (depends on all)
```

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Codex app-server protocol drift | Low | Low | Kept behind AgentSession trait; add CodexSession adapter if needed later |
| Linear GraphQL schema changes | N/A | N/A | Dropped — GitHub Issues adapter instead |
| `liquid` crate incompatibility with Solid | Low | High | Test all template patterns from WORKFLOW.md early |
| GitHub API rate limiting during heavy polling | High | Medium | Implement conditional requests (ETags/If-None-Match), respect X-RateLimit headers |
| Copilot CLI ACP protocol version drift | Medium | High | Keep protocol parsing lenient; abstract behind AgentSession trait |
| SSH subprocess management complexity | Medium | Medium | Defer to Phase 7; test with Docker SSH container |
| Large binary size from static linking | Low | Low | Use `lto = true` and strip in release profile |

## Key Design Patterns

### Orchestrator Message Loop
```rust
enum OrchestratorMsg {
    Tick,
    WorkerExited { issue_id: String, result: WorkerResult },
    AgentUpdate { issue_id: String, event: AgentEvent },
    RetryFired { issue_id: String },
    SnapshotRequest { reply: oneshot::Sender<Snapshot> },
    RefreshRequest { reply: oneshot::Sender<RefreshAck> },
    WorkflowReloaded { workflow: WorkflowDefinition },
}
```

### Error Hierarchy (Domain-Scoped)
```rust
// Domain-scoped errors (per Issue 6 review decision)
enum ConfigError {
    MissingWorkflowFile(PathBuf),
    WorkflowParseError(String),
    WorkflowFrontMatterNotAMap,
    TemplateParseError(String),
    TemplateRenderError(String),
}

enum TrackerError {
    UnsupportedKind(String),
    MissingApiKey,
    MissingRepo,
    GitHubApiRequest(reqwest::Error),
    GitHubApiStatus(u16, String),
    GitHubGraphqlErrors(Vec<serde_json::Value>),
    GitHubRateLimited { reset_at: DateTime<Utc> },
}

enum WorkspaceError {
    CreationFailed(PathBuf, io::Error),
    PathOutsideRoot { path: PathBuf, root: PathBuf },
    HookFailed { hook: HookKind, exit_code: i32 },
    HookTimeout { hook: HookKind },
}

enum AgentError {
    NotFound(String),
    InvalidWorkspaceCwd(PathBuf),
    ResponseTimeout,
    TurnTimeout,
    TurnFailed(String),
    TurnInputRequired,
    ProcessExit(i32),
}

// Top-level wrapper
enum SymphonyError {
    Config(ConfigError),
    Tracker(TrackerError),
    Workspace(WorkspaceError),
    Agent(AgentError),
}
```
