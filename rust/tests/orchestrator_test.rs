use chrono::{TimeZone, Utc};
use symphony::config::schema::{SymphonyConfig, TrackerConfig};
use symphony::orchestrator::state::{OrchestratorState, RetryEntry, RunningEntry, TokenTotals};
use symphony::orchestrator::{build_snapshot, is_eligible, sort_for_dispatch};
use symphony::tracker::{BlockerRef, Issue};

fn test_config() -> SymphonyConfig {
    SymphonyConfig {
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
    let task = tokio::spawn(async {});

    RunningEntry {
        issue_id: issue.id.clone(),
        identifier: issue.identifier.clone(),
        issue,
        session_id: Some("session-1".into()),
        last_event: Some("running".into()),
        last_event_at: Some(at(2024, 1, 2, 3, 4, 5)),
        last_message: Some("worker active".into()),
        input_tokens: 10,
        output_tokens: 20,
        total_tokens: 30,
        last_reported_input: 8,
        last_reported_output: 16,
        last_reported_total: 24,
        turn_count: 2,
        retry_attempt: Some(1),
        started_at: at(2024, 1, 2, 3, 0, 0),
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
