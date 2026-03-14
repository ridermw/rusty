pub mod state;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use chrono::Utc;

use crate::config;
use crate::config::schema::RustyConfig;
use crate::tracker::Issue;
use crate::workspace::hooks::{HookKind, ShellExecutor};
use state::{OrchestratorState, RetryEntry, RunningEntry, TokenTotals};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;

/// Messages the orchestrator receives.
#[derive(Debug)]
pub enum OrchestratorMsg {
    Tick,
    WorkerExited {
        issue_id: String,
        success: bool,
        error: Option<String>,
    },
    AgentUpdate {
        issue_id: String,
        event: String,
        message: Option<String>,
    },
    RetryFired {
        issue_id: String,
    },
    SnapshotRequest {
        reply: oneshot::Sender<OrchestratorSnapshot>,
    },
    RefreshRequest {
        reply: oneshot::Sender<()>,
    },
    Shutdown,
}

/// Snapshot of orchestrator state for API/dashboard.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OrchestratorSnapshot {
    pub running_count: usize,
    pub retrying_count: usize,
    pub running: Vec<RunningSnapshot>,
    pub retrying: Vec<RetrySnapshot>,
    pub agent_totals: TokenTotals,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RunningSnapshot {
    pub issue_id: String,
    pub identifier: String,
    pub state: String,
    pub session_id: Option<String>,
    pub turn_count: u32,
    pub last_event: Option<String>,
    pub last_message: Option<String>,
    pub started_at: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RetrySnapshot {
    pub issue_id: String,
    pub identifier: String,
    pub attempt: u32,
    pub due_at: String,
    pub error: Option<String>,
}

/// Check if an issue is eligible for dispatch.
pub fn is_eligible(issue: &Issue, state: &OrchestratorState, config: &RustyConfig) -> bool {
    if issue.id.is_empty()
        || issue.identifier.is_empty()
        || issue.title.is_empty()
        || issue.state.is_empty()
    {
        return false;
    }

    let state_lower = issue.state.to_lowercase();
    let active: Vec<String> = config
        .tracker
        .effective_active_states()
        .iter()
        .map(|value| value.to_lowercase())
        .collect();
    if !active.contains(&state_lower) {
        return false;
    }

    let terminal: Vec<String> = config
        .tracker
        .effective_terminal_states()
        .iter()
        .map(|value| value.to_lowercase())
        .collect();
    if terminal.contains(&state_lower) {
        return false;
    }

    if state.running.contains_key(&issue.id) || state.claimed.contains(&issue.id) {
        return false;
    }

    if state.completed.contains(&issue.id) {
        return false;
    }

    if state.available_global_slots() == 0 {
        return false;
    }

    let normalized_map =
        config::normalize_state_concurrency(&config.agent.max_concurrent_agents_by_state);
    if let Some(&max) = normalized_map.get(&state_lower) {
        if state.running_count_by_state(&state_lower) >= max {
            return false;
        }
    }

    if state_lower == "todo" {
        for blocker in &issue.blocked_by {
            if let Some(blocker_state) = &blocker.state {
                if !terminal.contains(&blocker_state.to_lowercase()) {
                    return false;
                }
            } else {
                return false;
            }
        }
    }

    true
}

/// Sort issues for dispatch: priority asc (null last), created_at asc, identifier asc.
pub fn sort_for_dispatch(issues: &mut [Issue]) {
    issues.sort_by(|a, b| {
        let a_priority = a.priority.unwrap_or(i32::MAX);
        let b_priority = b.priority.unwrap_or(i32::MAX);

        a_priority
            .cmp(&b_priority)
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.identifier.cmp(&b.identifier))
    });
}

/// Apply a token usage update to a running entry and aggregate totals.
/// Uses absolute totals with delta tracking to avoid double-counting.
pub fn apply_token_update(
    entry: &mut state::RunningEntry,
    totals: &mut state::TokenTotals,
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
) {
    let input_delta = input_tokens.saturating_sub(entry.last_reported_input);
    let output_delta = output_tokens.saturating_sub(entry.last_reported_output);
    let total_delta = total_tokens.saturating_sub(entry.last_reported_total);

    entry.input_tokens = input_tokens;
    entry.output_tokens = output_tokens;
    entry.total_tokens = total_tokens;

    entry.last_reported_input = input_tokens;
    entry.last_reported_output = output_tokens;
    entry.last_reported_total = total_tokens;

    totals.input_tokens += input_delta;
    totals.output_tokens += output_delta;
    totals.total_tokens += total_delta;
}

/// Add runtime seconds to totals when a session ends.
pub fn add_runtime_seconds(totals: &mut state::TokenTotals, entry: &state::RunningEntry) {
    let elapsed = chrono::Utc::now()
        .signed_duration_since(entry.started_at)
        .num_milliseconds() as f64
        / 1000.0;
    totals.seconds_running += elapsed;
}

/// Compose a session ID from thread and turn IDs.
pub fn compose_session_id(thread_id: &str, turn_id: &str) -> String {
    format!("{}-{}", thread_id, turn_id)
}

/// Check running entries for stalls. Returns list of issue_ids that are stalled.
pub fn detect_stalled(state: &OrchestratorState, stall_timeout_ms: u64) -> Vec<String> {
    if stall_timeout_ms == 0 {
        return vec![];
    }

    let now = Utc::now();
    let timeout = chrono::Duration::milliseconds(stall_timeout_ms as i64);

    state
        .running
        .values()
        .filter(|entry| {
            let last_activity = entry.last_event_at.unwrap_or(entry.started_at);
            now.signed_duration_since(last_activity) > timeout
        })
        .map(|entry| entry.issue_id.clone())
        .collect()
}

/// Reconcile running issues against fresh tracker state.
/// Returns actions to take: stop (with/without cleanup), update.
#[derive(Debug)]
pub enum ReconcileAction {
    StopAndCleanup(String),
    StopNoCleanup(String),
    UpdateState(String, Box<Issue>),
}

pub fn reconcile_against_tracker(
    running_ids: &[String],
    refreshed: &[Issue],
    terminal_states: &[String],
    active_states: &[String],
) -> Vec<ReconcileAction> {
    let terminal: Vec<String> = terminal_states
        .iter()
        .map(|state| state.to_lowercase())
        .collect();
    let active: Vec<String> = active_states
        .iter()
        .map(|state| state.to_lowercase())
        .collect();

    refreshed
        .iter()
        .filter_map(|issue| {
            if !running_ids.contains(&issue.id) {
                return None;
            }

            let state_lower = issue.state.to_lowercase();
            if terminal.contains(&state_lower) {
                Some(ReconcileAction::StopAndCleanup(issue.id.clone()))
            } else if active.contains(&state_lower) {
                Some(ReconcileAction::UpdateState(
                    issue.id.clone(),
                    Box::new(issue.clone()),
                ))
            } else {
                Some(ReconcileAction::StopNoCleanup(issue.id.clone()))
            }
        })
        .collect()
}

/// After this many consecutive normal completions for the same issue
/// without a state change, switch from 1s continuation retry to
/// exponential backoff to avoid burning API calls on no-op workers.
const MAX_CONTINUATION_RETRIES: u32 = 3;

/// After this many consecutive failure retries, stop scheduling
/// further retries and set the project status to HumanReview.
pub const MAX_FAILURE_RETRIES: u32 = 20;

/// Check if a failure retry attempt has exceeded the maximum allowed retries.
pub fn should_stop_retrying(attempt: u32) -> bool {
    attempt > MAX_FAILURE_RETRIES
}

/// Calculate backoff delay in milliseconds for a retry attempt.
pub fn calculate_backoff(attempt: u32, max_backoff_ms: u64, is_continuation: bool) -> u64 {
    if is_continuation {
        return 1000;
    }

    let base: u64 = 10_000;
    let exp = attempt.saturating_sub(1);
    let delay = base.saturating_mul(2u64.saturating_pow(exp));
    delay.min(max_backoff_ms)
}

/// Check if a continuation retry should be throttled.
/// Returns true if the issue has had too many consecutive no-op completions.
pub fn should_throttle_continuation(consecutive_completions: u32) -> bool {
    consecutive_completions > MAX_CONTINUATION_RETRIES
}

/// Check if a retry attempt should trigger a warning log.
pub fn should_warn_retry(attempt: u32) -> bool {
    matches!(attempt, 5 | 10 | 20)
}

/// Determine the next attempt number.
/// For normal exit: always attempt 1 (continuation).
/// For failure: increment from the running entry's retry_attempt.
pub fn next_attempt(current: Option<u32>, is_normal_exit: bool) -> u32 {
    if is_normal_exit {
        1
    } else {
        current.map_or(1, |a| a + 1)
    }
}

/// Build a snapshot from current state.
pub fn build_snapshot(state: &OrchestratorState) -> OrchestratorSnapshot {
    let running: Vec<RunningSnapshot> = state
        .running
        .values()
        .map(|entry| RunningSnapshot {
            issue_id: entry.issue_id.clone(),
            identifier: entry.identifier.clone(),
            state: entry.issue.state.clone(),
            session_id: entry.session_id.clone(),
            turn_count: entry.turn_count,
            last_event: entry.last_event.clone(),
            last_message: entry.last_message.clone(),
            started_at: entry.started_at.to_rfc3339(),
            input_tokens: entry.input_tokens,
            output_tokens: entry.output_tokens,
            total_tokens: entry.total_tokens,
        })
        .collect();

    let retrying: Vec<RetrySnapshot> = state
        .retry_attempts
        .values()
        .map(|entry| RetrySnapshot {
            issue_id: entry.issue_id.clone(),
            identifier: entry.identifier.clone(),
            attempt: entry.attempt,
            due_at: entry.due_at.to_rfc3339(),
            error: entry.error.clone(),
        })
        .collect();

    OrchestratorSnapshot {
        running_count: running.len(),
        retrying_count: retrying.len(),
        running,
        retrying,
        agent_totals: state.agent_totals.clone(),
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_agent_update_to_state(
    state: &mut OrchestratorState,
    issue_id: &str,
    event: String,
    message: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
    session_id: Option<String>,
    workspace_root: &Path,
) {
    if let Some(entry) = state.running.get_mut(issue_id) {
        entry.last_event = Some(event.clone());
        entry.last_event_at = Some(Utc::now());
        entry.last_message = message;

        if let Some(ref session_id) = session_id {
            entry.session_id = Some(session_id.clone());
        }

        // Persist session ID to file store on session start
        if event == "session_started" {
            if let Some(ref sid) = session_id {
                let store = crate::session::SessionStore::new(workspace_root);
                if let Err(e) = store.save(crate::session::SessionRecord {
                    issue_id: issue_id.to_string(),
                    session_id: sid.clone(),
                    created_at: Utc::now(),
                    workspace_path: None,
                }) {
                    tracing::warn!(%issue_id, error = %e, "failed to persist session to file store");
                }
            }
        }

        // Clean up session record on completion or failure
        if event == "completed" || event == "failed" {
            let store = crate::session::SessionStore::new(workspace_root);
            if let Err(e) = store.delete(issue_id) {
                tracing::warn!(%issue_id, error = %e, "failed to delete session from file store");
            }
        }

        // Increment turn count on turn completion events
        if event == "turn_completed" || event == "completed" {
            entry.turn_count += 1;
        }

        if let (Some(input_tokens), Some(output_tokens), Some(total_tokens)) =
            (input_tokens, output_tokens, total_tokens)
        {
            apply_token_update(
                entry,
                &mut state.agent_totals,
                input_tokens,
                output_tokens,
                total_tokens,
            );
        }
    }
}

/// Spawn a fire-and-forget task that updates the GitHub Project status for an issue.
/// No-op when `project_number` is not configured.
fn spawn_project_status_update(config: &RustyConfig, issue_id: &str, status: &str) {
    if config.tracker.project_number.unwrap_or(0) > 0 {
        let owner = config.tracker.owner.clone().unwrap_or_default();
        let proj_num = config.tracker.project_number.unwrap_or(0);
        let issue_num = issue_id.to_string();
        let status = status.to_string();
        tokio::spawn(async move {
            update_project_status(&owner, proj_num, &issue_num, &status).await;
        });
    }
}

/// Update a GitHub Project item's status field.
/// Shells out to `gh project item-list` to find the item, then `gh project item-edit` to update.
async fn update_project_status(
    owner: &str,
    project_number: u32,
    issue_number: &str,
    status_name: &str,
) {
    // Status option IDs (from project field config)
    let option_id = match status_name {
        "Backlog" => "f75ad846",
        "Todo" => "61e4505c",
        "InProgress" => "47fc9ee4",
        "HumanReview" => "df73e18b",
        "Merging" => "37ec4a6e",
        "Rework" => "84ea7dca",
        "Done" => "98236657",
        _ => {
            tracing::warn!(status_name, "unknown project status, skipping update");
            return;
        }
    };

    // Find the project item ID for this issue
    let list_output = tokio::process::Command::new("gh")
        .args([
            "project",
            "item-list",
            &project_number.to_string(),
            "--owner",
            owner,
            "--format",
            "json",
            "--limit",
            "200",
        ])
        .output()
        .await;

    let item_id = match list_output {
        Ok(output) if output.status.success() => {
            let json: serde_json::Value = match serde_json::from_slice(&output.stdout) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to parse project items");
                    return;
                }
            };
            json.get("items")
                .and_then(|items| items.as_array())
                .and_then(|items| {
                    items.iter().find(|item| {
                        item.get("content")
                            .and_then(|c| c.get("number"))
                            .and_then(|n| n.as_u64())
                            .map(|n| n.to_string())
                            == Some(issue_number.to_string())
                    })
                })
                .and_then(|item| item.get("id").and_then(|id| id.as_str()))
                .map(|s| s.to_string())
        }
        _ => {
            tracing::warn!(
                issue_number,
                "failed to list project items for status update"
            );
            return;
        }
    };

    let item_id = match item_id {
        Some(id) => id,
        None => {
            tracing::debug!(
                issue_number,
                "issue not found in project, skipping status update"
            );
            return;
        }
    };

    // Update the status
    let project_id = "PVT_kwHOAFl5zs4BRqBN"; // TODO: read from config
    let field_id = "PVTSSF_lAHOAFl5zs4BRqBNzg_a8jM"; // TODO: read from config

    let result = tokio::process::Command::new("gh")
        .args([
            "project",
            "item-edit",
            "--project-id",
            project_id,
            "--id",
            &item_id,
            "--field-id",
            field_id,
            "--single-select-option-id",
            option_id,
        ])
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            tracing::info!(issue_number, status_name, "project status updated");
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(issue_number, status_name, %stderr, "project status update failed");
        }
        Err(e) => {
            tracing::warn!(issue_number, status_name, error = %e, "project status update failed");
        }
    }
}

fn cleanup_workspace_for_issue(
    config: &RustyConfig,
    workspace_root: &Path,
    shell_executor: &dyn ShellExecutor,
    identifier: &str,
) {
    let ws_path = crate::workspace::workspace_path(workspace_root, identifier);
    if !ws_path.exists() {
        return;
    }

    let timeout = std::time::Duration::from_millis(config.hooks.timeout_ms);
    if let Err(error) = crate::workspace::hooks::run_hook(
        shell_executor,
        HookKind::BeforeRemove,
        config.hooks.before_remove.as_deref(),
        &ws_path,
        timeout,
    ) {
        tracing::warn!(%identifier, %error, "before_remove hook failed during cleanup");
    }

    if let Err(error) = crate::workspace::remove_workspace(workspace_root, identifier) {
        tracing::warn!(%identifier, %error, "failed to remove workspace during cleanup");
    }
}

fn schedule_retry(msg_tx: &mpsc::Sender<OrchestratorMsg>, issue_id: String, delay_ms: u64) {
    let retry_tx = msg_tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        let _ = retry_tx
            .send(OrchestratorMsg::RetryFired { issue_id })
            .await;
    });
}

/// Run the orchestrator main loop. This is the heart of Rusty.
pub async fn run_orchestrator(
    mut state: OrchestratorState,
    config: RustyConfig,
    tracker: Arc<dyn crate::tracker::Tracker>,
    workflow_prompt: String,
    workspace_root: std::path::PathBuf,
    shell_executor: Arc<dyn ShellExecutor>,
    mut msg_rx: mpsc::Receiver<OrchestratorMsg>,
    msg_tx: mpsc::Sender<OrchestratorMsg>,
) {
    let mut workers: JoinSet<(String, bool, Option<String>)> = JoinSet::new();
    let (agent_update_tx, mut agent_update_rx) = mpsc::channel::<crate::agent::AgentUpdate>(256);
    let mut tick_interval =
        tokio::time::interval(tokio::time::Duration::from_millis(state.poll_interval_ms));
    let active_states = config.tracker.effective_active_states();
    let terminal_states = config.tracker.effective_terminal_states();

    tracing::info!(
        poll_ms = state.poll_interval_ms,
        max_agents = state.max_concurrent_agents,
        "orchestrator loop started"
    );

    loop {
        tokio::select! {
            _ = tick_interval.tick() => {
                tracing::debug!("tick: starting poll cycle");

                let stalled = detect_stalled(&state, config.agent.stall_timeout_ms);
                for issue_id in stalled {
                    tracing::warn!(%issue_id, "stalled session detected, aborting worker");
                    if let Some(entry) = state.running.remove(&issue_id) {
                        entry.worker_handle.abort();
                        add_runtime_seconds(&mut state.agent_totals, &entry);
                        state.claimed.remove(&issue_id);
                        state.completed_counts.remove(&issue_id);
                    }
                }

                let running_ids: Vec<String> = state.running.keys().cloned().collect();
                if !running_ids.is_empty() {
                    match tracker.fetch_issue_states_by_ids(&running_ids).await {
                        Ok(refreshed) => {
                            for action in reconcile_against_tracker(
                                &running_ids,
                                &refreshed,
                                &terminal_states,
                                &active_states,
                            ) {
                                match action {
                                    ReconcileAction::StopAndCleanup(issue_id) => {
                                        tracing::info!(%issue_id, "tracker marked running issue terminal; stopping worker");
                                        if let Some(entry) = state.running.remove(&issue_id) {
                                            entry.worker_handle.abort();
                                            add_runtime_seconds(&mut state.agent_totals, &entry);
                                            cleanup_workspace_for_issue(
                                                &config,
                                                &workspace_root,
                                                shell_executor.as_ref(),
                                                &entry.identifier,
                                            );
                                            state.claimed.remove(&issue_id);
                                            state.retry_attempts.remove(&issue_id);
                                            state.completed_counts.remove(&issue_id);

                                            // Terminal state (e.g. issue closed by merged PR) → Done
                                            spawn_project_status_update(&config, &issue_id, "Done");
                                        }
                                    }
                                    ReconcileAction::StopNoCleanup(issue_id) => {
                                        tracing::info!(%issue_id, "tracker marked running issue inactive; stopping worker");
                                        if let Some(entry) = state.running.remove(&issue_id) {
                                            entry.worker_handle.abort();
                                            add_runtime_seconds(&mut state.agent_totals, &entry);
                                            state.claimed.remove(&issue_id);
                                            state.retry_attempts.remove(&issue_id);
                                            state.completed_counts.remove(&issue_id);

                                            // Non-terminal inactive state → needs human attention
                                            spawn_project_status_update(&config, &issue_id, "HumanReview");
                                        }
                                    }
                                    ReconcileAction::UpdateState(issue_id, issue) => {
                                        if let Some(entry) = state.running.get_mut(&issue_id) {
                                            // When project tracking is enabled, REST returns
                                            // "open"/"closed" (not project status like "InProgress").
                                            // Preserve the project-derived state from dispatch;
                                            // only update non-state fields (title, labels, etc.).
                                            let preserve_state = config.tracker.project_number.unwrap_or(0) > 0;
                                            let preserved = entry.issue.state.clone();

                                            if !preserve_state && entry.issue.state != issue.state {
                                                state.completed_counts.remove(&issue_id);
                                            }
                                            entry.issue = *issue;
                                            if preserve_state {
                                                entry.issue.state = preserved;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            tracing::error!(error = %error, "failed to reconcile running issues");
                        }
                    }
                }

                match tracker.fetch_candidate_issues(&config.tracker).await {
                    Ok(mut candidates) => {
                        tracing::info!(count = candidates.len(), "fetched candidate issues");

                        let candidate_ids: HashSet<String> = candidates.iter().map(|issue| issue.id.clone()).collect();
                        state.retry_attempts.retain(|issue_id, _| {
                            state.claimed.contains(issue_id) || candidate_ids.contains(issue_id)
                        });
                        state.completed_counts.retain(|issue_id, _| {
                            state.running.contains_key(issue_id)
                                || state.claimed.contains(issue_id)
                                || candidate_ids.contains(issue_id)
                        });

                        sort_for_dispatch(&mut candidates);

                        for issue in candidates {
                            if state.available_global_slots() == 0 {
                                break;
                            }
                            if !is_eligible(&issue, &state, &config) {
                                continue;
                            }

                            let issue_id = issue.id.clone();
                            let identifier = issue.identifier.clone();
                            let retry_attempt = state.retry_attempts.remove(&issue_id).map(|entry| entry.attempt);
                            state.claimed.insert(issue_id.clone());

                            tracing::info!(%issue_id, %identifier, attempt = ?retry_attempt, "dispatching issue");

                            // Update project status to InProgress on dispatch
                            spawn_project_status_update(&config, &issue_id, "InProgress");

                            let dispatch_config = config.clone();
                            let dispatch_prompt = workflow_prompt.clone();
                            let dispatch_root = workspace_root.clone();
                            let dispatch_executor = shell_executor.clone();
                            let dispatch_issue = issue.clone();
                            let dispatch_updates = agent_update_tx.clone();

                            // Look up any saved session ID from a previous run
                            let session_store = crate::session::SessionStore::new(&workspace_root);
                            let previous_session_id = session_store
                                .load(&issue_id)
                                .map(|r| r.session_id);

                            let abort_handle = workers.spawn(async move {
                                let result = crate::agent::run_agent_attempt(
                                    dispatch_issue.clone(),
                                    retry_attempt,
                                    dispatch_config,
                                    dispatch_prompt,
                                    dispatch_root,
                                    dispatch_executor,
                                    dispatch_updates,
                                    previous_session_id,
                                )
                                .await;

                                let (success, error) = match result {
                                    crate::agent::WorkerResult::Completed => (true, None),
                                    crate::agent::WorkerResult::Failed(error) => (false, Some(error)),
                                };

                                (dispatch_issue.id, success, error)
                            });

                            state.running.insert(issue_id.clone(), RunningEntry {
                                issue_id: issue_id.clone(),
                                identifier: identifier.clone(),
                                issue,
                                session_id: None,
                                last_event: None,
                                last_event_at: None,
                                last_message: None,
                                input_tokens: 0,
                                output_tokens: 0,
                                total_tokens: 0,
                                last_reported_input: 0,
                                last_reported_output: 0,
                                last_reported_total: 0,
                                turn_count: 0,
                                retry_attempt,
                                started_at: chrono::Utc::now(),
                                worker_handle: abort_handle,
                            });
                        }
                    }
                    Err(error) => {
                        tracing::error!(error = %error, "failed to fetch candidates, skipping tick");
                    }
                }
            }

            Some(update) = agent_update_rx.recv() => {
                apply_agent_update_to_state(
                    &mut state,
                    &update.issue_id,
                    update.event,
                    update.message,
                    update.input_tokens,
                    update.output_tokens,
                    update.total_tokens,
                    update.session_id,
                    &workspace_root,
                );
            }

            Some(result) = workers.join_next() => {
                match result {
                    Ok((issue_id, success, error)) => {
                        tracing::info!(%issue_id, %success, ?error, "worker exited");
                        if let Some(entry) = state.running.remove(&issue_id) {
                            let retry_attempt = next_attempt(entry.retry_attempt, success);

                            add_runtime_seconds(&mut state.agent_totals, &entry);
                            let delay_ms = if success {
                                state.completed.insert(issue_id.clone());

                                // Agent completed successfully — mark for human review
                                spawn_project_status_update(&config, &issue_id, "HumanReview");

                                let consecutive = state
                                    .completed_counts
                                    .entry(issue_id.clone())
                                    .and_modify(|count| *count += 1)
                                    .or_insert(1);

                                if should_throttle_continuation(*consecutive) {
                                    let delay = calculate_backoff(
                                        *consecutive,
                                        config.agent.max_retry_backoff_ms,
                                        false,
                                    );
                                    tracing::warn!(
                                        %issue_id,
                                        consecutive = *consecutive,
                                        delay_ms = delay,
                                        "throttling continuation retry — issue may not need agent work"
                                    );
                                    delay
                                } else {
                                    let delay = calculate_backoff(
                                        1,
                                        config.agent.max_retry_backoff_ms,
                                        true,
                                    );
                                    tracing::debug!(%issue_id, delay_ms = delay, "scheduling continuation retry");
                                    delay
                                }
                            } else {
                                state.completed_counts.remove(&issue_id);

                                if should_stop_retrying(retry_attempt) {
                                    // Exhausted retries — escalate for human review
                                    tracing::warn!(
                                        %issue_id,
                                        attempt = retry_attempt,
                                        ?error,
                                        "worker exceeded max failure retries; setting HumanReview"
                                    );
                                    spawn_project_status_update(&config, &issue_id, "HumanReview");
                                    continue;
                                }

                                let delay = calculate_backoff(
                                    retry_attempt,
                                    config.agent.max_retry_backoff_ms,
                                    false,
                                );
                                if should_warn_retry(retry_attempt) {
                                    tracing::warn!(%issue_id, attempt = retry_attempt, ?error, "worker failed; scheduling retry");
                                }
                                delay
                            };

                            state.retry_attempts.insert(
                                issue_id.clone(),
                                RetryEntry {
                                    issue_id: issue_id.clone(),
                                    identifier: entry.identifier.clone(),
                                    attempt: retry_attempt,
                                    due_at: chrono::Utc::now()
                                        + chrono::Duration::milliseconds(delay_ms as i64),
                                    error: error.clone(),
                                },
                            );
                            schedule_retry(&msg_tx, issue_id.clone(), delay_ms);
                        }
                    }
                    Err(error) if error.is_cancelled() => {
                        tracing::debug!(error = %error, "worker task cancelled");
                    }
                    Err(error) => {
                        tracing::error!(error = %error, "worker task panicked");
                    }
                }
            }

            msg = msg_rx.recv() => {
                match msg {
                    Some(OrchestratorMsg::SnapshotRequest { reply }) => {
                        let _ = reply.send(build_snapshot(&state));
                    }
                    Some(OrchestratorMsg::RefreshRequest { reply }) => {
                        let _ = reply.send(());
                        tick_interval.reset_immediately();
                    }
                    Some(OrchestratorMsg::Tick) => {
                        tick_interval.reset_immediately();
                    }
                    Some(OrchestratorMsg::RetryFired { issue_id }) => {
                        state.claimed.remove(&issue_id);
                        tick_interval.reset_immediately();
                    }
                    Some(OrchestratorMsg::AgentUpdate { issue_id, event, message }) => {
                        apply_agent_update_to_state(
                            &mut state,
                            &issue_id,
                            event,
                            message,
                            None,
                            None,
                            None,
                            None,
                            &workspace_root,
                        );
                    }
                    Some(OrchestratorMsg::WorkerExited { issue_id, success, error }) => {
                        tracing::warn!(%issue_id, %success, ?error, "received external worker exit message; ignoring in favor of JoinSet tracking");
                    }
                    Some(OrchestratorMsg::Shutdown) => {
                        tracing::info!("shutdown message received, stopping orchestrator");
                        break;
                    }
                    None => {
                        tracing::info!("orchestrator message channel closed, stopping orchestrator");
                        break;
                    }
                }
            }
        }
    }

    workers.shutdown().await;

    tracing::info!(
        total_tokens = state.agent_totals.total_tokens,
        runtime_secs = state.agent_totals.seconds_running,
        completed = state.completed.len(),
        retrying = state.retry_attempts.len(),
        "orchestrator stopped"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::Issue;

    fn dummy_issue(issue_id: &str) -> Issue {
        Issue {
            id: issue_id.to_string(),
            identifier: format!("rusty-{issue_id}"),
            title: "test".into(),
            description: None,
            priority: None,
            state: "open".into(),
            labels: vec![],
            url: None,
            blocked_by: vec![],
            branch_name: None,
            created_at: None,
            updated_at: None,
        }
    }

    // Tests need tokio runtime for AbortHandle creation
    #[tokio::test]
    async fn turn_completed_event_increments_count() {
        let mut state = OrchestratorState::new(5000, 10);
        let handle = tokio::task::spawn(async {});
        state.running.insert("42".into(), RunningEntry {
            issue_id: "42".into(),
            identifier: "rusty-42".into(),
            issue: dummy_issue("42"),
            session_id: Some("sess-1".into()),
            last_event: None,
            last_event_at: None,
            last_message: None,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            last_reported_input: 0,
            last_reported_output: 0,
            last_reported_total: 0,
            turn_count: 0,
            retry_attempt: None,
            started_at: Utc::now(),
            worker_handle: handle.abort_handle(),
        });

        apply_agent_update_to_state(
            &mut state, "42", "turn_completed".into(),
            Some("turn 1 completed".into()), None, None, None, None, std::path::Path::new("."),
        );

        let entry = state.running.get("42").unwrap();
        assert_eq!(entry.turn_count, 1);
        assert_eq!(entry.last_event.as_deref(), Some("turn_completed"));
    }

    #[tokio::test]
    async fn completed_event_increments_count() {
        let mut state = OrchestratorState::new(5000, 10);
        let handle = tokio::task::spawn(async {});
        state.running.insert("42".into(), RunningEntry {
            issue_id: "42".into(),
            identifier: "rusty-42".into(),
            issue: dummy_issue("42"),
            session_id: None,
            last_event: None,
            last_event_at: None,
            last_message: None,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            last_reported_input: 0,
            last_reported_output: 0,
            last_reported_total: 0,
            turn_count: 0,
            retry_attempt: None,
            started_at: Utc::now(),
            worker_handle: handle.abort_handle(),
        });

        apply_agent_update_to_state(
            &mut state, "42", "completed".into(), None, None, None, None, None, std::path::Path::new("."),
        );

        assert_eq!(state.running.get("42").unwrap().turn_count, 1);
    }

    #[tokio::test]
    async fn notification_event_does_not_increment_count() {
        let mut state = OrchestratorState::new(5000, 10);
        let handle = tokio::task::spawn(async {});
        state.running.insert("42".into(), RunningEntry {
            issue_id: "42".into(),
            identifier: "rusty-42".into(),
            issue: dummy_issue("42"),
            session_id: None,
            last_event: None,
            last_event_at: None,
            last_message: None,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            last_reported_input: 0,
            last_reported_output: 0,
            last_reported_total: 0,
            turn_count: 0,
            retry_attempt: None,
            started_at: Utc::now(),
            worker_handle: handle.abort_handle(),
        });

        apply_agent_update_to_state(
            &mut state, "42", "notification".into(),
            Some("session update".into()), None, None, None, None, std::path::Path::new("."),
        );

        let entry = state.running.get("42").unwrap();
        assert_eq!(entry.turn_count, 0);
        assert_eq!(entry.last_event.as_deref(), Some("notification"));
    }

    #[tokio::test]
    async fn token_usage_updates_totals() {
        let mut state = OrchestratorState::new(5000, 10);
        let handle = tokio::task::spawn(async {});
        state.running.insert("42".into(), RunningEntry {
            issue_id: "42".into(),
            identifier: "rusty-42".into(),
            issue: dummy_issue("42"),
            session_id: None,
            last_event: None,
            last_event_at: None,
            last_message: None,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            last_reported_input: 0,
            last_reported_output: 0,
            last_reported_total: 0,
            turn_count: 0,
            retry_attempt: None,
            started_at: Utc::now(),
            worker_handle: handle.abort_handle(),
        });

        apply_agent_update_to_state(
            &mut state, "42", "token_usage".into(),
            None, Some(100), Some(200), Some(300), None, std::path::Path::new("."),
        );

        let entry = state.running.get("42").unwrap();
        assert_eq!(entry.input_tokens, 100);
        assert_eq!(entry.output_tokens, 200);
        assert_eq!(entry.total_tokens, 300);
        assert_eq!(state.agent_totals.total_tokens, 300);
    }

    #[tokio::test]
    async fn session_id_updated_on_event() {
        let mut state = OrchestratorState::new(5000, 10);
        let handle = tokio::task::spawn(async {});
        state.running.insert("42".into(), RunningEntry {
            issue_id: "42".into(),
            identifier: "rusty-42".into(),
            issue: dummy_issue("42"),
            session_id: None,
            last_event: None,
            last_event_at: None,
            last_message: None,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            last_reported_input: 0,
            last_reported_output: 0,
            last_reported_total: 0,
            turn_count: 0,
            retry_attempt: None,
            started_at: Utc::now(),
            worker_handle: handle.abort_handle(),
        });

        apply_agent_update_to_state(
            &mut state, "42", "session_started".into(),
            Some("session abc".into()), None, None, None, Some("abc-123".into()), std::path::Path::new("."),
        );

        assert_eq!(state.running.get("42").unwrap().session_id.as_deref(), Some("abc-123"));
    }

    #[tokio::test]
    async fn multiple_turns_accumulate() {
        let mut state = OrchestratorState::new(5000, 10);
        let handle = tokio::task::spawn(async {});
        state.running.insert("42".into(), RunningEntry {
            issue_id: "42".into(),
            identifier: "rusty-42".into(),
            issue: dummy_issue("42"),
            session_id: None,
            last_event: None,
            last_event_at: None,
            last_message: None,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            last_reported_input: 0,
            last_reported_output: 0,
            last_reported_total: 0,
            turn_count: 0,
            retry_attempt: None,
            started_at: Utc::now(),
            worker_handle: handle.abort_handle(),
        });

        for i in 1..=5 {
            apply_agent_update_to_state(
                &mut state, "42", "turn_completed".into(),
                Some(format!("turn {i} completed")), None, None, None, None, std::path::Path::new("."),
            );
        }

        assert_eq!(state.running.get("42").unwrap().turn_count, 5);
    }

    #[test]
    fn unknown_issue_id_is_noop() {
        let mut state = OrchestratorState::new(5000, 10);
        // Should not panic
        apply_agent_update_to_state(
            &mut state, "nonexistent", "turn_completed".into(),
            None, None, None, None, None, std::path::Path::new("."),
        );
    }
}
