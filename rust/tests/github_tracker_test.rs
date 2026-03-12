use std::collections::HashMap;

use serde_json::json;
use symphony::config::schema::TrackerConfig;
use symphony::tracker::github::client::normalize_github_issue;

fn github_config(state_labels: &[(&str, &str)]) -> TrackerConfig {
    TrackerConfig {
        repo: Some("acme/demo-repo".to_string()),
        state_labels: state_labels
            .iter()
            .map(|(label, state)| (label.to_string(), state.to_string()))
            .collect::<HashMap<_, _>>(),
        ..TrackerConfig::default()
    }
}

#[test]
fn normalize_valid_github_issue_json() {
    let issue = normalize_github_issue(
        &json!({
            "number": 42,
            "title": "Fix login",
            "state": "open",
            "body": "Investigate auth edge case",
            "html_url": "https://github.com/acme/demo-repo/issues/42",
            "labels": [
                { "name": "Bug" },
                { "name": "PRIORITY-2" }
            ],
            "created_at": "2024-01-02T03:04:05Z",
            "updated_at": "2024-01-03T04:05:06Z"
        }),
        "demo-repo",
        &github_config(&[]),
    )
    .expect("issue should normalize");

    assert_eq!(issue.id, "42");
    assert_eq!(issue.identifier, "demo-repo-42");
    assert_eq!(issue.title, "Fix login");
    assert_eq!(issue.state, "open");
    assert_eq!(issue.labels, vec!["bug", "priority-2"]);
}

#[test]
fn normalize_maps_labels_to_configured_state() {
    let issue = normalize_github_issue(
        &json!({
            "number": 7,
            "title": "Implement workflow",
            "state": "open",
            "labels": [
                { "name": "In-Progress" }
            ]
        }),
        "demo-repo",
        &github_config(&[("in-progress", "in_progress")]),
    )
    .expect("issue should normalize");

    assert_eq!(issue.state, "in_progress");
}

#[test]
fn normalize_extracts_priority_from_priority_label() {
    let issue = normalize_github_issue(
        &json!({
            "number": 8,
            "title": "Ship feature",
            "state": "open",
            "labels": [
                { "name": "priority-1" },
                { "name": "enhancement" }
            ]
        }),
        "demo-repo",
        &github_config(&[]),
    )
    .expect("issue should normalize");

    assert_eq!(issue.priority, Some(1));
}

#[test]
fn normalize_missing_optional_fields_to_none_or_empty() {
    let issue = normalize_github_issue(
        &json!({
            "number": 9,
            "title": "Document behavior",
            "state": "closed"
        }),
        "demo-repo",
        &github_config(&[]),
    )
    .expect("issue should normalize");

    assert_eq!(issue.description, None);
    assert_eq!(issue.priority, None);
    assert_eq!(issue.url, None);
    assert!(issue.labels.is_empty());
    assert!(issue.blocked_by.is_empty());
    assert_eq!(issue.created_at, None);
    assert_eq!(issue.updated_at, None);
}

#[test]
fn normalize_uses_repo_name_in_identifier() {
    let issue = normalize_github_issue(
        &json!({
            "number": 42,
            "title": "Keep identifier stable",
            "state": "open",
            "labels": []
        }),
        "demo-repo",
        &github_config(&[]),
    )
    .expect("issue should normalize");

    assert_eq!(issue.identifier, "demo-repo-42");
}

#[test]
fn normalize_empty_labels_array_to_empty_vec() {
    let issue = normalize_github_issue(
        &json!({
            "number": 12,
            "title": "No labels yet",
            "state": "open",
            "labels": []
        }),
        "demo-repo",
        &github_config(&[]),
    )
    .expect("issue should normalize");

    assert!(issue.labels.is_empty());
}
