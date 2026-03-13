use std::any::Any;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tracing::{error, info, warn};

use super::WorkspaceError;

/// Platform-aware shell executor for running hook scripts.
pub trait ShellExecutor: Send + Sync + 'static {
    /// Execute a script string in the given working directory with timeout.
    fn execute(&self, script: &str, cwd: &Path, timeout: Duration) -> Result<(), WorkspaceError>;

    /// Expose the concrete type for tests and diagnostics.
    fn as_any(&self) -> &dyn Any;
}

/// Unix shell executor: sh -lc <script>
#[derive(Debug, Default)]
pub struct PosixShellExecutor;

impl ShellExecutor for PosixShellExecutor {
    fn execute(&self, script: &str, cwd: &Path, timeout: Duration) -> Result<(), WorkspaceError> {
        run_shell_command("sh", &["-lc", script], cwd, timeout, "posix_hook")
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Windows PowerShell executor: pwsh -Command <script>
#[derive(Debug, Default)]
pub struct PowerShellExecutor;

impl ShellExecutor for PowerShellExecutor {
    fn execute(&self, script: &str, cwd: &Path, timeout: Duration) -> Result<(), WorkspaceError> {
        run_shell_command(
            "pwsh",
            &["-NoProfile", "-Command", script],
            cwd,
            timeout,
            "pwsh_hook",
        )
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Create the appropriate shell executor for the current platform.
pub fn default_shell_executor() -> Box<dyn ShellExecutor> {
    if cfg!(windows) {
        Box::new(PowerShellExecutor)
    } else {
        Box::new(PosixShellExecutor)
    }
}

/// Hook kinds for logging context
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    AfterCreate,
    BeforeRun,
    AfterRun,
    BeforeRemove,
}

impl std::fmt::Display for HookKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookKind::AfterCreate => write!(f, "after_create"),
            HookKind::BeforeRun => write!(f, "before_run"),
            HookKind::AfterRun => write!(f, "after_run"),
            HookKind::BeforeRemove => write!(f, "before_remove"),
        }
    }
}

/// Run a hook with the given kind, respecting failure semantics:
/// - after_create, before_run: fatal (returns error)
/// - after_run, before_remove: best-effort (logs error, returns Ok)
pub fn run_hook(
    executor: &dyn ShellExecutor,
    kind: HookKind,
    script: Option<&str>,
    cwd: &Path,
    timeout: Duration,
) -> Result<(), WorkspaceError> {
    let script = match script {
        Some(script) if !script.trim().is_empty() => script,
        _ => return Ok(()),
    };

    info!(hook = %kind, cwd = %cwd.display(), "running hook");

    match executor.execute(script, cwd, timeout) {
        Ok(()) => {
            info!(hook = %kind, "hook completed successfully");
            Ok(())
        }
        Err(error_value) => match kind {
            HookKind::AfterCreate | HookKind::BeforeRun => {
                error!(hook = %kind, error = %error_value, "hook failed (fatal)");
                Err(error_value)
            }
            HookKind::AfterRun | HookKind::BeforeRemove => {
                warn!(hook = %kind, error = %error_value, "hook failed (non-fatal, continuing)");
                Ok(())
            }
        },
    }
}

/// Internal: run a shell command with timeout.
fn run_shell_command(
    shell: &str,
    args: &[&str],
    cwd: &Path,
    timeout: Duration,
    hook_name: &str,
) -> Result<(), WorkspaceError> {
    let mut child = Command::new(shell)
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            tracing::error!(hook = hook_name, error = %e, "failed to spawn hook shell");
            WorkspaceError::HookFailed {
                hook: hook_name.to_string(),
                exit_code: -1,
            }
        })?;

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Capture and log output for debugging
                if let Some(mut stderr) = child.stderr.take() {
                    let mut err_output = String::new();
                    let _ = std::io::Read::read_to_string(&mut stderr, &mut err_output);
                    if !err_output.trim().is_empty() {
                        let truncated = if err_output.len() > 500 {
                            &err_output[..500]
                        } else {
                            &err_output
                        };
                        tracing::warn!(hook = hook_name, stderr = truncated, "hook stderr output");
                    }
                }

                return if status.success() {
                    Ok(())
                } else {
                    Err(WorkspaceError::HookFailed {
                        hook: hook_name.to_string(),
                        exit_code: status.code().unwrap_or(-1),
                    })
                };
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(WorkspaceError::HookTimeout {
                        hook: hook_name.to_string(),
                    });
                }

                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WorkspaceError::HookFailed {
                    hook: hook_name.to_string(),
                    exit_code: -1,
                });
            }
        }
    }
}
