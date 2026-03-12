use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use symphony::agent::{run_agent_attempt, WorkerResult};
use symphony::config::schema::SymphonyConfig;
use symphony::tracker::memory::test_issue;
use symphony::tracker::Issue;
use symphony::workspace::hooks::{default_shell_executor, ShellExecutor};
use symphony::workspace::{workspace_path, WorkspaceError};
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

#[tokio::test]
async fn run_agent_attempt_with_valid_config_creates_workspace_and_returns_completed() {
    let workspace_root = tempdir().expect("create temp dir");
    let issue = sample_issue();
    let (update_tx, _update_rx) = mpsc::channel(8);

    let result = run_agent_attempt(
        issue.clone(),
        None,
        SymphonyConfig::default(),
        "{{ issue.identifier }}".to_string(),
        workspace_root.path().to_path_buf(),
        Arc::new(UnexpectedExecutor),
        update_tx,
    )
    .await;

    assert!(matches!(result, WorkerResult::Completed));
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
        SymphonyConfig::default(),
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
    let mut config = SymphonyConfig::default();
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
