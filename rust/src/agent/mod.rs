pub mod acp_client;
pub mod dynamic_tool;
pub mod log_parser;

pub use acp_client::{
    AcpClient, AgentError, AgentEvent, ChildGuard, JsonRpcMessage, JsonRpcResponse, TurnResult,
};

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{error, info, info_span, warn, Instrument};

use crate::config::schema::{HooksConfig, RustyConfig};
use crate::prompt;
use crate::tracker::Issue;
use crate::workspace::{
    self,
    hooks::{self, HookKind, ShellExecutor},
};

/// Result of a complete agent run for one issue.
#[derive(Debug)]
pub enum WorkerResult {
    /// Agent completed normally (issue may still be active — orchestrator decides retry).
    Completed,
    /// Agent failed with an error.
    Failed(String),
}

/// Run a complete agent attempt for one issue.
/// This is spawned as a tokio task by the orchestrator.
pub async fn run_agent_attempt(
    issue: Issue,
    attempt: Option<u32>,
    config: RustyConfig,
    prompt_template: String,
    workspace_root: std::path::PathBuf,
    shell_executor: Arc<dyn ShellExecutor>,
    update_tx: mpsc::Sender<AgentUpdate>,
    previous_session_id: Option<String>,
) -> WorkerResult {
    let span = info_span!(
        "agent_run",
        issue_id = %issue.id,
        issue_identifier = %issue.identifier
    );

    async move {
        let issue_id = issue.id.clone();
        let ws = match workspace::create_for_issue(&workspace_root, &issue.identifier) {
            Ok(ws) => ws,
            Err(e) => {
                let error_message = format!("workspace: {e}");
                error!(error = %e, "workspace creation failed");
                emit_update(
                    &update_tx,
                    AgentUpdate::new(&issue_id, "failed").with_message(error_message.clone()),
                )
                .await;
                return WorkerResult::Failed(error_message);
            }
        };

        let ws_path = ws.path.clone();
        let timeout = Duration::from_millis(config.hooks.timeout_ms);
        let workspace_event = if ws.created_now {
            "workspace_created"
        } else {
            "workspace_reused"
        };
        emit_update(
            &update_tx,
            AgentUpdate::new(&issue_id, workspace_event)
                .with_message(ws_path.display().to_string()),
        )
        .await;

        if ws.created_now {
            if let Err(e) = hooks::run_hook(
                shell_executor.as_ref(),
                HookKind::AfterCreate,
                config.hooks.after_create.as_deref(),
                &ws_path,
                timeout,
            ) {
                let error_message = format!("after_create hook: {e}");
                error!(error = %e, "after_create hook failed");
                emit_update(
                    &update_tx,
                    AgentUpdate::new(&issue_id, "failed").with_message(error_message.clone()),
                )
                .await;
                return WorkerResult::Failed(error_message);
            }
        }

        if let Err(e) = hooks::run_hook(
            shell_executor.as_ref(),
            HookKind::BeforeRun,
            config.hooks.before_run.as_deref(),
            &ws_path,
            timeout,
        ) {
            let error_message = format!("before_run hook: {e}");
            error!(error = %e, "before_run hook failed");
            run_after_run_hook(&shell_executor, &config.hooks, &ws_path);
            emit_update(
                &update_tx,
                AgentUpdate::new(&issue_id, "failed").with_message(error_message.clone()),
            )
            .await;
            return WorkerResult::Failed(error_message);
        }

        let rendered_prompt = match prompt::render_prompt(&prompt_template, &issue, attempt) {
            Ok(prompt) => prompt,
            Err(e) => {
                let error_message = format!("prompt: {e}");
                error!(error = %e, "prompt rendering failed");
                run_after_run_hook(&shell_executor, &config.hooks, &ws_path);
                emit_update(
                    &update_tx,
                    AgentUpdate::new(&issue_id, "failed").with_message(error_message.clone()),
                )
                .await;
                return WorkerResult::Failed(error_message);
            }
        };

        info!(
            turns_max = config.agent.max_turns,
            prompt_len = rendered_prompt.len(),
            "starting agent session"
        );
        emit_update(
            &update_tx,
            AgentUpdate::new(&issue_id, "started")
                .with_message(format!("prompt rendered ({} bytes)", rendered_prompt.len())),
        )
        .await;

        let launch_cmd = crate::config::agent_launch_command(&config);
        let command_parts: Vec<&str> = launch_cmd.split_whitespace().collect();
        let (cmd, base_args): (&str, &[&str]) = match command_parts.split_first() {
            Some((cmd, args)) => (*cmd, args),
            None => {
                warn!("agent.command is empty, falling back to copilot --acp");
                ("copilot", &["--acp", "--yolo", "--no-ask-user"][..])
            }
        };

        // When log_dir is configured, create a per-session subdirectory and
        // inject `--log-dir <path>` into the agent command so Copilot CLI
        // writes its internal logs there. We parse those logs post-session
        // to extract token usage metrics not available over ACP.
        let session_log_dir = config.agent.log_dir.as_ref().map(|base| {
            let session_stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
            let dir = std::path::PathBuf::from(base)
                .join(format!("{}-{}", issue.identifier, session_stamp));
            if let Err(e) = std::fs::create_dir_all(&dir) {
                warn!(dir = %dir.display(), error = %e, "failed to create agent log directory");
            }
            dir
        });

        let mut owned_args: Vec<String> = base_args.iter().map(|s| s.to_string()).collect();
        if let Some(ref log_dir) = session_log_dir {
            owned_args.push("--log-dir".to_string());
            owned_args.push(log_dir.to_string_lossy().to_string());
            info!(log_dir = %log_dir.display(), "injecting --log-dir for token metrics");
        }
        let final_args: Vec<&str> = owned_args.iter().map(|s| s.as_str()).collect();

        let mut client = match AcpClient::launch(cmd, &final_args, &ws_path) {
            Ok(client) => client,
            Err(e) => {
                let error_message = format!("agent launch: {e}");
                error!(error = %e, command = cmd, "failed to launch agent");
                run_after_run_hook(&shell_executor, &config.hooks, &ws_path);
                emit_update(
                    &update_tx,
                    AgentUpdate::new(&issue_id, "failed").with_message(error_message.clone()),
                )
                .await;
                return WorkerResult::Failed(error_message);
            }
        };

        if let Err(e) = client.handshake(config.agent.read_timeout_ms).await {
            let error_message = format!("handshake: {e}");
            error!(error = %e, "ACP handshake failed");
            let _ = client.stop().await;
            run_after_run_hook(&shell_executor, &config.hooks, &ws_path);
            emit_update(
                &update_tx,
                AgentUpdate::new(&issue_id, "failed").with_message(error_message.clone()),
            )
            .await;
            return WorkerResult::Failed(error_message);
        }
        info!("ACP handshake completed");

        let session_id = match try_load_or_create_session(
            &mut client,
            &ws_path,
            &config,
            previous_session_id.as_deref(),
        )
        .await
        {
            Ok(session_id) => session_id,
            Err(e) => {
                let error_message = format!("session: {e}");
                error!(error = %e, "failed to create/load ACP session");
                let _ = client.stop().await;
                run_after_run_hook(&shell_executor, &config.hooks, &ws_path);
                emit_update(
                    &update_tx,
                    AgentUpdate::new(&issue_id, "failed").with_message(error_message.clone()),
                )
                .await;
                return WorkerResult::Failed(error_message);
            }
        };

        emit_update(
            &update_tx,
            AgentUpdate {
                issue_id: issue_id.clone(),
                event: "session_started".to_string(),
                message: Some(format!("session {session_id}")),
                input_tokens: None,
                output_tokens: None,
                total_tokens: None,
                session_id: Some(session_id.clone()),
            },
        )
        .await;

        let max_turns = config.agent.max_turns.max(1);
        let mut turn_number = 1usize;
        let title = format!("{}: {}", issue.identifier, issue.title);

        loop {
            let turn_prompt = if turn_number == 1 {
                rendered_prompt.clone()
            } else {
                format!(
                    "Continue working on {}. This is turn {}/{}. Resume from current workspace state.",
                    issue.identifier, turn_number, max_turns
                )
            };

            info!(turn = turn_number, max_turns, "starting turn");

            let turn_result = client
                .send_turn(
                    &session_id,
                    &turn_prompt,
                    &title,
                    &ws_path,
                    &config.agent.approval_policy,
                    None,
                    config.agent.turn_timeout_ms,
                    |event| {
                        let update = match &event {
                            crate::agent::AgentEvent::TokenUsage {
                                input,
                                output,
                                total,
                            } => AgentUpdate {
                                issue_id: issue_id.clone(),
                                event: "token_usage".to_string(),
                                message: None,
                                input_tokens: Some(*input),
                                output_tokens: Some(*output),
                                total_tokens: Some(*total),
                                session_id: Some(session_id.clone()),
                            },
                            crate::agent::AgentEvent::Notification { message } => AgentUpdate {
                                issue_id: issue_id.clone(),
                                event: "notification".to_string(),
                                message: Some(message.clone()),
                                input_tokens: None,
                                output_tokens: None,
                                total_tokens: None,
                                session_id: Some(session_id.clone()),
                            },
                            _ => AgentUpdate {
                                issue_id: issue_id.clone(),
                                event: format!("{event:?}"),
                                message: None,
                                input_tokens: None,
                                output_tokens: None,
                                total_tokens: None,
                                session_id: Some(session_id.clone()),
                            },
                        };
                        let _ = update_tx.try_send(update);
                    },
                )
                .await;

            match turn_result {
                Ok(crate::agent::TurnResult::Completed { turn_id }) => {
                    info!(%turn_id, turn = turn_number, "turn completed");
                    emit_update(
                        &update_tx,
                        AgentUpdate::new(&issue_id, "turn_completed")
                            .with_message(format!("turn {turn_number} completed"))
                            .with_session(session_id.clone()),
                    )
                    .await;
                }
                Ok(crate::agent::TurnResult::Failed { turn_id, reason }) => {
                    let error_message = format!("turn failed: {reason}");
                    error!(%turn_id, %reason, "turn failed");
                    let _ = client.stop().await;
                    emit_log_token_usage(&session_log_dir, &update_tx, &issue_id, &session_id).await;
                    run_after_run_hook(&shell_executor, &config.hooks, &ws_path);
                    emit_update(
                        &update_tx,
                        AgentUpdate::new(&issue_id, "failed").with_message(error_message.clone()),
                    )
                    .await;
                    return WorkerResult::Failed(error_message);
                }
                Ok(crate::agent::TurnResult::Cancelled { turn_id }) => {
                    let error_message = "turn cancelled".to_string();
                    warn!(%turn_id, "turn cancelled");
                    let _ = client.stop().await;
                    emit_log_token_usage(&session_log_dir, &update_tx, &issue_id, &session_id).await;
                    run_after_run_hook(&shell_executor, &config.hooks, &ws_path);
                    emit_update(
                        &update_tx,
                        AgentUpdate::new(&issue_id, "failed").with_message(error_message.clone()),
                    )
                    .await;
                    return WorkerResult::Failed(error_message);
                }
                Err(e) => {
                    let error_message = format!("turn error: {e}");
                    error!(error = %e, "turn error");
                    let _ = client.stop().await;
                    emit_log_token_usage(&session_log_dir, &update_tx, &issue_id, &session_id).await;
                    run_after_run_hook(&shell_executor, &config.hooks, &ws_path);
                    emit_update(
                        &update_tx,
                        AgentUpdate::new(&issue_id, "failed").with_message(error_message.clone()),
                    )
                    .await;
                    return WorkerResult::Failed(error_message);
                }
            }

            if turn_number >= max_turns {
                info!(turn = turn_number, "max turns reached");
                break;
            }

            turn_number += 1;
        }

        let _ = client.stop().await;

        // Scan Copilot log files for token usage metrics that aren't
        // available over ACP in Copilot CLI 1.0.5.
        emit_log_token_usage(&session_log_dir, &update_tx, &issue_id, &session_id).await;

        run_after_run_hook(&shell_executor, &config.hooks, &ws_path);

        info!(turns_max = max_turns, "agent run completed");
        emit_update(&update_tx, AgentUpdate::new(&issue_id, "completed")).await;
        WorkerResult::Completed
    }
    .instrument(span)
    .await
}

/// Agent update sent from worker to orchestrator.
#[derive(Debug, Clone)]
pub struct AgentUpdate {
    pub issue_id: String,
    pub event: String,
    pub message: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub session_id: Option<String>,
}

impl AgentUpdate {
    fn new(issue_id: &str, event: impl Into<String>) -> Self {
        Self {
            issue_id: issue_id.to_string(),
            event: event.into(),
            message: None,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            session_id: None,
        }
    }

    fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    fn with_session(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }
}

/// Best-effort after_run hook — never returns error.
fn run_after_run_hook(
    executor: &Arc<dyn ShellExecutor>,
    hooks_config: &HooksConfig,
    ws_path: &Path,
) {
    let timeout = Duration::from_millis(hooks_config.timeout_ms);
    let _ = hooks::run_hook(
        executor.as_ref(),
        HookKind::AfterRun,
        hooks_config.after_run.as_deref(),
        ws_path,
        timeout,
    );
}

async fn emit_update(update_tx: &mpsc::Sender<AgentUpdate>, update: AgentUpdate) {
    if let Err(e) = update_tx.send(update.clone()).await {
        warn!(
            issue_id = %update.issue_id,
            event = %update.event,
            error = %e,
            "agent update receiver dropped"
        );
    }
}

/// Best-effort scan of Copilot log directory for token usage metrics.
/// Emits an `AgentUpdate` if any tokens are found. Safe to call when
/// `session_log_dir` is `None` (no-op).
async fn emit_log_token_usage(
    session_log_dir: &Option<std::path::PathBuf>,
    update_tx: &mpsc::Sender<AgentUpdate>,
    issue_id: &str,
    session_id: &str,
) {
    if let Some(ref log_dir) = session_log_dir {
        let usage = log_parser::scan_log_dir(log_dir).await;
        if usage.total_tokens > 0 || usage.prompt_tokens > 0 {
            info!(
                prompt = usage.prompt_tokens,
                completion = usage.completion_tokens,
                total = usage.total_tokens,
                "token usage from log files"
            );
            emit_update(
                update_tx,
                AgentUpdate {
                    issue_id: issue_id.to_string(),
                    event: "token_usage".to_string(),
                    message: Some("from log files".to_string()),
                    input_tokens: Some(usage.prompt_tokens),
                    output_tokens: Some(usage.completion_tokens),
                    total_tokens: Some(usage.total_tokens),
                    session_id: Some(session_id.to_string()),
                },
            )
            .await;
        } else {
            info!(log_dir = %log_dir.display(), "no token usage found in log files");
        }
    }
}

/// Try to load a previous session; fall back to creating a new one.
///
/// If `previous_session_id` is `Some`, sends `session/load` first.
/// On load failure, logs a warning and creates a fresh session via `session/new`.
async fn try_load_or_create_session(
    client: &mut AcpClient,
    cwd: &std::path::Path,
    config: &RustyConfig,
    previous_session_id: Option<&str>,
) -> Result<String, AgentError> {
    if let Some(prev_id) = previous_session_id {
        info!(%prev_id, "attempting to resume previous ACP session");
        match client
            .load_session(prev_id, None, config.agent.read_timeout_ms)
            .await
        {
            Ok(session_id) => {
                info!(%session_id, "ACP session resumed successfully");
                return Ok(session_id);
            }
            Err(e) => {
                warn!(
                    error = %e,
                    prev_id,
                    "session/load failed, falling back to session/new"
                );
            }
        }
    }

    client
        .create_session(
            cwd,
            &config.agent.approval_policy,
            None,
            config.agent.read_timeout_ms,
        )
        .await
}
