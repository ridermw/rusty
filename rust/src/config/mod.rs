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

/// Expand `~` to home directory using dirs::home_dir().
pub fn expand_home(path: &str) -> String {
    if path == "~" || path.starts_with("~/") || path.starts_with("~\\") {
        if let Some(home) = dirs::home_dir() {
            return path.replacen('~', &home.to_string_lossy(), 1);
        }
    }

    path.to_string()
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

pub fn validate_dispatch_config(config: &RustyConfig) -> Result<(), ConfigError> {
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

    let api_key = config.tracker.api_key.as_deref().unwrap_or("$GITHUB_TOKEN");
    resolve_env_value(api_key)?;

    if config.tracker.full_repo().is_none() {
        return Err(ConfigError::ValidationError(
            "tracker.repo is required (format: owner/repo, or set tracker.owner + tracker.repo separately)".to_string(),
        ));
    }

    if config.agent.command.is_empty() {
        return Err(ConfigError::ValidationError(
            "agent.command must not be empty".to_string(),
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
