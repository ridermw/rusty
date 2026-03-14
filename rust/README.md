# Rusty — Symphony for GitHub

[![Build Status](https://github.com/ridermw/rusty/actions/workflows/make-all.yml/badge.svg)](https://github.com/ridermw/rusty/actions/workflows/make-all.yml)

A Rust implementation of [Symphony](../SPEC.md) that orchestrates coding agents against GitHub Issues using Copilot CLI.

> [!WARNING]
> Rusty runs coding agents autonomously. Use in trusted environments only.

## Quick Start

### Prerequisites

- **Rust** stable toolchain ([install](https://rustup.rs/))
- **GitHub CLI** (`gh`) — [install](https://cli.github.com/)
- **Copilot CLI** (`copilot`) — [install](https://docs.github.com/en/copilot/github-copilot-in-the-cli)
- **PowerShell 7+** (Windows only, for hooks) — [install](https://learn.microsoft.com/en-us/powershell/scripting/install/installing-powershell-on-windows)
- GitHub authentication via **`GITHUB_TOKEN`**, **`GH_TOKEN`**, or **`gh auth login`** with scopes: `repo`, `read:discussion`, `project`

### 1. Build

```bash
cd rust

# Debug build (fast compile, slower runtime)
cargo build

# Release build (slow compile, optimized binary)
cargo build --release
```

| Build | Binary location | Use for |
|---|---|---|
| Debug | `target/debug/rusty` | Development, testing |
| Release | `target/release/rusty` | Production, deployment |

### 2. Setup

Run the interactive setup checker to verify your environment:

```bash
# From the rust/ directory after building
target/debug/rusty setup

# Or from anywhere with the release binary
./rusty setup
```

This checks GitHub auth (`GITHUB_TOKEN`, `GH_TOKEN`, or `gh auth login`), `WORKFLOW.md`, Copilot CLI, GitHub CLI, and the logs directory.

### 3. Configure

Copy the default workflow to wherever you'll run the binary:

```bash
# If running from rust/
# WORKFLOW.md is already here

# If deploying the release binary elsewhere
cp rust/WORKFLOW.md /path/to/deploy/
cp target/release/rusty /path/to/deploy/
```

Edit `WORKFLOW.md` to set your repo:

```yaml
tracker:
  kind: github
  owner: "your-username"
  repo: "your-repo"
```

### 4. Run

```bash
# Start the daemon
rusty run --yolo

# Start with web dashboard on port 4000
rusty run --yolo --port 4000

# Custom workflow path and logs directory
rusty run --yolo --port 4000 --logs-root ./my-logs path/to/WORKFLOW.md
```

### 5. Monitor

- **Terminal**: Status prints to stderr automatically
- **Web dashboard**: `http://127.0.0.1:4000/` (when `--port` is set)
- **JSON API**: `GET http://127.0.0.1:4000/api/v1/state`
- **Logs**: `./logs/rusty.log` (daily rotation)

## CLI Reference

```
rusty — Rusty orchestration daemon for GitHub Issues + Copilot CLI

Usage: rusty <COMMAND>

Commands:
  run    Start the orchestration daemon
  setup  Interactive first-time setup
  help   Print this message or the help of the given subcommand

Options:
  -h, --help     Print help
  -V, --version  Print version
```

### `rusty run`

```
Start the orchestration daemon

Usage: rusty run [OPTIONS] [WORKFLOW_PATH]

Arguments:
  [WORKFLOW_PATH]  Path to WORKFLOW.md file [default: WORKFLOW.md]

Options:
      --port <PORT>            HTTP server port
      --logs-root <LOGS_ROOT>  Log files directory [default: ./logs]
      --yolo                   Acknowledge autonomous agent execution (required)
  -h, --help                   Print help
```

### `rusty setup`

Interactive environment checker. Verifies:
1. GitHub auth is available via `GITHUB_TOKEN`, `GH_TOKEN`, or `gh auth login`
2. `WORKFLOW.md` exists (checks current dir and `rust/`)
3. Copilot CLI (`copilot`) is installed
4. GitHub CLI (`gh`) is installed
5. Logs directory exists (creates if missing)

## Development

### Debug workflow

```bash
cd rust
cargo build                        # Build debug binary
cargo test                         # Run all tests
cargo clippy -- -D warnings        # Lint
cargo fmt --check                  # Format check
target/debug/rusty setup           # Verify environment
target/debug/rusty run --yolo      # Run with debug binary
```

### Release workflow

```bash
cd rust
cargo build --release              # Build optimized binary
cargo test                         # Verify tests pass
target/release/rusty setup         # Verify environment
target/release/rusty run --yolo    # Run with release binary
```

### Deploy checklist

- [ ] `cargo build --release` succeeds
- [ ] `cargo test` — all tests pass
- [ ] `cargo clippy -- -D warnings` — clean
- [ ] Copy `target/release/rusty` to deploy location
- [ ] Copy `WORKFLOW.md` to same directory as binary
- [ ] Set `GITHUB_TOKEN`/`GH_TOKEN` in environment or run `gh auth login`
- [ ] Run `rusty setup` to verify
- [ ] Run `rusty run --yolo` to start

## How it differs from the Elixir reference

| Aspect | Elixir reference | Rusty |
|---|---|---|
| Issue tracker | Linear (GraphQL) | **GitHub Issues** (REST + GraphQL) |
| Auth | `LINEAR_API_KEY` | `GITHUB_TOKEN` / `GH_TOKEN` / `gh auth login` |
| Coding agent | Codex app-server | **Copilot CLI** (`copilot --acp --stdio`) |
| Dynamic tool | `linear_graphql` | **`github_graphql`** |
| Runtime | Elixir/OTP/BEAM | **Single static binary**, no runtime deps |
| Hooks (Windows) | N/A (Unix only) | **PowerShell 7+** via `ShellExecutor` trait |

## Architecture

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

## Configuration

See [`WORKFLOW.md`](WORKFLOW.md) for the full workflow configuration template.

### Config Reference

The YAML front matter in `WORKFLOW.md` (between `---` fences) configures Rusty.
All sections and fields are optional and fall back to the defaults shown below.

#### `tracker`

Issue tracker connection. Only `github` is supported.

| Field | Type | Default | Description |
|---|---|---|---|
| `kind` | string | _(none)_ | Tracker type. Must be `"github"`. |
| `endpoint` | string | _(none)_ | GitHub API endpoint URL (for GHES). |
| `api_key` | string | _(none)_ | Auth token. Resolved from `GITHUB_TOKEN`, `GH_TOKEN`, or `gh auth token` when omitted. |
| `owner` | string | _(none)_ | Repository owner (e.g., `"ridermw"`). |
| `repo` | string | _(none)_ | Repository name or `"owner/repo"` combined format. |
| `active_states` | list | `["open"]` | Issue states considered active. |
| `terminal_states` | list | `["closed"]` | Issue states considered terminal. |
| `labels` | list | `[]` | General issue labels to filter by. |
| `active_issue_labels` | list | `[]` | Labels that map to active states (e.g., `["todo", "in_progress"]`). Merged with `active_states`. |
| `terminal_issue_labels` | list | `[]` | Labels that map to terminal states (e.g., `["done"]`). Merged with `terminal_states`. |
| `state_labels` | map | `{}` | Custom state-to-label mappings. |
| `assignee` | string | _(none)_ | Filter issues to a specific assignee. |

#### `polling`

How often to poll the tracker for changes.

| Field | Type | Default | Description |
|---|---|---|---|
| `interval_ms` | integer | `30000` | Polling interval in milliseconds. |

#### `workspace`

Per-issue workspace directory settings.

| Field | Type | Default | Description |
|---|---|---|---|
| `root` | string | `<TEMP_DIR>/rusty_workspaces` | Root directory for workspaces. Supports `~` (home) expansion. |

#### `hooks`

Shell commands executed at workspace lifecycle events.

| Field | Type | Default | Description |
|---|---|---|---|
| `after_create` | string | _(none)_ | Command run after a workspace is created. |
| `before_run` | string | _(none)_ | Command run before the agent starts. |
| `after_run` | string | _(none)_ | Command run after the agent completes. |
| `before_remove` | string | _(none)_ | Command run before a workspace is deleted. |
| `timeout_ms` | integer | `60000` | Hook execution timeout in milliseconds. |

#### `agent`

Agent execution and concurrency settings.

| Field | Type | Default | Description |
|---|---|---|---|
| `max_concurrent_agents` | integer | `10` | Maximum agent instances running globally. |
| `max_turns` | integer | `20` | Maximum conversation turns per agent session. |
| `max_retry_backoff_ms` | integer | `300000` | Maximum exponential backoff between retries (5 min). |
| `max_concurrent_agents_by_state` | map | `{}` | Per-state concurrency limits (keys are lowercased). |
| `command` | string | `"copilot --acp --yolo --no-ask-user"` | Full Copilot invocation command. |
| `turn_timeout_ms` | integer | `3600000` | Single turn timeout (1 hour). |
| `read_timeout_ms` | integer | `30000` | Read timeout for agent output. |
| `stall_timeout_ms` | integer | `300000` | Agent stall detection timeout (5 min). |
| `approval_policy` | string | `"auto-approve"` | Approval behavior (`"auto-approve"` or `"require-approval"`). |

#### `copilot`

Copilot CLI integration settings.

| Field | Type | Default | Description |
|---|---|---|---|
| `command` | string | `"copilot"` | Base Copilot CLI command path. |
| `chat_command` | string | _(none)_ | Custom chat subcommand (e.g., `"copilot chat"`). |
| `approval_policy` | string | `"never"` | Approval policy (`"never"`, `"auto-approve"`, or `"require-approval"`). |
| `thread_sandbox` | string | _(none)_ | Sandbox mode for threads (e.g., `"workspace_write"`). |
| `turn_sandbox_policy` | YAML object | _(none)_ | Sandbox policy for individual turns (passed as raw YAML). |

#### `github`

GitHub CLI and repository defaults.

| Field | Type | Default | Description |
|---|---|---|---|
| `cli_command` | string | `"gh"` | GitHub CLI command path. |
| `default_branch` | string | `"main"` | Default branch for pull requests. |
| `required_pr_label` | string | _(none)_ | Label required on all created pull requests. |

## License

This project is licensed under the [Apache License 2.0](../LICENSE).
