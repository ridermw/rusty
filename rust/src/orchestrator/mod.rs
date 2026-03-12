pub mod state;

use crate::config;
use crate::config::schema::SymphonyConfig;
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
pub fn is_eligible(issue: &Issue, state: &OrchestratorState, config: &SymphonyConfig) -> bool {
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
