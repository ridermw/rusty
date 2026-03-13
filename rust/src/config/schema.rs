use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RustyConfig {
    pub tracker: TrackerConfig,
    pub polling: PollingConfig,
    pub workspace: WorkspaceConfig,
    pub hooks: HooksConfig,
    pub agent: AgentConfig,
    pub server: ServerConfig,
    pub copilot: CopilotConfig,
    pub github: GitHubCliConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TrackerConfig {
    pub kind: Option<String>,
    pub endpoint: Option<String>,
    pub api_key: Option<String>,
    /// Repository owner (e.g., "ridermw"). Can be set separately or combined in `repo`.
    pub owner: Option<String>,
    /// Repository name or "owner/repo" combined format.
    pub repo: Option<String>,
    pub active_states: Vec<String>,
    pub terminal_states: Vec<String>,
    pub labels: Vec<String>,
    /// Labels that map to active issue states (e.g., ["todo", "in_progress"]).
    /// Used when `active_states` needs label-based matching.
    pub active_issue_labels: Vec<String>,
    /// Labels that map to terminal issue states (e.g., ["done", "closed"]).
    pub terminal_issue_labels: Vec<String>,
    pub state_labels: HashMap<String, String>,
    pub assignee: Option<String>,
}

impl TrackerConfig {
    /// Get the full "owner/repo" string, combining separate fields if needed.
    pub fn full_repo(&self) -> Option<String> {
        match (&self.owner, &self.repo) {
            (Some(owner), Some(repo)) if !repo.contains('/') => Some(format!("{}/{}", owner, repo)),
            (_, Some(repo)) if repo.contains('/') => Some(repo.clone()),
            _ => None,
        }
    }

    /// Get effective active states: combines active_states with active_issue_labels.
    pub fn effective_active_states(&self) -> Vec<String> {
        let mut states: Vec<String> = self.active_states.clone();
        for label in &self.active_issue_labels {
            let lower = label.to_lowercase();
            if !states.iter().any(|s| s.to_lowercase() == lower) {
                states.push(label.clone());
            }
        }
        states
    }

    /// Get effective terminal states: combines terminal_states with terminal_issue_labels.
    pub fn effective_terminal_states(&self) -> Vec<String> {
        let mut states: Vec<String> = self.terminal_states.clone();
        for label in &self.terminal_issue_labels {
            let lower = label.to_lowercase();
            if !states.iter().any(|s| s.to_lowercase() == lower) {
                states.push(label.clone());
            }
        }
        states
    }
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            kind: None,
            endpoint: None,
            api_key: None,
            owner: None,
            repo: None,
            active_states: vec!["open".to_string()],
            terminal_states: vec!["closed".to_string()],
            labels: Vec::new(),
            active_issue_labels: Vec::new(),
            terminal_issue_labels: Vec::new(),
            state_labels: HashMap::new(),
            assignee: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PollingConfig {
    pub interval_ms: u64,
}

impl Default for PollingConfig {
    fn default() -> Self {
        Self {
            interval_ms: 30_000,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WorkspaceConfig {
    pub root: Option<String>,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            root: Some(default_workspace_root()),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct HooksConfig {
    pub after_create: Option<String>,
    pub before_run: Option<String>,
    pub after_run: Option<String>,
    pub before_remove: Option<String>,
    pub timeout_ms: u64,
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self {
            after_create: None,
            before_run: None,
            after_run: None,
            before_remove: None,
            timeout_ms: 60_000,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub max_concurrent_agents: usize,
    pub max_turns: usize,
    pub max_retry_backoff_ms: u64,
    pub max_concurrent_agents_by_state: HashMap<String, usize>,
    pub command: String,
    pub turn_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub stall_timeout_ms: u64,
    pub approval_policy: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_concurrent_agents: 10,
            max_turns: 20,
            max_retry_backoff_ms: 300_000,
            max_concurrent_agents_by_state: HashMap::new(),
            command: "copilot --acp --yolo --no-ask-user".to_string(),
            turn_timeout_ms: 3_600_000,
            read_timeout_ms: 30_000,
            stall_timeout_ms: 300_000,
            approval_policy: "auto-approve".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub port: Option<u16>,
    pub host: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: None,
            host: Some("127.0.0.1".to_string()),
        }
    }
}

/// Copilot CLI configuration (maps to copilot.* in WORKFLOW.md)
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CopilotConfig {
    pub command: String,
    pub chat_command: Option<String>,
    pub approval_policy: String,
    pub thread_sandbox: Option<String>,
    pub turn_sandbox_policy: Option<serde_yaml::Value>,
}

impl Default for CopilotConfig {
    fn default() -> Self {
        Self {
            command: "copilot".to_string(),
            chat_command: None,
            approval_policy: "never".to_string(),
            thread_sandbox: None,
            turn_sandbox_policy: None,
        }
    }
}

/// GitHub CLI configuration (maps to github.* in WORKFLOW.md)
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GitHubCliConfig {
    pub cli_command: String,
    pub default_branch: String,
    pub required_pr_label: Option<String>,
}

impl Default for GitHubCliConfig {
    fn default() -> Self {
        Self {
            cli_command: "gh".to_string(),
            default_branch: "main".to_string(),
            required_pr_label: None,
        }
    }
}

fn default_workspace_root() -> String {
    std::env::temp_dir()
        .join("rusty_workspaces")
        .to_string_lossy()
        .into_owned()
}
