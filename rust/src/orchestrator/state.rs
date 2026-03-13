use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::tracker::Issue;

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TokenTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub seconds_running: f64,
}

#[derive(Debug, Clone)]
pub struct RunningEntry {
    pub issue_id: String,
    pub identifier: String,
    pub issue: Issue,
    pub session_id: Option<String>,
    pub last_event: Option<String>,
    pub last_event_at: Option<DateTime<Utc>>,
    pub last_message: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub last_reported_input: u64,
    pub last_reported_output: u64,
    pub last_reported_total: u64,
    pub turn_count: u32,
    pub retry_attempt: Option<u32>,
    pub started_at: DateTime<Utc>,
    pub worker_handle: tokio::task::AbortHandle,
}

#[derive(Debug, Clone)]
pub struct RetryEntry {
    pub issue_id: String,
    pub identifier: String,
    pub attempt: u32,
    pub due_at: DateTime<Utc>,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct OrchestratorState {
    pub poll_interval_ms: u64,
    pub max_concurrent_agents: usize,
    pub running: HashMap<String, RunningEntry>,
    pub claimed: HashSet<String>,
    pub retry_attempts: HashMap<String, RetryEntry>,
    pub completed: HashSet<String>,
    /// Track consecutive normal completions per issue (for throttling).
    pub completed_counts: HashMap<String, u32>,
    pub agent_totals: TokenTotals,
    pub agent_rate_limits: Option<serde_json::Value>,
}

impl OrchestratorState {
    pub fn new(poll_interval_ms: u64, max_concurrent_agents: usize) -> Self {
        Self {
            poll_interval_ms,
            max_concurrent_agents,
            running: HashMap::new(),
            claimed: HashSet::new(),
            retry_attempts: HashMap::new(),
            completed: HashSet::new(),
            completed_counts: HashMap::new(),
            agent_totals: TokenTotals::default(),
            agent_rate_limits: None,
        }
    }

    pub fn running_count(&self) -> usize {
        self.running.len()
    }

    pub fn available_global_slots(&self) -> usize {
        self.max_concurrent_agents
            .saturating_sub(self.running_count())
    }

    pub fn running_count_by_state(&self, state: &str) -> usize {
        self.running
            .values()
            .filter(|entry| entry.issue.state.eq_ignore_ascii_case(state))
            .count()
    }
}
