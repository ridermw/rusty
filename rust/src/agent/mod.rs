pub mod acp_client;
pub mod dynamic_tool;

pub use acp_client::{AcpClient, AgentError, AgentEvent, ChildGuard, JsonRpcMessage, TurnResult};

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

        // ACP session integration will be wired in a follow-up change. For now, the
        // runner exercises the workspace, hook, and prompt lifecycle in one attempt.
        run_after_run_hook(&shell_executor, &config.hooks, &ws_path);

        info!(turns_max = config.agent.max_turns, "agent run completed");
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
