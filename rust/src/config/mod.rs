pub mod schema;

use self::schema::RustyConfig;
use std::{collections::HashMap, env, path::PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing workflow file: {0}")]
    MissingWorkflowFile(PathBuf),
    #[error("workflow parse error: {0}")]
    WorkflowParseError(String),
    #[error("workflow front matter is not a map")]
    WorkflowFrontMatterNotAMap,
    #[error("template parse error: {0}")]
    TemplateParseError(String),
    #[error("template render error: {0}")]
    TemplateRenderError(String),
    #[error("validation error: {0}")]
    ValidationError(String),
}

/// Resolve `$VAR_NAME` references in a string to environment variable values.
/// Returns the original string if no `$` prefix. Returns error if var is unset or empty.
pub fn resolve_env_value(value: &str) -> Result<String, ConfigError> {
    if let Some(var_name) = value.strip_prefix('$') {
        match env::var(var_name) {
            Ok(val) if !val.is_empty() => Ok(val),
            _ => Err(ConfigError::ValidationError(format!(
                "environment variable '{}' is not set or empty",
                var_name
            ))),
        }
    } else {
        Ok(value.to_string())
    }
}

/// Resolve GitHub auth token from multiple sources (in priority order):
/// 1. Explicit literal config value
/// 2. GITHUB_TOKEN env var
/// 3. GH_TOKEN env var (gh CLI convention)
/// 4. `gh auth token` subprocess output
///
/// Returns the token string or an error if no source provides one.
pub async fn resolve_github_token(config_value: Option<&str>) -> Result<String, ConfigError> {
    match config_value {
        Some(val) if !val.is_empty() && !val.starts_with('$') => return Ok(val.to_string()),
        Some(val) if !val.is_empty() && val != "$GITHUB_TOKEN" && val != "$GH_TOKEN" => {
            return resolve_env_value(val);
        }
        _ => {}
    }

    if let Ok(token) = env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            return Ok(token);
        }
    }

    if let Ok(token) = env::var("GH_TOKEN") {
        if !token.is_empty() {
            return Ok(token);
        }
    }

    match tokio::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !token.is_empty() {
                return Ok(token);
            }
        }
        _ => {}
    }

    Err(ConfigError::ValidationError(
        "No GitHub token found. Set GITHUB_TOKEN, GH_TOKEN, or run 'gh auth login'.".to_string(),
    ))
}

/// Expand `~` to home directory using dirs::home_dir().
pub fn expand_home(path: &str) -> String {
    if path == "~" || path.starts_with("~/") || path.starts_with("~\\") {
        if let Some(home) = dirs::home_dir() {
            let expanded = path.replacen('~', &home.to_string_lossy(), 1);
            return normalize_path_separators(&expanded);
        }
    }

    path.to_string()
}

/// Normalize path separators to the platform default.
/// On Windows: forward slashes → backslashes.
/// On Unix: backslashes → forward slashes.
pub fn normalize_path_separators(path: &str) -> String {
    if cfg!(windows) {
        path.replace('/', "\\")
    } else {
        path.replace('\\', "/")
    }
}

/// Resolve a path value: expand ~ then resolve $VAR.
pub fn resolve_path(value: &str) -> Result<String, ConfigError> {
    let expanded = expand_home(value);
    if expanded.contains('$') {
        resolve_env_value(&expanded)
    } else {
        Ok(expanded)
    }
}

/// Get the effective agent launch command.
/// Prefers agent.command if non-default, otherwise falls back to copilot.command.
pub fn effective_agent_command(config: &RustyConfig) -> &str {
    if config.agent.command != "copilot --acp --stdio" && !config.agent.command.is_empty() {
        &config.agent.command
    } else {
        &config.copilot.command
    }
}

pub async fn validate_dispatch_config(config: &RustyConfig) -> Result<(), ConfigError> {
    match &config.tracker.kind {
        Some(kind) if kind == "github" => {}
        Some(kind) => {
            return Err(ConfigError::ValidationError(format!(
                "unsupported tracker kind: '{}' (only 'github' supported)",
                kind
            )));
        }
        None => {
            return Err(ConfigError::ValidationError(
                "tracker.kind is required".to_string(),
            ));
        }
    }

    // Validate GitHub auth via the full resolution chain:
    // explicit literal → GITHUB_TOKEN env → GH_TOKEN env → gh auth token
    match resolve_github_token(config.tracker.api_key.as_deref()).await {
        Ok(_) => {}
        Err(_) => {
            return Err(ConfigError::ValidationError(
                "No GitHub token found.\n  \
                 Option 1: $env:GITHUB_TOKEN = \"ghp_your_token\"\n  \
                 Option 2: $env:GH_TOKEN = \"ghp_your_token\"\n  \
                 Option 3: gh auth login\n  \
                 Required scopes: repo, read:discussion, project"
                    .to_string(),
            ));
        }
    }

    if config.tracker.full_repo().is_none() {
        return Err(ConfigError::ValidationError(
            "tracker.repo is required (format: owner/repo, or set tracker.owner + tracker.repo separately)".to_string(),
        ));
    }

    if effective_agent_command(config).is_empty() {
        return Err(ConfigError::ValidationError(
            "agent.command or copilot.command must not be empty".to_string(),
        ));
    }

    Ok(())
}

/// Normalize max_concurrent_agents_by_state: lowercase keys, drop invalid (zero) values.
pub fn normalize_state_concurrency(map: &HashMap<String, usize>) -> HashMap<String, usize> {
    map.iter()
        .filter(|(_, &value)| value > 0)
        .map(|(key, &value)| (key.to_lowercase(), value))
        .collect()
}
