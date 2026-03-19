use chrono::{TimeZone, Utc};
use rusty::config::schema::{RustyConfig, TrackerConfig};
use rusty::orchestrator::state::{OrchestratorState, RetryEntry, RunningEntry, TokenTotals};
use rusty::orchestrator::{
    add_runtime_seconds, apply_token_update, build_snapshot, calculate_backoff, compose_session_id,
    detect_stalled, is_eligible, next_attempt, reconcile_against_tracker, should_warn_retry,
    sort_for_dispatch, ReconcileAction,
};
use rusty::tracker::{BlockerRef, Issue};

fn test_config() -> RustyConfig {
    RustyConfig {
        tracker: TrackerConfig {
            active_states: vec!["open".into(), "todo".into(), "in progress".into()],
            terminal_states: vec!["closed".into(), "done".into()],
            ..Default::default()
        },
        ..Default::default()
    }
}

fn make_issue(id: &str, ident: &str, state: &str, priority: Option<i32>) -> Issue {
    Issue {
        id: id.into(),
        identifier: ident.into(),
        title: format!("Issue {ident}"),
        description: None,
        priority,
        state: state.into(),
        branch_name: None,
        url: None,
        labels: vec![],
        blocked_by: vec![],
        created_at: None,
        updated_at: None,
    }
}

fn at(year: i32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, hour, min, sec)
        .single()
        .expect("valid timestamp")
}

fn make_running_entry(issue: Issue) -> RunningEntry {
    make_running_entry_with_activity(
        issue,
        at(2024, 1, 2, 3, 0, 0),
        Some(at(2024, 1, 2, 3, 4, 5)),
    )
}

fn make_running_entry_with_activity(
    issue: Issue,
    started_at: chrono::DateTime<Utc>,
    last_event_at: Option<chrono::DateTime<Utc>>,
) -> RunningEntry {
    let task = tokio::spawn(async {});

    RunningEntry {
        issue_id: issue.id.clone(),
        identifier: issue.identifier.clone(),
        issue,
        pid: None,
        session_id: Some("session-1".into()),
        last_event: Some("running".into()),
        last_event_at,
        last_message: Some("worker active".into()),
        input_tokens: 10,
        output_tokens: 20,
        total_tokens: 30,
        last_reported_input: 8,
        last_reported_output: 16,
        last_reported_total: 24,
        turn_count: 2,
        retry_attempt: Some(1),
        started_at,
        worker_handle: task.abort_handle(),
    }
}

#[test]
fn is_eligible_returns_true_for_active_state_issue_with_available_slots() {
    let state = OrchestratorState::new(1_000, 2);
    let issue = make_issue("1", "ISSUE-1", "open", Some(1));

    assert!(is_eligible(&issue, &state, &test_config()));
}

#[test]
fn is_eligible_returns_false_for_terminal_state_issue() {
    let state = OrchestratorState::new(1_000, 2);
    let issue = make_issue("1", "ISSUE-1", "done", Some(1));

    assert!(!is_eligible(&issue, &state, &test_config()));
}

#[tokio::test]
async fn is_eligible_returns_false_for_already_running_issue() {
    let mut state = OrchestratorState::new(1_000, 2);
    let issue = make_issue("1", "ISSUE-1", "open", Some(1));
    state
        .running
        .insert(issue.id.clone(), make_running_entry(issue.clone()));

    assert!(!is_eligible(&issue, &state, &test_config()));
}

#[test]
fn is_eligible_returns_false_for_already_claimed_issue() {
    let mut state = OrchestratorState::new(1_000, 2);
    let issue = make_issue("1", "ISSUE-1", "open", Some(1));
    state.claimed.insert(issue.id.clone());

    assert!(!is_eligible(&issue, &state, &test_config()));
}

#[test]
fn is_eligible_returns_false_for_already_completed_issue() {
    let mut state = OrchestratorState::new(1_000, 2);
    let issue = make_issue("1", "ISSUE-1", "open", Some(1));
    state.completed.insert(issue.id.clone());

    assert!(!is_eligible(&issue, &state, &test_config()));
}

#[test]
fn is_eligible_returns_false_when_global_slots_exhausted() {
    let state = OrchestratorState::new(1_000, 0);
    let issue = make_issue("1", "ISSUE-1", "open", Some(1));

    assert!(!is_eligible(&issue, &state, &test_config()));
}

#[test]
fn is_eligible_returns_false_for_todo_issue_with_non_terminal_blocker() {
    let state = OrchestratorState::new(1_000, 2);
    let mut issue = make_issue("1", "ISSUE-1", "todo", Some(1));
    issue.blocked_by.push(BlockerRef {
        id: Some("2".into()),
        identifier: Some("ISSUE-2".into()),
        state: Some("open".into()),
    });

    assert!(!is_eligible(&issue, &state, &test_config()));
}

#[test]
fn is_eligible_returns_true_for_todo_issue_with_only_terminal_blockers() {
    let state = OrchestratorState::new(1_000, 2);
    let mut issue = make_issue("1", "ISSUE-1", "todo", Some(1));
    issue.blocked_by.push(BlockerRef {
        id: Some("2".into()),
        identifier: Some("ISSUE-2".into()),
        state: Some("done".into()),
    });
    issue.blocked_by.push(BlockerRef {
        id: Some("3".into()),
        identifier: Some("ISSUE-3".into()),
        state: Some("closed".into()),
    });

    assert!(is_eligible(&issue, &state, &test_config()));
}

#[test]
fn sort_for_dispatch_sorts_by_priority_ascending_with_null_priority_last() {
    let mut issues = vec![
        make_issue("1", "ISSUE-1", "open", Some(3)),
        make_issue("2", "ISSUE-2", "open", None),
        make_issue("3", "ISSUE-3", "open", Some(1)),
    ];

    sort_for_dispatch(&mut issues);

    let identifiers: Vec<_> = issues
        .iter()
        .map(|issue| issue.identifier.as_str())
        .collect();
    assert_eq!(identifiers, vec!["ISSUE-3", "ISSUE-1", "ISSUE-2"]);
}

#[test]
fn sort_for_dispatch_breaks_ties_by_created_at_then_identifier() {
    let mut first = make_issue("1", "ISSUE-B", "open", Some(1));
    first.created_at = Some(at(2024, 1, 2, 0, 0, 0));

    let mut second = make_issue("2", "ISSUE-A", "open", Some(1));
    second.created_at = Some(at(2024, 1, 1, 0, 0, 0));

    let mut third = make_issue("3", "ISSUE-C", "open", Some(1));
    third.created_at = Some(at(2024, 1, 1, 0, 0, 0));

    let mut issues = vec![first, third, second];
    sort_for_dispatch(&mut issues);

    let identifiers: Vec<_> = issues
        .iter()
        .map(|issue| issue.identifier.as_str())
        .collect();
    assert_eq!(identifiers, vec!["ISSUE-A", "ISSUE-C", "ISSUE-B"]);
}

#[tokio::test]
async fn build_snapshot_returns_correct_counts() {
    let mut state = OrchestratorState::new(1_000, 2);
    let issue = make_issue("1", "ISSUE-1", "open", Some(1));
    state
        .running
        .insert(issue.id.clone(), make_running_entry(issue.clone()));
    state.retry_attempts.insert(
        issue.id.clone(),
        RetryEntry {
            issue_id: issue.id.clone(),
            identifier: issue.identifier.clone(),
            attempt: 2,
            due_at: at(2024, 1, 2, 4, 0, 0),
            error: Some("transient failure".into()),
        },
    );
    state.agent_totals = TokenTotals {
        input_tokens: 100,
        output_tokens: 50,
        total_tokens: 150,
        seconds_running: 12.5,
    };

    let snapshot = build_snapshot(&state);

    assert_eq!(snapshot.running_count, 1);
    assert_eq!(snapshot.retrying_count, 1);
    assert_eq!(snapshot.running.len(), 1);
    assert_eq!(snapshot.retrying.len(), 1);
    assert_eq!(snapshot.agent_totals.input_tokens, 100);
    assert_eq!(snapshot.agent_totals.output_tokens, 50);
    assert_eq!(snapshot.agent_totals.total_tokens, 150);

    let running = snapshot
        .running
        .iter()
        .find(|entry| entry.issue_id == "1")
        .expect("running entry present");
    assert_eq!(running.identifier, "ISSUE-1");
    assert_eq!(running.state, "open");
    assert_eq!(running.turn_count, 2);

    let retry = snapshot
        .retrying
        .iter()
        .find(|entry| entry.issue_id == "1")
        .expect("retry entry present");
    assert_eq!(retry.attempt, 2);
    assert_eq!(retry.error.as_deref(), Some("transient failure"));
}

#[test]
fn build_snapshot_includes_retry_entries() {
    use chrono::Utc;

    let mut state = OrchestratorState::new(30_000, 10);
    state.retry_attempts.insert(
        "issue-1".to_string(),
        RetryEntry {
            issue_id: "issue-1".to_string(),
            identifier: "repo-1".to_string(),
            attempt: 3,
            due_at: Utc::now(),
            error: Some("timeout".to_string()),
        },
    );

    let snapshot = build_snapshot(&state);
    assert_eq!(snapshot.retrying_count, 1);
    assert_eq!(snapshot.retrying[0].attempt, 3);
    assert_eq!(snapshot.retrying[0].error, Some("timeout".to_string()));
}

#[tokio::test]
async fn orchestrator_state_running_count_by_state() {
    use chrono::Utc;
    use rusty::tracker::memory::test_issue;

    let mut state = OrchestratorState::new(30_000, 10);

    let issue1 = test_issue("1", "repo-1", "Task 1", "open", Some(1));
    state.running.insert(
        "1".to_string(),
        RunningEntry {
            issue_id: "1".into(),
            identifier: "repo-1".into(),
            issue: issue1,
            pid: None,
            session_id: None,
            last_event: None,
            last_event_at: None,
            last_message: None,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            last_reported_input: 0,
            last_reported_output: 0,
            last_reported_total: 0,
            turn_count: 0,
            retry_attempt: None,
            started_at: Utc::now(),
            worker_handle: tokio::spawn(async {}).abort_handle(),
        },
    );

    assert_eq!(state.running_count(), 1);
    assert_eq!(state.running_count_by_state("open"), 1);
    assert_eq!(state.running_count_by_state("closed"), 0);
    assert_eq!(state.available_global_slots(), 9);
}

#[tokio::test]
async fn per_state_concurrency_blocks_dispatch() {
    use chrono::Utc;
    use rusty::tracker::memory::test_issue;
    use std::collections::HashMap;

    let mut config = RustyConfig::default();
    config.tracker.active_states = vec!["open".to_string()];
    config.tracker.terminal_states = vec!["closed".to_string()];
    config.agent.max_concurrent_agents_by_state = HashMap::from([("open".to_string(), 1)]);

    let mut state = OrchestratorState::new(30_000, 10);
    let running_issue = test_issue("1", "repo-1", "Running", "open", Some(1));
    state.running.insert(
        "1".to_string(),
        RunningEntry {
            issue_id: "1".into(),
            identifier: "repo-1".into(),
            issue: running_issue,
            pid: None,
            session_id: None,
            last_event: None,
            last_event_at: None,
            last_message: None,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            last_reported_input: 0,
            last_reported_output: 0,
            last_reported_total: 0,
            turn_count: 0,
            retry_attempt: None,
            started_at: Utc::now(),
            worker_handle: tokio::spawn(async {}).abort_handle(),
        },
    );
    state.claimed.insert("1".to_string());

    let new_issue = test_issue("2", "repo-2", "Waiting", "open", Some(2));
    assert!(!is_eligible(&new_issue, &state, &config));
}

#[tokio::test]
async fn apply_token_update_first_call_uses_absolute_values_as_deltas() {
    let issue = make_issue("1", "ISSUE-1", "open", Some(1));
    let mut entry = make_running_entry(issue);
    let mut totals = TokenTotals::default();

    entry.input_tokens = 0;
    entry.output_tokens = 0;
    entry.total_tokens = 0;
    entry.last_reported_input = 0;
    entry.last_reported_output = 0;
    entry.last_reported_total = 0;

    apply_token_update(&mut entry, &mut totals, 10, 20, 30);

    assert_eq!(entry.input_tokens, 10);
    assert_eq!(entry.output_tokens, 20);
    assert_eq!(entry.total_tokens, 30);
    assert_eq!(totals.input_tokens, 10);
    assert_eq!(totals.output_tokens, 20);
    assert_eq!(totals.total_tokens, 30);
}

#[tokio::test]
async fn apply_token_update_second_call_only_adds_incremental_deltas() {
    let issue = make_issue("1", "ISSUE-1", "open", Some(1));
    let mut entry = make_running_entry(issue);
    let mut totals = TokenTotals::default();

    entry.input_tokens = 10;
    entry.output_tokens = 20;
    entry.total_tokens = 30;
    entry.last_reported_input = 10;
    entry.last_reported_output = 20;
    entry.last_reported_total = 30;

    apply_token_update(&mut entry, &mut totals, 15, 24, 39);

    assert_eq!(totals.input_tokens, 5);
    assert_eq!(totals.output_tokens, 4);
    assert_eq!(totals.total_tokens, 9);
    assert_eq!(entry.last_reported_input, 15);
    assert_eq!(entry.last_reported_output, 24);
    assert_eq!(entry.last_reported_total, 39);
}

#[tokio::test]
async fn apply_token_update_ignores_decreasing_values() {
    let issue = make_issue("1", "ISSUE-1", "open", Some(1));
    let mut entry = make_running_entry(issue);
    let mut totals = TokenTotals::default();

    entry.last_reported_input = 10;
    entry.last_reported_output = 20;
    entry.last_reported_total = 30;

    apply_token_update(&mut entry, &mut totals, 8, 18, 28);

    assert_eq!(totals.input_tokens, 0);
    assert_eq!(totals.output_tokens, 0);
    assert_eq!(totals.total_tokens, 0);
    assert_eq!(entry.input_tokens, 8);
    assert_eq!(entry.output_tokens, 18);
    assert_eq!(entry.total_tokens, 28);
}

#[tokio::test]
async fn add_runtime_seconds_adds_positive_elapsed_time() {
    let issue = make_issue("1", "ISSUE-1", "open", Some(1));
    let mut entry = make_running_entry(issue);
    let mut totals = TokenTotals::default();

    entry.started_at = Utc::now() - chrono::Duration::milliseconds(1500);

    add_runtime_seconds(&mut totals, &entry);

    assert!(totals.seconds_running > 0.0);
}

#[test]
fn compose_session_id_formats_thread_and_turn_ids() {
    assert_eq!(
        compose_session_id("thread-123", "turn-456"),
        "thread-123-turn-456"
    );
}

#[tokio::test]
async fn detect_stalled_returns_stalled_issue_ids_when_elapsed_exceeds_timeout() {
    let mut state = OrchestratorState::new(1_000, 2);
    let stalled_issue = make_issue("1", "ISSUE-1", "open", Some(1));
    let fresh_issue = make_issue("2", "ISSUE-2", "open", Some(2));
    let now = Utc::now();

    state.running.insert(
        stalled_issue.id.clone(),
        make_running_entry_with_activity(
            stalled_issue.clone(),
            now - chrono::Duration::minutes(10),
            Some(now - chrono::Duration::minutes(6)),
        ),
    );
    state.running.insert(
        fresh_issue.id.clone(),
        make_running_entry_with_activity(
            fresh_issue.clone(),
            now - chrono::Duration::minutes(3),
            Some(now - chrono::Duration::seconds(20)),
        ),
    );

    let stalled = detect_stalled(&state, 60_000);

    assert_eq!(stalled, vec![stalled_issue.id]);
}

#[tokio::test]
async fn detect_stalled_returns_empty_when_all_sessions_are_fresh() {
    let mut state = OrchestratorState::new(1_000, 2);
    let issue = make_issue("1", "ISSUE-1", "open", Some(1));
    let now = Utc::now();

    state.running.insert(
        issue.id.clone(),
        make_running_entry_with_activity(
            issue,
            now - chrono::Duration::seconds(30),
            Some(now - chrono::Duration::seconds(10)),
        ),
    );

    assert!(detect_stalled(&state, 60_000).is_empty());
}

#[tokio::test]
async fn detect_stalled_returns_empty_when_timeout_is_disabled() {
    let mut state = OrchestratorState::new(1_000, 2);
    let issue = make_issue("1", "ISSUE-1", "open", Some(1));
    let now = Utc::now();

    state.running.insert(
        issue.id.clone(),
        make_running_entry_with_activity(
            issue,
            now - chrono::Duration::minutes(10),
            Some(now - chrono::Duration::minutes(5)),
        ),
    );

    assert!(detect_stalled(&state, 0).is_empty());
}

#[test]
fn reconcile_against_tracker_returns_stop_and_cleanup_for_terminal_issues() {
    let running_ids = vec!["1".to_string()];
    let refreshed = vec![make_issue("1", "ISSUE-1", "done", Some(1))];

    let actions = reconcile_against_tracker(
        &running_ids,
        &refreshed,
        &["closed".into(), "done".into()],
        &["open".into(), "todo".into()],
    );

    assert_eq!(actions.len(), 1);
    match &actions[0] {
        ReconcileAction::StopAndCleanup(issue_id) => assert_eq!(issue_id, "1"),
        other => panic!("expected StopAndCleanup, got {other:?}"),
    }
}

#[test]
fn reconcile_against_tracker_returns_update_state_for_active_issues() {
    let running_ids = vec!["1".to_string()];
    let refreshed_issue = make_issue("1", "ISSUE-1", "open", Some(1));
    let refreshed = vec![refreshed_issue.clone()];

    let actions = reconcile_against_tracker(
        &running_ids,
        &refreshed,
        &["closed".into(), "done".into()],
        &["open".into(), "todo".into()],
    );

    assert_eq!(actions.len(), 1);
    match &actions[0] {
        ReconcileAction::UpdateState(issue_id, issue) => {
            assert_eq!(issue_id, "1");
            assert_eq!(issue.as_ref(), &refreshed_issue);
        }
        other => panic!("expected UpdateState, got {other:?}"),
    }
}

#[test]
fn reconcile_against_tracker_returns_stop_no_cleanup_for_non_active_non_terminal_issues() {
    let running_ids = vec!["1".to_string()];
    let refreshed = vec![make_issue("1", "ISSUE-1", "blocked", Some(1))];

    let actions = reconcile_against_tracker(
        &running_ids,
        &refreshed,
        &["closed".into(), "done".into()],
        &["open".into(), "todo".into()],
    );

    assert_eq!(actions.len(), 1);
    match &actions[0] {
        ReconcileAction::StopNoCleanup(issue_id) => assert_eq!(issue_id, "1"),
        other => panic!("expected StopNoCleanup, got {other:?}"),
    }
}

#[test]
fn reconcile_against_tracker_ignores_issues_not_in_running_list() {
    let running_ids = vec!["1".to_string()];
    let refreshed = vec![
        make_issue("1", "ISSUE-1", "open", Some(1)),
        make_issue("2", "ISSUE-2", "done", Some(2)),
    ];

    let actions = reconcile_against_tracker(
        &running_ids,
        &refreshed,
        &["closed".into(), "done".into()],
        &["open".into(), "todo".into()],
    );

    assert_eq!(actions.len(), 1);
    match &actions[0] {
        ReconcileAction::UpdateState(issue_id, issue) => {
            assert_eq!(issue_id, "1");
            assert_eq!(issue.id, "1");
        }
        other => panic!("expected UpdateState, got {other:?}"),
    }
}

#[test]
fn calculate_backoff_returns_fixed_delay_for_continuation_retries() {
    assert_eq!(calculate_backoff(7, 60_000, true), 1_000);
}

#[test]
fn calculate_backoff_returns_10_seconds_for_first_failure_attempt() {
    assert_eq!(calculate_backoff(1, 60_000, false), 10_000);
}

#[test]
fn calculate_backoff_returns_20_seconds_for_second_failure_attempt() {
    assert_eq!(calculate_backoff(2, 60_000, false), 20_000);
}

#[test]
fn calculate_backoff_returns_40_seconds_for_third_failure_attempt() {
    assert_eq!(calculate_backoff(3, 60_000, false), 40_000);
}

#[test]
fn calculate_backoff_caps_delay_at_max_backoff() {
    assert_eq!(calculate_backoff(5, 30_000, false), 30_000);
}

#[test]
fn calculate_backoff_increases_on_consecutive_continuations() {
    use rusty::orchestrator::{calculate_backoff, should_throttle_continuation};

    // First continuation: 1000ms (as before)
    assert_eq!(calculate_backoff(1, 300_000, true), 1000);

    // But if we track consecutive no-op completions, backoff should increase
    // after a threshold.
    let consecutive_completions = 4;
    assert!(should_throttle_continuation(consecutive_completions));
    assert_eq!(
        calculate_backoff(consecutive_completions, 300_000, false),
        80_000
    );
}

#[test]
fn should_throttle_continuation_after_threshold() {
    use rusty::orchestrator::should_throttle_continuation;

    // First few continuations are fine
    assert!(!should_throttle_continuation(1));
    assert!(!should_throttle_continuation(2));
    assert!(!should_throttle_continuation(3));

    // After 3 consecutive no-op completions without state change, throttle
    assert!(should_throttle_continuation(4));
    assert!(should_throttle_continuation(10));
}

#[test]
fn should_warn_retry_returns_true_for_warning_thresholds() {
    assert!(should_warn_retry(5));
    assert!(should_warn_retry(10));
    assert!(should_warn_retry(20));
}

#[test]
fn should_warn_retry_returns_false_for_non_warning_attempts() {
    for attempt in [1, 2, 3, 4, 6, 15] {
        assert!(
            !should_warn_retry(attempt),
            "unexpected warning at {attempt}"
        );
    }
}

#[test]
fn next_attempt_returns_one_for_normal_exits() {
    assert_eq!(next_attempt(None, true), 1);
    assert_eq!(next_attempt(Some(1), true), 1);
    assert_eq!(next_attempt(Some(4), true), 1);
}

#[test]
fn next_attempt_increments_for_failures() {
    assert_eq!(next_attempt(None, false), 1);
    assert_eq!(next_attempt(Some(1), false), 2);
    assert_eq!(next_attempt(Some(4), false), 5);
}

#[test]
fn should_stop_retrying_returns_false_within_limit() {
    use rusty::orchestrator::should_stop_retrying;

    assert!(!should_stop_retrying(1));
    assert!(!should_stop_retrying(10));
    assert!(!should_stop_retrying(20));
}

#[test]
fn should_stop_retrying_returns_true_after_max_failure_retries() {
    use rusty::orchestrator::{should_stop_retrying, MAX_FAILURE_RETRIES};

    assert!(should_stop_retrying(MAX_FAILURE_RETRIES + 1));
    assert!(should_stop_retrying(MAX_FAILURE_RETRIES + 10));
}

#[test]
fn max_failure_retries_is_reasonable_value() {
    use rusty::orchestrator::MAX_FAILURE_RETRIES;

    // Must be at least as high as the highest warn threshold (20)
    assert!(MAX_FAILURE_RETRIES >= 20);
    // Must not be excessively high (sanity bound)
    assert!(MAX_FAILURE_RETRIES <= 100);
}
