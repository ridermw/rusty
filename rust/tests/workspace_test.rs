use std::fs;
use std::path::Path;
use std::time::Duration;

use symphony::workspace::hooks::{default_shell_executor, run_hook, HookKind, ShellExecutor};
use symphony::workspace::{
    create_for_issue, remove_workspace, sanitize_workspace_key, verify_containment, workspace_path,
    WorkspaceError,
};
use tempfile::tempdir;

struct PanicExecutor;

impl ShellExecutor for PanicExecutor {
    fn execute(
        &self,
        _script: &str,
        _cwd: &Path,
        _timeout: Duration,
    ) -> Result<(), WorkspaceError> {
        panic!("executor should not be called for empty hook scripts");
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

struct AlwaysFailExecutor;

impl ShellExecutor for AlwaysFailExecutor {
    fn execute(
        &self,
        _script: &str,
        _cwd: &Path,
        _timeout: Duration,
    ) -> Result<(), WorkspaceError> {
        Err(WorkspaceError::HookFailed {
            hook: "test_hook".to_string(),
            exit_code: 1,
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[test]
fn sanitize_workspace_key_keeps_safe_identifiers() {
    assert_eq!(sanitize_workspace_key("rusty-42"), "rusty-42");
}

#[test]
fn sanitize_workspace_key_replaces_path_separator() {
    assert_eq!(sanitize_workspace_key("ABC/123"), "ABC_123");
}

#[test]
fn sanitize_workspace_key_replaces_spaces_and_punctuation() {
    assert_eq!(sanitize_workspace_key("hello world!"), "hello_world_");
}

#[test]
fn sanitize_workspace_key_replaces_leading_symbol() {
    assert_eq!(sanitize_workspace_key("#42"), "_42");
}

#[test]
fn workspace_path_joins_root_with_sanitized_key() {
    let root = tempdir().expect("create temp dir");
    let expected = root.path().join("rusty-42");

    assert_eq!(workspace_path(root.path(), "rusty-42"), expected);
}

#[test]
fn verify_containment_passes_for_path_inside_root() {
    let root = tempdir().expect("create temp dir");
    let workspace = root.path().join("rusty-42");
    fs::create_dir_all(&workspace).expect("create workspace dir");

    let result = verify_containment(&workspace, root.path());

    assert!(result.is_ok());
}

#[test]
fn verify_containment_fails_for_traversal_outside_root() {
    let root = tempdir().expect("create temp dir");
    let workspace = root.path().join("..").join("outside");

    let result = verify_containment(&workspace, root.path());

    assert!(matches!(
        result,
        Err(WorkspaceError::PathOutsideRoot { .. })
    ));
}

#[test]
fn verify_containment_fails_when_existing_workspace_resolves_outside_root() {
    let root = tempdir().expect("create temp dir");
    let nested = root.path().join("nested");
    fs::create_dir_all(&nested).expect("create nested dir");

    let outside_name = format!(
        "escaped-{}",
        root.path()
            .file_name()
            .expect("tempdir file name")
            .to_string_lossy()
    );
    let outside = root
        .path()
        .parent()
        .expect("tempdir parent")
        .join(&outside_name);
    fs::create_dir_all(&outside).expect("create outside dir");

    let workspace = nested.join("..").join("..").join(&outside_name);
    let result = verify_containment(&workspace, root.path());

    assert!(matches!(
        result,
        Err(WorkspaceError::PathOutsideRoot { .. })
    ));

    fs::remove_dir_all(&outside).expect("remove outside dir");
}

#[test]
fn create_for_issue_creates_directory_and_marks_created_now() {
    let root = tempdir().expect("create temp dir");

    let workspace = create_for_issue(root.path(), "ABC/123").expect("create workspace");

    assert!(workspace.created_now);
    assert_eq!(workspace.workspace_key, "ABC_123");
    assert_eq!(workspace.path, root.path().join("ABC_123"));
    assert!(workspace.path.is_dir());
}

#[test]
fn create_for_issue_reuses_existing_directory() {
    let root = tempdir().expect("create temp dir");
    let existing = root.path().join("rusty-42");
    fs::create_dir_all(&existing).expect("create workspace dir");

    let workspace = create_for_issue(root.path(), "rusty-42").expect("reuse workspace");

    assert!(!workspace.created_now);
    assert_eq!(workspace.workspace_key, "rusty-42");
    assert_eq!(workspace.path, existing);
}

#[test]
fn remove_workspace_removes_directory() {
    let root = tempdir().expect("create temp dir");
    let workspace = create_for_issue(root.path(), "rusty-42").expect("create workspace");

    remove_workspace(root.path(), "rusty-42").expect("remove workspace");

    assert!(!workspace.path.exists());
}

#[test]
fn run_hook_with_none_script_is_noop() {
    let root = tempdir().expect("create temp dir");

    run_hook(
        &PanicExecutor,
        HookKind::BeforeRun,
        None,
        root.path(),
        Duration::from_secs(1),
    )
    .expect("skip missing hook");
}

#[test]
fn run_hook_after_run_failure_is_non_fatal() {
    let root = tempdir().expect("create temp dir");

    let result = run_hook(
        &AlwaysFailExecutor,
        HookKind::AfterRun,
        Some("ignored"),
        root.path(),
        Duration::from_secs(1),
    );

    assert!(result.is_ok());
}

#[test]
fn run_hook_before_run_failure_is_fatal() {
    let root = tempdir().expect("create temp dir");

    let result = run_hook(
        &AlwaysFailExecutor,
        HookKind::BeforeRun,
        Some("ignored"),
        root.path(),
        Duration::from_secs(1),
    );

    assert!(matches!(
        result,
        Err(WorkspaceError::HookFailed {
            hook,
            exit_code: 1,
        }) if hook == "test_hook"
    ));
}

#[test]
fn default_shell_executor_returns_platform_executor() {
    let executor = default_shell_executor();

    #[cfg(windows)]
    assert!(executor
        .as_any()
        .is::<symphony::workspace::hooks::PowerShellExecutor>());

    #[cfg(not(windows))]
    assert!(executor
        .as_any()
        .is::<symphony::workspace::hooks::PosixShellExecutor>());
}

#[test]
fn hook_execution_runs_simple_script() {
    let root = tempdir().expect("create temp dir");
    let executor = default_shell_executor();
    let script = if cfg!(windows) {
        "Write-Output hello"
    } else {
        "echo hello"
    };

    run_hook(
        executor.as_ref(),
        HookKind::BeforeRun,
        Some(script),
        root.path(),
        Duration::from_secs(5),
    )
    .expect("run hook script");
}
