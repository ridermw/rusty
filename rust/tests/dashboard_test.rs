use rusty::dashboard::{humanize_event, render_dashboard};
use rusty::orchestrator::state::TokenTotals;
use rusty::orchestrator::{OrchestratorSnapshot, RetrySnapshot, RunningSnapshot};

fn empty_snapshot() -> OrchestratorSnapshot {
    OrchestratorSnapshot {
        running_count: 0,
        retrying_count: 0,
        running: vec![],
        retrying: vec![],
        agent_totals: TokenTotals::default(),
    }
}

#[test]
fn render_dashboard_with_empty_snapshot_shows_no_running_sessions() {
    let output = render_dashboard(&empty_snapshot());

    assert!(output.contains("No running sessions."));
}

#[test]
fn render_dashboard_with_running_and_retry_sections_shows_both_sections() {
    let snapshot = OrchestratorSnapshot {
        running_count: 1,
        retrying_count: 1,
        running: vec![RunningSnapshot {
            issue_id: "1".into(),
            identifier: "ISSUE-1".into(),
            state: "running".into(),
            session_id: Some("session-1".into()),
            turn_count: 3,
            last_event: Some("turn_completed".into()),
            last_message: Some("Agent is making progress".into()),
            started_at: "2024-01-02T03:04:05Z".into(),
            input_tokens: 10,
            output_tokens: 20,
            total_tokens: 30,
        }],
        retrying: vec![RetrySnapshot {
            issue_id: "2".into(),
            identifier: "ISSUE-2".into(),
            attempt: 2,
            due_at: "2024-01-02T03:05:00Z".into(),
            error: Some("network timeout".into()),
        }],
        agent_totals: TokenTotals {
            input_tokens: 10,
            output_tokens: 20,
            total_tokens: 30,
            seconds_running: 12.5,
        },
    };

    let output = render_dashboard(&snapshot);

    assert!(output.contains("── Running ──"));
    assert!(output.contains("ISSUE-1"));
    assert!(output.contains("── Retry Queue ──"));
    assert!(output.contains("ISSUE-2"));
}

#[test]
fn render_dashboard_shows_correct_token_totals() {
    let snapshot = OrchestratorSnapshot {
        running_count: 0,
        retrying_count: 0,
        running: vec![],
        retrying: vec![],
        agent_totals: TokenTotals {
            input_tokens: 16,
            output_tokens: 26,
            total_tokens: 42,
            seconds_running: 9.5,
        },
    };

    let output = render_dashboard(&snapshot);

    assert!(output.contains("Tokens: 42 (in:16 out:26)"));
    assert!(output.contains("Runtime: 9.5s"));
}

#[test]
fn humanize_event_turn_completed_returns_turn_done() {
    assert_eq!(humanize_event("turn_completed"), "Turn done");
}

#[test]
fn humanize_event_unknown_value_returns_original_value() {
    assert_eq!(humanize_event("unknown_thing"), "unknown_thing");
}

#[test]
fn render_dashboard_truncates_long_messages_at_sixty_chars() {
    let long_message = format!("{}TRUNCATED", "a".repeat(60));
    let snapshot = OrchestratorSnapshot {
        running_count: 1,
        retrying_count: 0,
        running: vec![RunningSnapshot {
            issue_id: "1".into(),
            identifier: "ISSUE-3".into(),
            state: "running".into(),
            session_id: None,
            turn_count: 1,
            last_event: Some("notification".into()),
            last_message: Some(long_message.clone()),
            started_at: "2024-01-02T03:04:05Z".into(),
            input_tokens: 1,
            output_tokens: 2,
            total_tokens: 3,
        }],
        retrying: vec![],
        agent_totals: TokenTotals {
            input_tokens: 1,
            output_tokens: 2,
            total_tokens: 3,
            seconds_running: 1.0,
        },
    };

    let output = render_dashboard(&snapshot);

    assert!(output.contains(&"a".repeat(60)));
    assert!(!output.contains(&long_message));
}
