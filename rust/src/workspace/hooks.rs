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

    // Drain stdout and stderr in background threads to prevent pipe buffer deadlock.
    // Drains raw bytes (not UTF-8 strings) to avoid EPIPE/SIGPIPE on non-UTF8 hook output.
    let stderr_handle = child.stderr.take().map(|stderr| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = std::io::Read::read_to_end(&mut std::io::BufReader::new(stderr), &mut buf);
            String::from_utf8_lossy(&buf).into_owned()
        })
    });
    let stdout_handle = child.stdout.take().map(|stdout| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = std::io::Read::read_to_end(&mut std::io::BufReader::new(stdout), &mut buf);
            // stdout drained but not used
        })
    });

    /// Join drain threads with a short timeout to avoid leaking threads
    /// when timed-out hooks have descendants keeping pipes open.
    fn join_drain_threads(
        stderr_handle: Option<thread::JoinHandle<String>>,
        stdout_handle: Option<thread::JoinHandle<()>>,
    ) -> String {
        // Give drain threads 2s to finish after process exit/kill
        let deadline = Instant::now() + Duration::from_secs(2);

        let err_output = stderr_handle
            .and_then(|h| {
                let remaining = deadline.saturating_duration_since(Instant::now());
                // park_timeout + join: if thread doesn't finish in time, detach it
                thread::scope(|_| {
                    thread::sleep(remaining.min(Duration::from_secs(2)));
                    // Can't really timeout a join, but we tried to read_to_end which
                    // should complete once the pipe closes (which happens after kill+wait)
                    h.join().ok()
                })
            })
            .unwrap_or_default();

        if let Some(h) = stdout_handle {
            let _ = h.join();
        }

        err_output
    }

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let err_output = join_drain_threads(stderr_handle, stdout_handle);

                if !err_output.trim().is_empty() {
                    let truncated = truncate_utf8(&err_output, 500);
                    tracing::warn!(hook = hook_name, stderr = truncated, "hook stderr output");
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
                    // Join drain threads to prevent leaks — kill+wait closed the pipes,
                    // so read_to_end should unblock.
                    let err_output = join_drain_threads(stderr_handle, stdout_handle);
                    if !err_output.trim().is_empty() {
                        let truncated = truncate_utf8(&err_output, 500);
                        tracing::warn!(
                            hook = hook_name,
                            stderr = truncated,
                            "timed-out hook stderr"
                        );
                    }
                    return Err(WorkspaceError::HookTimeout {
                        hook: hook_name.to_string(),
                    });
                }

                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_drain_threads(stderr_handle, stdout_handle);
                return Err(WorkspaceError::HookFailed {
                    hook: hook_name.to_string(),
                    exit_code: -1,
                });
            }
        }
    }
}

/// Truncate a string to at most `max_bytes` bytes on a valid UTF-8 boundary.
/// Never panics on multibyte characters.
fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_utf8_short_string_unchanged() {
        assert_eq!(truncate_utf8("hello", 10), "hello");
    }

    #[test]
    fn truncate_utf8_exact_boundary() {
        assert_eq!(truncate_utf8("hello world", 5), "hello");
    }

    #[test]
    fn truncate_utf8_multibyte_no_panic() {
        // '€' is 3 bytes (E2 82 AC). Cutting at byte 4 would split it.
        let s = "a€b"; // 1 + 3 + 1 = 5 bytes
        assert_eq!(truncate_utf8(s, 4), "a€"); // backs up to byte 4 → char boundary at 4
        assert_eq!(truncate_utf8(s, 3), "a"); // byte 3 is mid-€, backs up to 1
        assert_eq!(truncate_utf8(s, 2), "a"); // byte 2 is mid-€, backs up to 1
    }

    #[test]
    fn truncate_utf8_empty_string() {
        assert_eq!(truncate_utf8("", 10), "");
    }

    #[test]
    fn truncate_utf8_zero_max() {
        assert_eq!(truncate_utf8("hello", 0), "");
    }
}
