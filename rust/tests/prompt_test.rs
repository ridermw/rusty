use rusty::config::ConfigError;
use rusty::prompt::render_prompt;
use rusty::tracker::memory::test_issue;
use rusty::tracker::Issue;

fn sample_issue() -> Issue {
    let mut issue = test_issue(
        "1",
        "ISSUE-123",
        "Investigate prompt builder",
        "open",
        Some(2),
    );
    issue.description = Some("Render the workflow template".to_string());
    issue.branch_name = Some("feat/issue-123".to_string());
    issue.url = Some("https://example.test/issues/123".to_string());
    issue.labels = vec!["bug".to_string(), "backend".to_string()];
    issue
}

#[test]
fn renders_issue_identifier_and_title() {
    let issue = sample_issue();

    let rendered =
        render_prompt("{{ issue.identifier }}: {{ issue.title }}", &issue, None).unwrap();

    assert_eq!(rendered, "ISSUE-123: Investigate prompt builder");
}

#[test]
fn renders_issue_number_alias() {
    let issue = sample_issue();

    let rendered = render_prompt("{{ issue.number }}", &issue, None).unwrap();

    assert_eq!(rendered, "ISSUE-123");
}

#[test]
fn renders_issue_body_alias() {
    let issue = sample_issue();

    let rendered = render_prompt("{{ issue.body }}", &issue, None).unwrap();

    assert_eq!(rendered, "Render the workflow template");
}

#[test]
fn renders_attempt_when_present() {
    let issue = sample_issue();

    let rendered = render_prompt("{{ attempt }}", &issue, Some(3)).unwrap();

    assert_eq!(rendered, "3");
}

#[test]
fn returns_fallback_prompt_for_empty_template() {
    let issue = sample_issue();

    let rendered = render_prompt("   ", &issue, None).unwrap();

    assert_eq!(rendered, "You are working on an issue from GitHub.");
}

#[test]
fn returns_render_error_for_unknown_variable() {
    let issue = sample_issue();

    let error = render_prompt("{{ unknown_var }}", &issue, None).unwrap_err();

    assert!(matches!(error, ConfigError::TemplateRenderError(_)));
}

#[test]
fn renders_issue_labels_in_loop() {
    let issue = sample_issue();

    let rendered = render_prompt(
        "{% for l in issue.labels %}{{ l }} {% endfor %}",
        &issue,
        None,
    )
    .unwrap();

    assert_eq!(rendered, "bug backend ");
}

#[test]
fn missing_attempt_variable_is_falsey_in_conditionals() {
    let issue = sample_issue();

    let rendered = render_prompt(
        "{% if attempt %}retry {{ attempt }}{% endif %}",
        &issue,
        None,
    )
    .unwrap();

    assert_eq!(rendered, "");
}

#[test]
fn renders_none_description_as_empty_string() {
    let issue = test_issue("1", "ISSUE-1", "No desc issue", "open", None);

    let rendered =
        render_prompt("desc=[{{ issue.description }}]", &issue, None).unwrap();

    assert_eq!(rendered, "desc=[]");
}

#[test]
fn renders_special_characters_in_fields() {
    let mut issue = test_issue("1", "ISSUE-1", "Fix <html> & \"quotes\"", "open", None);
    issue.description = Some("Héllo wörld 🚀 foo&bar".to_string());

    let rendered = render_prompt(
        "{{ issue.title }} — {{ issue.description }}",
        &issue,
        None,
    )
    .unwrap();

    assert_eq!(rendered, "Fix <html> & \"quotes\" — Héllo wörld 🚀 foo&bar");
}

#[test]
fn returns_parse_error_for_invalid_liquid_syntax() {
    let issue = sample_issue();

    let error = render_prompt("{% if %}", &issue, None).unwrap_err();

    assert!(matches!(error, ConfigError::TemplateParseError(_)));
}
