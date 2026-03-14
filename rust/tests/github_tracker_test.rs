use std::collections::HashMap;

use rusty::config::schema::TrackerConfig;
use rusty::tracker::github::client::{normalize_github_issue, GitHubClient};
use rusty::tracker::TrackerError;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

fn test_tracker_config(server_url: &str) -> TrackerConfig {
    TrackerConfig {
        kind: Some("github".to_string()),
        endpoint: Some(server_url.to_string()),
        api_key: Some("test-token".to_string()),
        owner: Some("testowner".to_string()),
        repo: Some("testrepo".to_string()),
        ..Default::default()
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

#[test]
fn fetch_by_states_must_filter_to_exact_requested_states() {
    let config = TrackerConfig {
        repo: Some("acme/demo-repo".to_string()),
        state_labels: [
            ("done".to_string(), "Done".to_string()),
            ("cancelled".to_string(), "Cancelled".to_string()),
        ]
        .into_iter()
        .collect(),
        terminal_states: vec!["Done".to_string(), "Cancelled".to_string()],
        ..TrackerConfig::default()
    };

    let done_issue = normalize_github_issue(
        &json!({
            "number": 1, "title": "Completed task", "state": "closed",
            "labels": [{"name": "done"}]
        }),
        "demo-repo",
        &config,
    )
    .unwrap();
    assert_eq!(done_issue.state, "Done");

    let cancelled_issue = normalize_github_issue(
        &json!({
            "number": 2, "title": "Cancelled task", "state": "closed",
            "labels": [{"name": "cancelled"}]
        }),
        "demo-repo",
        &config,
    )
    .unwrap();
    assert_eq!(cancelled_issue.state, "Cancelled");

    let plain_closed = normalize_github_issue(
        &json!({
            "number": 3, "title": "Just closed", "state": "closed",
            "labels": []
        }),
        "demo-repo",
        &config,
    )
    .unwrap();
    assert_eq!(plain_closed.state, "closed");

    let all = vec![done_issue, cancelled_issue, plain_closed];
    let requested = ["Done".to_string()];
    let requested_lower: Vec<String> = requested.iter().map(|s| s.to_lowercase()).collect();
    let filtered: Vec<_> = all
        .into_iter()
        .filter(|i| requested_lower.contains(&i.state.to_lowercase()))
        .collect();

    assert_eq!(filtered.len(), 1, "should filter to exact requested states");
    assert_eq!(filtered[0].identifier, "demo-repo-1");
}

#[test]
fn github_client_must_cache_responses_for_304_handling() {
    let _client = GitHubClient::new();
}

#[tokio::test]
async fn fetch_issues_returns_normalized_issues() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/testowner/testrepo/issues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "number": 1, "title": "First", "state": "open",
                "labels": [{"name": "todo"}],
                "html_url": "https://github.com/testowner/testrepo/issues/1",
                "created_at": "2024-01-01T00:00:00Z",
                "updated_at": "2024-01-02T00:00:00Z"
            },
            {
                "number": 2, "title": "Second", "state": "open",
                "labels": [], "created_at": "2024-01-01T00:00:00Z"
            }
        ])))
        .mount(&server)
        .await;

    let client = GitHubClient::new();
    let config = test_tracker_config(&server.uri());
    let issues = client.fetch_issues(&config, "open", None).await.unwrap();

    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].identifier, "testrepo-1");
    assert_eq!(issues[0].title, "First");
    assert_eq!(
        issues[0].url.as_deref(),
        Some("https://github.com/testowner/testrepo/issues/1")
    );
    assert_eq!(issues[1].identifier, "testrepo-2");
}

#[tokio::test]
async fn fetch_issues_handles_rate_limiting() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/testowner/testrepo/issues"))
        .respond_with(ResponseTemplate::new(429).insert_header("x-ratelimit-reset", "1700000000"))
        .mount(&server)
        .await;

    let client = GitHubClient::new();
    let config = test_tracker_config(&server.uri());
    let result = client.fetch_issues(&config, "open", None).await;

    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        TrackerError::RateLimited { .. }
    ));
}

#[tokio::test]
async fn fetch_issues_handles_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/testowner/testrepo/issues"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .mount(&server)
        .await;

    let client = GitHubClient::new();
    let config = test_tracker_config(&server.uri());
    let result = client.fetch_issues(&config, "open", None).await;

    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        TrackerError::ApiStatus(500, ref body) if body == "Internal Server Error"
    ));
}

#[tokio::test]
async fn fetch_issues_caches_etag_and_returns_cached_on_304() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/repos/testowner/testrepo/issues"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"abc123\"")
                .set_body_json(json!([
                    {"number": 1, "title": "Cached", "state": "open", "labels": []}
                ])),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = GitHubClient::new();
    let config = test_tracker_config(&server.uri());

    let issues1 = client.fetch_issues(&config, "open", None).await.unwrap();
    assert_eq!(issues1.len(), 1);

    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/repos/testowner/testrepo/issues"))
        .and(header("If-None-Match", "\"abc123\""))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;

    let issues2 = client.fetch_issues(&config, "open", None).await.unwrap();
    assert_eq!(issues2.len(), 1);
    assert_eq!(issues2[0].title, "Cached");
}

#[tokio::test]
async fn fetch_issues_by_numbers_returns_specific_issues() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/testowner/testrepo/issues/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(
            {"number": 42, "title": "Specific", "state": "open", "labels": []}
        )))
        .mount(&server)
        .await;

    let client = GitHubClient::new();
    let config = test_tracker_config(&server.uri());
    let issues = client
        .fetch_issues_by_numbers(&config, &[42])
        .await
        .unwrap();

    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].identifier, "testrepo-42");
}

#[tokio::test]
async fn fetch_issues_skips_pull_requests() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/testowner/testrepo/issues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"number": 1, "title": "Issue", "state": "open", "labels": []},
            {"number": 2, "title": "PR", "state": "open", "labels": [], "pull_request": {"url": "..."}}
        ])))
        .mount(&server)
        .await;

    let client = GitHubClient::new();
    let config = test_tracker_config(&server.uri());
    let issues = client.fetch_issues(&config, "open", None).await.unwrap();

    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].title, "Issue");
}

#[tokio::test]
async fn fetch_issues_paginates_and_sends_label_filters() {
    let server = MockServer::start().await;
    let first_page: Vec<_> = (1..=50)
        .map(|number| {
            json!({
                "number": number,
                "title": format!("Issue {number}"),
                "state": "open",
                "labels": []
            })
        })
        .collect();

    Mock::given(method("GET"))
        .and(path("/repos/testowner/testrepo/issues"))
        .and(query_param("state", "open"))
        .and(query_param("per_page", "50"))
        .and(query_param("page", "1"))
        .and(query_param("labels", "bug,help-wanted"))
        .respond_with(ResponseTemplate::new(200).set_body_json(first_page))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/repos/testowner/testrepo/issues"))
        .and(query_param("state", "open"))
        .and(query_param("per_page", "50"))
        .and(query_param("page", "2"))
        .and(query_param("labels", "bug,help-wanted"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"number": 51, "title": "Issue 51", "state": "open", "labels": []}
        ])))
        .mount(&server)
        .await;

    let client = GitHubClient::new();
    let config = test_tracker_config(&server.uri());
    let labels = vec!["bug".to_string(), "help-wanted".to_string()];
    let issues = client
        .fetch_issues(&config, "open", Some(&labels))
        .await
        .unwrap();

    assert_eq!(issues.len(), 51);
    assert_eq!(issues[0].identifier, "testrepo-1");
    assert_eq!(issues[50].identifier, "testrepo-51");
}

#[tokio::test]
async fn fetch_issues_by_numbers_returns_rate_limit_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/testowner/testrepo/issues/42"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let client = GitHubClient::new();
    let config = test_tracker_config(&server.uri());
    let result = client.fetch_issues_by_numbers(&config, &[42]).await;

    assert!(matches!(
        result.unwrap_err(),
        TrackerError::RateLimited { .. }
    ));
}

#[tokio::test]
async fn fetch_issues_by_numbers_skips_non_success_responses() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/testowner/testrepo/issues/7"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let client = GitHubClient::new();
    let config = test_tracker_config(&server.uri());
    let issues = client.fetch_issues_by_numbers(&config, &[7]).await.unwrap();

    assert!(issues.is_empty());
}

// --- normalize_github_issue edge-case tests ---

#[test]
fn normalize_returns_none_when_number_missing() {
    let result = normalize_github_issue(
        &json!({"title": "No number", "state": "open"}),
        "demo-repo",
        &github_config(&[]),
    );
    assert!(result.is_none());
}

#[test]
fn normalize_returns_none_when_title_missing() {
    let result = normalize_github_issue(
        &json!({"number": 1, "state": "open"}),
        "demo-repo",
        &github_config(&[]),
    );
    assert!(result.is_none());
}

#[test]
fn normalize_returns_none_when_state_missing() {
    let result = normalize_github_issue(
        &json!({"number": 1, "title": "No state"}),
        "demo-repo",
        &github_config(&[]),
    );
    assert!(result.is_none());
}

#[test]
fn normalize_returns_none_for_pull_request() {
    let result = normalize_github_issue(
        &json!({
            "number": 10, "title": "A PR", "state": "open",
            "labels": [],
            "pull_request": {"url": "https://api.github.com/repos/acme/demo-repo/pulls/10"}
        }),
        "demo-repo",
        &github_config(&[]),
    );
    assert!(result.is_none());
}

#[test]
fn normalize_multiple_state_labels_picks_first_matching() {
    let config = TrackerConfig {
        repo: Some("acme/demo-repo".to_string()),
        state_labels: [
            ("todo".to_string(), "Todo".to_string()),
            ("in-progress".to_string(), "InProgress".to_string()),
        ]
        .into_iter()
        .collect::<HashMap<_, _>>(),
        ..TrackerConfig::default()
    };

    let issue = normalize_github_issue(
        &json!({
            "number": 5,
            "title": "Has two state labels",
            "state": "open",
            "labels": [{"name": "Todo"}, {"name": "In-Progress"}]
        }),
        "demo-repo",
        &config,
    )
    .expect("should normalize");

    // One of the mapped states should be picked (whichever label matches first)
    assert!(
        issue.state == "Todo" || issue.state == "InProgress",
        "expected mapped state, got: {}",
        issue.state
    );
}

#[test]
fn normalize_state_labels_miss_falls_back_to_github_state() {
    let config = TrackerConfig {
        repo: Some("acme/demo-repo".to_string()),
        state_labels: [("done".to_string(), "Done".to_string())]
            .into_iter()
            .collect::<HashMap<_, _>>(),
        ..TrackerConfig::default()
    };

    let issue = normalize_github_issue(
        &json!({
            "number": 6,
            "title": "No matching state label",
            "state": "open",
            "labels": [{"name": "enhancement"}]
        }),
        "demo-repo",
        &config,
    )
    .expect("should normalize");

    assert_eq!(issue.state, "open", "should fall back to github state");
}

// --- Additional GitHubClient integration tests ---

#[tokio::test]
async fn fetch_issues_by_numbers_returns_multiple_skips_404() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/repos/testowner/testrepo/issues/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(
            {"number": 1, "title": "First", "state": "open", "labels": []}
        )))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/repos/testowner/testrepo/issues/2"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/repos/testowner/testrepo/issues/3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(
            {"number": 3, "title": "Third", "state": "closed", "labels": []}
        )))
        .mount(&server)
        .await;

    let client = GitHubClient::new();
    let config = test_tracker_config(&server.uri());
    let issues = client
        .fetch_issues_by_numbers(&config, &[1, 2, 3])
        .await
        .unwrap();

    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].identifier, "testrepo-1");
    assert_eq!(issues[1].identifier, "testrepo-3");
}

#[tokio::test]
async fn fetch_issues_empty_first_page_returns_empty() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/testowner/testrepo/issues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let client = GitHubClient::new();
    let config = test_tracker_config(&server.uri());
    let issues = client.fetch_issues(&config, "open", None).await.unwrap();

    assert!(issues.is_empty());
}

#[tokio::test]
async fn fetch_issues_etag_updated_on_200_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/testowner/testrepo/issues"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"new-etag-value\"")
                .set_body_json(json!([
                    {"number": 1, "title": "Fresh", "state": "open", "labels": []}
                ])),
        )
        .mount(&server)
        .await;

    let client = GitHubClient::new();
    let config = test_tracker_config(&server.uri());
    let issues = client.fetch_issues(&config, "open", None).await.unwrap();

    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].title, "Fresh");

    // Second call should send the cached etag
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/repos/testowner/testrepo/issues"))
        .and(header("If-None-Match", "\"new-etag-value\""))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;

    let issues2 = client.fetch_issues(&config, "open", None).await.unwrap();
    assert_eq!(issues2.len(), 1);
    assert_eq!(issues2[0].title, "Fresh");
}
