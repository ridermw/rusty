pub mod state;

use chrono::Utc;

use crate::config;
use crate::config::schema::RustyConfig;
use crate::tracker::Issue;
use state::{OrchestratorState, TokenTotals};
use tokio::sync::oneshot;

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
        .active_states
        .iter()
        .map(|value| value.to_lowercase())
        .collect();
    if !active.contains(&state_lower) {
        return false;
    }

    let terminal: Vec<String> = config
        .tracker
        .terminal_states
        .iter()
        .map(|value| value.to_lowercase())
        .collect();
    if terminal.contains(&state_lower) {
        return false;
    }

    if state.running.contains_key(&issue.id) || state.claimed.contains(&issue.id) {
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
