pub mod store;

use std::path::Path;

use serde_yaml::{Mapping, Value};

use crate::config::ConfigError;

#[derive(Debug, Clone)]
pub struct WorkflowDefinition {
    pub config: Value,
    pub prompt_template: String,
}

/// Parse WORKFLOW.md content into config + prompt template.
pub fn parse_workflow(content: &str) -> Result<WorkflowDefinition, ConfigError> {
    let lines: Vec<&str> = content.lines().collect();

    if matches!(lines.first(), Some(&"---")) {
        let closing_idx = lines[1..]
            .iter()
            .position(|line| *line == "---")
            .map(|idx| idx + 1)
            .ok_or_else(|| {
                ConfigError::WorkflowParseError(
                    "YAML front matter has opening --- but no closing ---".to_string(),
                )
            })?;

        let yaml_str = lines[1..closing_idx].join("\n");
        let prompt = lines
            .get(closing_idx + 1..)
            .unwrap_or(&[])
            .join("\n")
            .trim()
            .to_string();

        let config = if yaml_str.trim().is_empty() {
            Value::Mapping(Mapping::new())
        } else {
            serde_yaml::from_str(&yaml_str)
                .map_err(|e| ConfigError::WorkflowParseError(e.to_string()))?
        };

        if !config.is_mapping() {
            Err(ConfigError::WorkflowFrontMatterNotAMap)
        } else {
            Ok(WorkflowDefinition {
                config,
                prompt_template: prompt,
            })
        }
    } else {
        Ok(WorkflowDefinition {
            config: Value::Mapping(Mapping::new()),
            prompt_template: content.trim().to_string(),
        })
    }
}

/// Load workflow from file path.
pub fn load_workflow(path: &Path) -> Result<WorkflowDefinition, ConfigError> {
    let content = std::fs::read_to_string(path)
        .map_err(|_| ConfigError::MissingWorkflowFile(path.to_path_buf()))?;
    parse_workflow(&content)
}
