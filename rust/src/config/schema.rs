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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TrackerConfig {
    pub kind: Option<String>,
    pub endpoint: Option<String>,
    pub api_key: Option<String>,
    pub repo: Option<String>,
    pub active_states: Vec<String>,
    pub terminal_states: Vec<String>,
    pub labels: Vec<String>,
    pub state_labels: HashMap<String, String>,
    pub assignee: Option<String>,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            kind: None,
            endpoint: None,
            api_key: None,
            repo: None,
            active_states: vec!["open".to_string()],
            terminal_states: vec!["closed".to_string()],
            labels: Vec::new(),
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
            command: "copilot --acp --stdio".to_string(),
            turn_timeout_ms: 3_600_000,
            read_timeout_ms: 5_000,
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

fn default_workspace_root() -> String {
    std::env::temp_dir()
        .join("rusty_workspaces")
        .to_string_lossy()
        .into_owned()
}
