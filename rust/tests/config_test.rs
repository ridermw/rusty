use std::{collections::HashMap, env, path::PathBuf, sync::Mutex};

use rusty::config::schema::RustyConfig;
use rusty::config::{
    agent_launch_command, expand_home, normalize_state_concurrency, resolve_env_value,
    resolve_github_token, validate_dispatch_config, ConfigError,
};

static GITHUB_AUTH_ENV_LOCK: Mutex<()> = Mutex::new(());

struct GitHubAuthEnvGuard {
    github_token: Option<String>,
    gh_token: Option<String>,
    path: Option<String>,
}

impl GitHubAuthEnvGuard {
    fn capture() -> Self {
        Self {
            github_token: env::var("GITHUB_TOKEN").ok(),
            gh_token: env::var("GH_TOKEN").ok(),
            path: env::var("PATH").ok(),
        }
    }

    fn clear_tokens(&self) {
        env::remove_var("GITHUB_TOKEN");
        env::remove_var("GH_TOKEN");
    }
}

impl Drop for GitHubAuthEnvGuard {
    fn drop(&mut self) {
        match &self.github_token {
            Some(value) => env::set_var("GITHUB_TOKEN", value),
            None => env::remove_var("GITHUB_TOKEN"),
        }
        match &self.gh_token {
            Some(value) => env::set_var("GH_TOKEN", value),
            None => env::remove_var("GH_TOKEN"),
        }
        match &self.path {
            Some(value) => env::set_var("PATH", value),
            None => env::remove_var("PATH"),
        }
    }
}

fn valid_config() -> RustyConfig {
    let mut config = RustyConfig::default();
    config.tracker.kind = Some("github".to_string());
    config.tracker.repo = Some("owner/repo".to_string());
    config.tracker.api_key = Some("token".to_string());
    config
}

#[test]
fn config_defaults_are_correct() {
    let config = RustyConfig::default();
    let expected_workspace_root = std::env::temp_dir()
        .join("rusty_workspaces")
        .to_string_lossy()
        .into_owned();

    assert_eq!(config.tracker.kind, None);
    assert_eq!(config.tracker.endpoint, None);
    assert_eq!(config.tracker.api_key, None);
    assert_eq!(config.tracker.repo, None);
    assert_eq!(config.tracker.active_states, vec!["open".to_string()]);
    assert_eq!(config.tracker.terminal_states, vec!["closed".to_string()]);
    assert!(config.tracker.labels.is_empty());
    assert!(config.tracker.active_issue_labels.is_empty());
    assert!(config.tracker.terminal_issue_labels.is_empty());
    assert!(config.tracker.state_labels.is_empty());
    assert_eq!(config.tracker.assignee, None);

    assert_eq!(config.polling.interval_ms, 30_000);
    assert_eq!(
        config.workspace.root.as_deref(),
        Some(expected_workspace_root.as_str())
    );

    assert_eq!(config.hooks.after_create, None);
    assert_eq!(config.hooks.before_run, None);
    assert_eq!(config.hooks.after_run, None);
    assert_eq!(config.hooks.before_remove, None);
    assert_eq!(config.hooks.timeout_ms, 60_000);

    assert_eq!(config.agent.max_concurrent_agents, 10);
    assert_eq!(config.agent.max_turns, 20);
    assert_eq!(config.agent.max_retry_backoff_ms, 300_000);
    assert!(config.agent.max_concurrent_agents_by_state.is_empty());
    assert_eq!(config.agent.command, "copilot --acp --yolo --no-ask-user");
    assert_eq!(config.agent.turn_timeout_ms, 3_600_000);
    assert_eq!(config.agent.read_timeout_ms, 5_000);
    assert_eq!(config.agent.stall_timeout_ms, 300_000);
    assert_eq!(config.agent.approval_policy, "auto-approve");

    assert_eq!(config.server.port, None);
    assert_eq!(config.server.host.as_deref(), Some("127.0.0.1"));

    assert_eq!(config.copilot.command, "copilot");
    assert_eq!(config.copilot.chat_command, None);
    assert_eq!(config.copilot.approval_policy, "never");
    assert_eq!(config.copilot.thread_sandbox, None);
    assert_eq!(config.copilot.turn_sandbox_policy, None);

    assert_eq!(config.github.cli_command, "gh");
    assert_eq!(config.github.default_branch, "main");
    assert_eq!(config.github.required_pr_label, None);
}

#[test]
fn resolve_env_value_returns_env_var_value() {
    let var_name = "SYMPHONY_TEST_GITHUB_TOKEN_RESOLVE";
    env::set_var(var_name, "secret-token");

    let resolved = resolve_env_value(&format!("${var_name}")).unwrap();
    assert_eq!(resolved, "secret-token");

    env::remove_var(var_name);
}

#[test]
fn resolve_env_value_returns_error_for_missing_var() {
    let var_name = "SYMPHONY_TEST_MISSING_VAR_5E3F8A43";
    env::remove_var(var_name);

    let err = resolve_env_value(&format!("${var_name}")).unwrap_err();
    assert!(matches!(
        err,
        ConfigError::ValidationError(message)
            if message == format!("environment variable '{}' is not set or empty", var_name)
    ));
}

#[test]
fn resolve_env_value_returns_literal_when_not_prefixed() {
    assert_eq!(resolve_env_value("literal").unwrap(), "literal");
}

#[tokio::test]
async fn resolve_github_token_returns_explicit_value_directly() {
    let _lock = GITHUB_AUTH_ENV_LOCK.lock().unwrap();
    let env_guard = GitHubAuthEnvGuard::capture();
    env_guard.clear_tokens();

    let resolved = resolve_github_token(Some("literal-token")).await.unwrap();
    assert_eq!(resolved, "literal-token");
}

#[tokio::test]
async fn resolve_github_token_returns_github_token_env_value() {
    let _lock = GITHUB_AUTH_ENV_LOCK.lock().unwrap();
    let env_guard = GitHubAuthEnvGuard::capture();
    env_guard.clear_tokens();
    env::set_var("GITHUB_TOKEN", "env-token");

    let resolved = resolve_github_token(None).await.unwrap();
    assert_eq!(resolved, "env-token");
}

#[tokio::test]
async fn resolve_github_token_returns_error_when_no_source_is_available() {
    let _lock = GITHUB_AUTH_ENV_LOCK.lock().unwrap();
    let env_guard = GitHubAuthEnvGuard::capture();
    env_guard.clear_tokens();
    env::set_var("PATH", "");

    let err = resolve_github_token(None).await.unwrap_err();
    assert!(matches!(
        err,
        ConfigError::ValidationError(message)
            if message == "No GitHub token found. Set GITHUB_TOKEN, GH_TOKEN, or run 'gh auth login'."
    ));
}

#[test]
fn expand_home_expands_home_prefixed_path() {
    let home = dirs::home_dir().expect("home directory should exist for this test");
    let expanded = expand_home("~/foo");

    assert_eq!(PathBuf::from(expanded), home.join("foo"));
}

#[test]
fn expand_home_leaves_absolute_path_unchanged() {
    assert_eq!(expand_home("/absolute/path"), "/absolute/path");
}

#[test]
fn expand_home_normalizes_path_separators() {
    let expanded = expand_home("~/foo/bar/baz");
    // Should not contain mixed separators
    let has_forward = expanded.contains('/');
    let has_back = expanded.contains('\\');
    // On any platform, separators should be consistent (not mixed)
    assert!(
        !(has_forward && has_back),
        "path has mixed separators: {expanded}"
    );
}

#[tokio::test]
async fn validate_dispatch_config_rejects_missing_tracker_kind() {
    let err = validate_dispatch_config(&RustyConfig::default())
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        ConfigError::ValidationError(message) if message == "tracker.kind is required"
    ));
}

#[tokio::test]
async fn validate_dispatch_config_rejects_unsupported_tracker_kind() {
    let mut config = RustyConfig::default();
    config.tracker.kind = Some("linear".to_string());

    let err = validate_dispatch_config(&config).await.unwrap_err();
    assert!(matches!(
        err,
        ConfigError::ValidationError(message)
            if message == "unsupported tracker kind: 'linear' (only 'github' supported)"
    ));
}

#[tokio::test]
async fn validate_dispatch_config_rejects_missing_tracker_repo() {
    let mut config = RustyConfig::default();
    config.tracker.kind = Some("github".to_string());
    config.tracker.api_key = Some("token".to_string());

    let err = validate_dispatch_config(&config).await.unwrap_err();
    assert!(matches!(
        err,
        ConfigError::ValidationError(message)
            if message.contains("tracker.repo is required")
    ));
}

#[test]
fn full_repo_combines_owner_and_repo() {
    use rusty::config::schema::TrackerConfig;
    let mut config = TrackerConfig::default();
    config.owner = Some("ridermw".to_string());
    config.repo = Some("rusty".to_string());
    assert_eq!(config.full_repo(), Some("ridermw/rusty".to_string()));
}

#[test]
fn full_repo_uses_combined_format_directly() {
    use rusty::config::schema::TrackerConfig;
    let mut config = TrackerConfig::default();
    config.repo = Some("ridermw/rusty".to_string());
    assert_eq!(config.full_repo(), Some("ridermw/rusty".to_string()));
}

#[test]
fn full_repo_returns_none_when_missing() {
    use rusty::config::schema::TrackerConfig;
    let config = TrackerConfig::default();
    assert_eq!(config.full_repo(), None);
}

#[tokio::test]
async fn validate_accepts_separate_owner_and_repo() {
    let mut config = valid_config();
    config.tracker.repo = Some("rusty".to_string());
    config.tracker.owner = Some("ridermw".to_string());
    assert!(validate_dispatch_config(&config).await.is_ok());
}

#[tokio::test]
async fn validate_dispatch_config_accepts_valid_config() {
    validate_dispatch_config(&valid_config()).await.unwrap();
}

#[test]
fn normalize_state_concurrency_lowercases_keys_and_drops_zero_values() {
    let mut state_limits = HashMap::new();
    state_limits.insert("Open".to_string(), 2);
    state_limits.insert("Closed".to_string(), 0);
    state_limits.insert("In-Progress".to_string(), 3);

    let normalized = normalize_state_concurrency(&state_limits);

    assert_eq!(normalized.len(), 2);
    assert_eq!(normalized.get("open"), Some(&2));
    assert_eq!(normalized.get("in-progress"), Some(&3));
    assert!(!normalized.contains_key("closed"));
}

#[test]
fn deserialize_yaml_applies_defaults() {
    let yaml = r#"
tracker:
  kind: github
  repo: owner/repo
"#;

    let config: RustyConfig = serde_yaml::from_str(yaml).unwrap();

    assert_eq!(config.tracker.kind.as_deref(), Some("github"));
    assert_eq!(config.tracker.repo.as_deref(), Some("owner/repo"));
    assert_eq!(config.tracker.endpoint, None);
    assert_eq!(config.tracker.active_states, vec!["open".to_string()]);
    assert_eq!(config.tracker.terminal_states, vec!["closed".to_string()]);
    assert_eq!(config.polling.interval_ms, 30_000);
    assert_eq!(config.agent.max_turns, 20);
    assert_eq!(config.agent.command, "copilot --acp --yolo --no-ask-user");
    assert_eq!(config.server.host.as_deref(), Some("127.0.0.1"));
    assert_eq!(config.copilot.command, "copilot");
    assert_eq!(config.github.cli_command, "gh");
}

#[test]
fn rusty_config_deserializes_with_copilot_and_github_sections() {
    let yaml = r#"
tracker:
  kind: github
  repo: owner/repo
copilot:
  command: copilot-cli
  chat_command: copilot chat
  approval_policy: on-request
  thread_sandbox: workspace-write
  turn_sandbox_policy:
    mode: strict
github:
  cli_command: ghx
  default_branch: trunk
  required_pr_label: ready
"#;

    let config: RustyConfig = serde_yaml::from_str(yaml).unwrap();

    assert_eq!(config.copilot.command, "copilot-cli");
    assert_eq!(config.copilot.chat_command.as_deref(), Some("copilot chat"));
    assert_eq!(config.copilot.approval_policy, "on-request");
    assert_eq!(
        config.copilot.thread_sandbox.as_deref(),
        Some("workspace-write")
    );
    assert_eq!(
        config.copilot.turn_sandbox_policy,
        Some(serde_yaml::from_str("mode: strict").unwrap())
    );
    assert_eq!(config.github.cli_command, "ghx");
    assert_eq!(config.github.default_branch, "trunk");
    assert_eq!(config.github.required_pr_label.as_deref(), Some("ready"));
}

#[test]
fn effective_active_states_merge_labels_without_duplicates() {
    let mut config = rusty::config::schema::TrackerConfig {
        active_states: vec!["Open".to_string(), "Todo".to_string()],
        active_issue_labels: vec!["todo".to_string(), "In Progress".to_string()],
        ..Default::default()
    };

    assert_eq!(
        config.effective_active_states(),
        vec![
            "Open".to_string(),
            "Todo".to_string(),
            "In Progress".to_string()
        ]
    );

    config.active_issue_labels.push("open".to_string());
    assert_eq!(
        config.effective_active_states(),
        vec![
            "Open".to_string(),
            "Todo".to_string(),
            "In Progress".to_string()
        ]
    );
}

#[test]
fn effective_terminal_states_merge_labels_without_duplicates() {
    let mut config = rusty::config::schema::TrackerConfig {
        terminal_states: vec!["Closed".to_string()],
        terminal_issue_labels: vec!["done".to_string(), "CLOSED".to_string()],
        ..Default::default()
    };

    assert_eq!(
        config.effective_terminal_states(),
        vec!["Closed".to_string(), "done".to_string()]
    );

    config.terminal_issue_labels.push("Done".to_string());
    assert_eq!(
        config.effective_terminal_states(),
        vec!["Closed".to_string(), "done".to_string()]
    );
}

#[test]
fn agent_launch_command_returns_agent_command_directly() {
    let config = RustyConfig::default();
    assert_eq!(
        agent_launch_command(&config),
        "copilot --acp --yolo --no-ask-user"
    );

    let mut custom = RustyConfig::default();
    custom.agent.command = "custom-agent --stdio".to_string();
    assert_eq!(agent_launch_command(&custom), "custom-agent --stdio");
}
