use rusty::config::schema::TrackerConfig;
use rusty::tracker::memory::{test_issue, MemoryTracker};
use rusty::tracker::Tracker;

fn tracker_config(active_states: &[&str]) -> TrackerConfig {
    TrackerConfig {
        active_states: active_states
            .iter()
            .map(|state| state.to_string())
            .collect(),
        ..TrackerConfig::default()
    }
}

#[tokio::test]
async fn fetch_candidate_issues_returns_only_active_state_issues() {
    let tracker = MemoryTracker::new(vec![
        test_issue("1", "ISSUE-1", "First", "open", Some(1)),
        test_issue("2", "ISSUE-2", "Second", "closed", Some(2)),
        test_issue("3", "ISSUE-3", "Third", "OPEN", Some(3)),
    ]);

    let issues = tracker
        .fetch_candidate_issues(&tracker_config(&["open"]))
        .await
        .unwrap();

    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].id, "1");
    assert_eq!(issues[1].id, "3");
}

#[tokio::test]
async fn fetch_candidate_issues_with_empty_tracker_returns_empty() {
    let tracker = MemoryTracker::new(vec![]);

    let issues = tracker
        .fetch_candidate_issues(&tracker_config(&["open"]))
        .await
        .unwrap();

    assert!(issues.is_empty());
}

#[tokio::test]
async fn memory_tracker_filters_by_active_issue_labels() {
    use rusty::config::schema::TrackerConfig;
    use rusty::tracker::memory::{test_issue, MemoryTracker};
    use rusty::tracker::Tracker;

    let mut issue_with_label = test_issue("1", "repo-1", "Has todo label", "open", Some(1));
    issue_with_label.labels = vec!["todo".to_string()];

    let issue_without_label = test_issue("2", "repo-2", "No workflow label", "open", Some(2));
    // no labels — should be filtered out

    let mut issue_enhancement = test_issue("3", "repo-3", "Enhancement", "open", Some(3));
    issue_enhancement.labels = vec!["enhancement".to_string()];

    let tracker = MemoryTracker::new(vec![
        issue_with_label,
        issue_without_label,
        issue_enhancement,
    ]);

    let mut config = TrackerConfig::default();
    config.active_issue_labels = vec!["todo".to_string(), "in_progress".to_string()];

    let candidates = tracker.fetch_candidate_issues(&config).await.unwrap();

    // Only the issue with "todo" label should be returned
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].identifier, "repo-1");
}

#[tokio::test]
async fn memory_tracker_returns_all_when_no_label_filter() {
    use rusty::config::schema::TrackerConfig;
    use rusty::tracker::memory::{test_issue, MemoryTracker};
    use rusty::tracker::Tracker;

    let tracker = MemoryTracker::new(vec![
        test_issue("1", "repo-1", "Issue 1", "open", Some(1)),
        test_issue("2", "repo-2", "Issue 2", "open", Some(2)),
    ]);

    let config = TrackerConfig::default(); // no active_issue_labels

    let candidates = tracker.fetch_candidate_issues(&config).await.unwrap();
    assert_eq!(candidates.len(), 2); // All open issues returned when no label filter
}

#[tokio::test]
async fn fetch_issue_states_by_ids_returns_correct_subset() {
    let tracker = MemoryTracker::new(vec![
        test_issue("1", "ISSUE-1", "First", "open", Some(1)),
        test_issue("2", "ISSUE-2", "Second", "closed", Some(2)),
        test_issue("3", "ISSUE-3", "Third", "open", Some(3)),
    ]);

    let issues = tracker
        .fetch_issue_states_by_ids(&["1".to_string(), "3".to_string()])
        .await
        .unwrap();

    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].id, "1");
    assert_eq!(issues[1].id, "3");
}

#[tokio::test]
async fn fetch_issue_states_by_ids_with_unknown_ids_returns_empty() {
    let tracker = MemoryTracker::new(vec![test_issue("1", "ISSUE-1", "First", "open", Some(1))]);

    let issues = tracker
        .fetch_issue_states_by_ids(&["unknown".to_string()])
        .await
        .unwrap();

    assert!(issues.is_empty());
}

#[tokio::test]
async fn fetch_issues_by_states_filters_case_insensitively() {
    let tracker = MemoryTracker::new(vec![
        test_issue("1", "ISSUE-1", "First", "Open", Some(1)),
        test_issue("2", "ISSUE-2", "Second", "CLOSED", Some(2)),
        test_issue("3", "ISSUE-3", "Third", "in_review", Some(3)),
    ]);

    let issues = tracker
        .fetch_issues_by_states(
            &["open".to_string(), "closed".to_string()],
            &TrackerConfig::default(),
        )
        .await
        .unwrap();

    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].id, "1");
    assert_eq!(issues[1].id, "2");
}

#[tokio::test]
async fn update_issue_state_changes_future_fetch_results() {
    let tracker = MemoryTracker::new(vec![test_issue("1", "ISSUE-1", "First", "open", Some(1))]);

    tracker.update_issue_state("1", "closed");

    let active = tracker
        .fetch_candidate_issues(&tracker_config(&["open"]))
        .await
        .unwrap();
    let closed = tracker
        .fetch_issues_by_states(&["closed".to_string()], &TrackerConfig::default())
        .await
        .unwrap();

    assert!(active.is_empty());
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].state, "closed");
}

#[test]
fn test_issue_builds_expected_defaults() {
    let issue = test_issue("1", "ISSUE-1", "First", "open", Some(5));

    assert_eq!(issue.id, "1");
    assert_eq!(issue.identifier, "ISSUE-1");
    assert_eq!(issue.title, "First");
    assert_eq!(issue.priority, Some(5));
    assert_eq!(issue.state, "open");
    assert_eq!(issue.description, None);
    assert_eq!(issue.branch_name, None);
    assert_eq!(issue.url, None);
    assert!(issue.labels.is_empty());
    assert!(issue.blocked_by.is_empty());
    assert_eq!(issue.created_at, None);
    assert_eq!(issue.updated_at, None);
}
