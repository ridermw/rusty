//! Liquid-compatible prompt template rendering with strict mode.

use liquid::{
    model::{to_value, Value},
    Object, ParserBuilder,
};

use crate::config::ConfigError;
use crate::tracker::Issue;

/// Render a prompt template with issue and attempt context.
///
/// - `template_str`: The Liquid template body from WORKFLOW.md
/// - `issue`: The normalized issue to render into the template
/// - `attempt`: None on first run, Some(n) on retry/continuation
///
/// Returns the rendered prompt string or a template error.
pub fn render_prompt(
    template_str: &str,
    issue: &Issue,
    attempt: Option<u32>,
) -> Result<String, ConfigError> {
    if template_str.trim().is_empty() {
        return Ok("You are working on an issue from GitHub.".to_string());
    }

    let parser = ParserBuilder::with_stdlib()
        .build()
        .map_err(|error| ConfigError::TemplateParseError(error.to_string()))?;

    let template = parser
        .parse(template_str)
        .map_err(|error| ConfigError::TemplateParseError(error.to_string()))?;

    let mut issue_obj = Object::new();
    issue_obj.insert("id".into(), Value::scalar(issue.id.clone()));
    issue_obj.insert("identifier".into(), Value::scalar(issue.identifier.clone()));
    // Aliases for WORKFLOW.md compatibility
    issue_obj.insert("number".into(), Value::scalar(issue.identifier.clone()));
    issue_obj.insert("title".into(), Value::scalar(issue.title.clone()));
    issue_obj.insert(
        "description".into(),
        Value::scalar(issue.description.clone().unwrap_or_default()),
    );
    issue_obj.insert(
        "body".into(),
        Value::scalar(issue.description.as_deref().unwrap_or("").to_string()),
    );
    issue_obj.insert(
        "priority".into(),
        to_value(&issue.priority).expect("serializing issue priority should never fail"),
    );
    issue_obj.insert("state".into(), Value::scalar(issue.state.clone()));
    issue_obj.insert(
        "url".into(),
        Value::scalar(issue.url.clone().unwrap_or_default()),
    );
    issue_obj.insert(
        "labels".into(),
        to_value(&issue.labels).expect("serializing issue labels should never fail"),
    );
    issue_obj.insert(
        "branch_name".into(),
        Value::scalar(issue.branch_name.clone().unwrap_or_default()),
    );

    let mut globals = Object::new();
    globals.insert("issue".into(), Value::Object(issue_obj));

    if let Some(attempt) = attempt {
        globals.insert("attempt".into(), Value::scalar(i64::from(attempt)));
    }

    template
        .render(&globals)
        .map_err(|error| ConfigError::TemplateRenderError(error.to_string()))
}
