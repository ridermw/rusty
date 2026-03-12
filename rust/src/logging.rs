use std::error::Error;
use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Initialize the logging system with structured JSON output.
///
/// - Always logs to stderr with human-readable format
/// - If `log_dir` is provided, also logs JSON to a rotating file
pub fn init_logging(log_dir: Option<&Path>) -> Result<Option<WorkerGuard>, Box<dyn Error>> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    if let Some(dir) = log_dir {
        std::fs::create_dir_all(dir)?;

        let file_appender = tracing_appender::rolling::daily(dir, "symphony.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

        let file_layer = fmt::layer()
            .json()
            .with_writer(non_blocking)
            .with_target(true)
            .with_thread_ids(true);

        let stderr_layer = fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(false)
            .compact();

        tracing_subscriber::registry()
            .with(env_filter)
            .with(file_layer)
            .with(stderr_layer)
            .try_init()?;

        Ok(Some(guard))
    } else {
        let stderr_layer = fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(false)
            .compact();

        tracing_subscriber::registry()
            .with(env_filter)
            .with(stderr_layer)
            .try_init()?;

        Ok(None)
    }
}

/// Symphony logging context is attached with standard `tracing` spans and fields.
///
/// Usage:
/// ```no_run
/// use tracing::info;
///
/// let issue_id = "abc123";
/// let issue_identifier = "rusty-42";
/// let span = tracing::info_span!("dispatch", issue_id, issue_identifier);
/// let _guard = span.enter();
/// info!("dispatching issue");
/// ```
///
/// For session-scoped logging:
/// ```no_run
/// let span = tracing::info_span!(
///     "agent_session",
///     issue_id = "abc123",
///     issue_identifier = "rusty-42",
///     session_id = "thread1-turn1"
/// );
/// ```
#[allow(dead_code)]
const LOGGING_CONTEXT_DOCS: () = ();
