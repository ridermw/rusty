use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rusty::agent::{run_agent_attempt, WorkerResult};
use rusty::config::schema::RustyConfig;
use rusty::tracker::memory::test_issue;
use rusty::tracker::Issue;
use rusty::workspace::hooks::{default_shell_executor, ShellExecutor};
use rusty::workspace::{workspace_path, WorkspaceError};
use tempfile::tempdir;
use tokio::sync::mpsc;

struct UnexpectedExecutor;

impl ShellExecutor for UnexpectedExecutor {
    fn execute(
        &self,
        _script: &str,
        _cwd: &Path,
        _timeout: Duration,
    ) -> Result<(), WorkspaceError> {
        panic!("executor should not be called when hooks are disabled");
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn sample_issue() -> Issue {
    let mut issue = test_issue("1", "ISSUE-123", "Implement agent runner", "open", Some(1));
    issue.description = Some("Run workspace, hooks, and prompt lifecycle".to_string());
    issue
}

fn marker_script(marker_name: &str) -> String {
    if cfg!(windows) {
        format!("Set-Content -Path '{marker_name}' -Value 'after_run' -NoNewline")
    } else {
        format!("printf 'after_run' > '{marker_name}'")
    }
}

fn write_fake_acp_server(script_path: &Path) {
    let script = r#"import json
import sys

session_id = "session-1"
turn_id = "turn-1"

for raw in sys.stdin:
    raw = raw.strip()
    if not raw:
        continue

    message = json.loads(raw)
    method = message.get("method")
    message_id = message.get("id")

    if method == "initialize":
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": message_id, "result": {"capabilities": {}}}) + "\n")
        sys.stdout.flush()
    elif method == "initialized":
        continue
    elif method == "session/create":
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": message_id, "result": {"session": {"id": session_id}}}) + "\n")
        sys.stdout.flush()
    elif method == "session/message/send":
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": message_id, "result": {"turn": {"id": turn_id}}}) + "\n")
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "method": "session/message/completed", "params": {"turnId": turn_id}}) + "\n")
        sys.stdout.flush()
    elif method == "approval/respond":
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": message_id, "result": {"approved": True}}) + "\n")
        sys.stdout.flush()
    else:
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": message_id, "result": {}}) + "\n")
        sys.stdout.flush()
"#;

    std::fs::write(script_path, script).expect("write fake ACP server script");
}

fn fake_acp_command(script_path: &Path) -> String {
    let script = script_path.to_string_lossy().into_owned();
    let candidates: &[(&str, &[&str])] = if cfg!(windows) {
        &[("python", &[]), ("py", &["-3"]), ("py", &[])]
    } else {
        &[("python3", &[]), ("python", &[])]
    };

    for (command, args) in candidates {
        if std::process::Command::new(command)
            .args(*args)
            .arg("--version")
            .output()
            .is_ok()
        {
            let mut parts = vec![(*command).to_string()];
            parts.extend(args.iter().map(|arg| (*arg).to_string()));
            parts.push(script.clone());
            return parts.join(" ");
        }
    }

    panic!("no Python interpreter available for fake ACP test server");
}

#[tokio::test]
async fn run_agent_attempt_with_valid_config_creates_workspace_and_returns_completed() {
    let workspace_root = tempdir().expect("create temp dir");
    let issue = sample_issue();
    let script_path = workspace_root.path().join("fake_acp_server.py");
    let (update_tx, _update_rx) = mpsc::channel(8);
    let mut config = RustyConfig::default();

    write_fake_acp_server(&script_path);
    config.agent.command = fake_acp_command(&script_path);
    config.agent.max_turns = 1;

    let result = run_agent_attempt(
        issue.clone(),
        None,
        config,
        "{{ issue.identifier }}".to_string(),
        workspace_root.path().to_path_buf(),
        Arc::new(UnexpectedExecutor),
        update_tx,
    )
    .await;

    assert!(
        matches!(result, WorkerResult::Completed),
        "expected Completed result, got {result:?}"
    );
    assert!(workspace_path(workspace_root.path(), &issue.identifier).is_dir());
}

#[tokio::test]
async fn run_agent_attempt_with_bad_workspace_root_returns_failed() {
    let temp = tempdir().expect("create temp dir");
    let bad_root = temp.path().join("workspace-root.txt");
    std::fs::write(&bad_root, "not a directory").expect("write temp file");
    let (update_tx, _update_rx) = mpsc::channel(8);

    let result = run_agent_attempt(
        sample_issue(),
        None,
        RustyConfig::default(),
        "{{ issue.identifier }}".to_string(),
        bad_root,
        Arc::new(UnexpectedExecutor),
        update_tx,
    )
    .await;

    match result {
        WorkerResult::Failed(message) => assert!(message.contains("workspace:")),
        WorkerResult::Completed => panic!("expected workspace creation failure"),
    }
}

#[test]
fn worker_result_completed_and_failed_pattern_matching_works() {
    let completed = WorkerResult::Completed;
    assert!(matches!(completed, WorkerResult::Completed));

    let failed = WorkerResult::Failed("boom".to_string());
    match failed {
        WorkerResult::Failed(message) => assert_eq!(message, "boom"),
        WorkerResult::Completed => panic!("expected failure variant"),
    }
}

#[tokio::test]
async fn after_run_hook_is_called_even_when_prompt_rendering_fails() {
    let workspace_root = tempdir().expect("create temp dir");
    let issue = sample_issue();
    let marker_name = "runner-after-run.marker";
    let mut config = RustyConfig::default();
    config.hooks.after_run = Some(marker_script(marker_name));
    config.hooks.timeout_ms = 5_000;

    let (update_tx, _update_rx) = mpsc::channel(8);
    let shell_executor: Arc<dyn ShellExecutor> = Arc::from(default_shell_executor());

    let result = run_agent_attempt(
        issue.clone(),
        Some(2),
        config,
        "{{ unknown_var }}".to_string(),
        workspace_root.path().to_path_buf(),
        shell_executor,
        update_tx,
    )
    .await;

    match result {
        WorkerResult::Failed(message) => assert!(message.contains("prompt:")),
        WorkerResult::Completed => panic!("expected prompt rendering failure"),
    }

    let marker_path = workspace_path(workspace_root.path(), &issue.identifier).join(marker_name);
    assert!(
        marker_path.is_file(),
        "after_run hook should create marker file"
    );
}

#[tokio::test]
async fn agent_runner_fails_on_before_run_hook_error() {
    use rusty::agent::{run_agent_attempt, AgentUpdate, WorkerResult};
    use rusty::config::schema::RustyConfig;
    use rusty::tracker::memory::test_issue;
    use rusty::workspace::hooks::default_shell_executor;
    use std::sync::Arc;
    use tempfile::tempdir;

    let tmp = tempdir().unwrap();
    let issue = test_issue("1", "repo-1", "Test", "open", Some(1));

    let mut config = RustyConfig::default();
    config.hooks.before_run = Some("exit 1".to_string());
    config.hooks.timeout_ms = 5_000;
    config.agent.command = "echo test".to_string();

    let (tx, _rx) = tokio::sync::mpsc::channel::<AgentUpdate>(16);
    let shell = Arc::from(default_shell_executor());

    let result = run_agent_attempt(
        issue,
        None,
        config,
        "prompt".to_string(),
        tmp.path().to_path_buf(),
        shell,
        tx,
    )
    .await;

    match result {
        WorkerResult::Failed(msg) => {
            assert!(
                msg.contains("hook") || msg.contains("before_run"),
                "expected hook error, got: {msg}"
            );
        }
        WorkerResult::Completed => panic!("should have failed on before_run hook"),
    }
}

#[tokio::test]
async fn agent_runner_attempts_to_launch_agent_process() {
    use rusty::agent::{run_agent_attempt, AgentUpdate, WorkerResult};
    use rusty::config::schema::RustyConfig;
    use rusty::tracker::memory::test_issue;
    use rusty::workspace::hooks::default_shell_executor;
    use std::sync::Arc;
    use tempfile::tempdir;

    let tmp = tempdir().unwrap();
    let issue = test_issue("1", "repo-1", "Test issue", "open", Some(1));

    let mut config = RustyConfig::default();
    config.agent.command = "nonexistent_binary_xyz_123".to_string();

    let (tx, _rx) = tokio::sync::mpsc::channel::<AgentUpdate>(16);
    let shell = Arc::from(default_shell_executor());

    let result = run_agent_attempt(
        issue,
        None,
        config,
        "Test prompt {{ issue.identifier }}".to_string(),
        tmp.path().to_path_buf(),
        shell,
        tx,
    )
    .await;

    match result {
        WorkerResult::Failed(msg) => {
            assert!(
                msg.contains("not found") || msg.contains("agent") || msg.contains("launch"),
                "expected agent launch error, got: {msg}"
            );
        }
        WorkerResult::Completed => {
            panic!("agent returned Completed without launching a process — Bug 35.1 not fixed");
        }
    }
}
