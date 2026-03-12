use std::{collections::HashMap, env, path::PathBuf};

use symphony::config::schema::SymphonyConfig;
use symphony::config::{
    expand_home, normalize_state_concurrency, resolve_env_value, validate_dispatch_config,
    ConfigError,
};

fn valid_config() -> SymphonyConfig {
    let mut config = SymphonyConfig::default();
    config.tracker.kind = Some("github".to_string());
    config.tracker.repo = Some("owner/repo".to_string());
    config.tracker.api_key = Some("token".to_string());
    config
}

#[test]
fn config_defaults_are_correct() {
    let config = SymphonyConfig::default();
    let expected_workspace_root = std::env::temp_dir()
        .join("symphony_workspaces")
        .to_string_lossy()
        .into_owned();

    assert_eq!(config.tracker.kind, None);
    assert_eq!(config.tracker.endpoint, None);
    assert_eq!(config.tracker.api_key, None);
    assert_eq!(config.tracker.repo, None);
    assert_eq!(config.tracker.active_states, vec!["open".to_string()]);
    assert_eq!(config.tracker.terminal_states, vec!["closed".to_string()]);
    assert!(config.tracker.labels.is_empty());
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
    assert_eq!(config.agent.command, "copilot --acp --stdio");
    assert_eq!(config.agent.turn_timeout_ms, 3_600_000);
    assert_eq!(config.agent.read_timeout_ms, 5_000);
    assert_eq!(config.agent.stall_timeout_ms, 300_000);
    assert_eq!(config.agent.approval_policy, "auto-approve");

    assert_eq!(config.server.port, None);
    assert_eq!(config.server.host.as_deref(), Some("127.0.0.1"));
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
fn validate_dispatch_config_rejects_missing_tracker_kind() {
    let err = validate_dispatch_config(&SymphonyConfig::default()).unwrap_err();
    assert!(matches!(
        err,
        ConfigError::ValidationError(message) if message == "tracker.kind is required"
    ));
}

#[test]
fn validate_dispatch_config_rejects_unsupported_tracker_kind() {
    let mut config = SymphonyConfig::default();
    config.tracker.kind = Some("linear".to_string());

    let err = validate_dispatch_config(&config).unwrap_err();
    assert!(matches!(
        err,
        ConfigError::ValidationError(message)
            if message == "unsupported tracker kind: 'linear' (only 'github' supported)"
    ));
}

#[test]
fn validate_dispatch_config_rejects_missing_tracker_repo() {
    let mut config = SymphonyConfig::default();
    config.tracker.kind = Some("github".to_string());
    config.tracker.api_key = Some("token".to_string());

    let err = validate_dispatch_config(&config).unwrap_err();
    assert!(matches!(
        err,
        ConfigError::ValidationError(message)
            if message == "tracker.repo is required (format: owner/repo)"
    ));
}

#[test]
fn validate_dispatch_config_accepts_valid_config() {
    validate_dispatch_config(&valid_config()).unwrap();
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

    let config: SymphonyConfig = serde_yaml::from_str(yaml).unwrap();

    assert_eq!(config.tracker.kind.as_deref(), Some("github"));
    assert_eq!(config.tracker.repo.as_deref(), Some("owner/repo"));
    assert_eq!(config.tracker.endpoint, None);
    assert_eq!(config.tracker.active_states, vec!["open".to_string()]);
    assert_eq!(config.tracker.terminal_states, vec!["closed".to_string()]);
    assert_eq!(config.polling.interval_ms, 30_000);
    assert_eq!(config.agent.max_turns, 20);
    assert_eq!(config.agent.command, "copilot --acp --stdio");
    assert_eq!(config.server.host.as_deref(), Some("127.0.0.1"));
}
