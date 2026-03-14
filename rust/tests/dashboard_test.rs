use rusty::dashboard::{humanize_event, render_dashboard};
use rusty::orchestrator::state::TokenTotals;
use rusty::orchestrator::{OrchestratorSnapshot, RetrySnapshot, RunningSnapshot};

fn empty_snapshot() -> OrchestratorSnapshot {
    OrchestratorSnapshot {
        running_count: 0,
        retrying_count: 0,
        max_agents: 10,
        throughput_tps: 0.0,
        running: vec![],
        retrying: vec![],
        agent_totals: TokenTotals::default(),
        rate_limits: None,
        project_url: None,
        next_tick_at: None,
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
        max_agents: 10,
        throughput_tps: 2.4,
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
        rate_limits: None,
        project_url: None,
        next_tick_at: None,
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
        max_agents: 10,
        throughput_tps: 4.42,
        running: vec![],
        retrying: vec![],
        agent_totals: TokenTotals {
            input_tokens: 16,
            output_tokens: 26,
            total_tokens: 42,
            seconds_running: 9.5,
        },
        rate_limits: None,
        project_url: None,
        next_tick_at: None,
    };

    let output = render_dashboard(&snapshot);

    assert!(output.contains("Tokens: in 16 | out 26 | total 42"));
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
fn humanize_event_session_started_returns_started() {
    assert_eq!(humanize_event("session_started"), "Started");
}

#[test]
fn humanize_event_turn_failed_returns_turn_failed() {
    assert_eq!(humanize_event("turn_failed"), "Turn FAILED");
}

#[test]
fn humanize_event_turn_cancelled_returns_cancelled() {
    assert_eq!(humanize_event("turn_cancelled"), "Cancelled");
}

#[test]
fn humanize_event_notification_returns_working() {
    assert_eq!(humanize_event("notification"), "Working");
}

#[test]
fn humanize_event_approval_auto_approved_returns_auto_approved() {
    assert_eq!(humanize_event("approval_auto_approved"), "Auto-approved");
}

#[test]
fn render_dashboard_with_multiple_running_sessions() {
    let snapshot = OrchestratorSnapshot {
        running_count: 3,
        retrying_count: 0,
        max_agents: 10,
        throughput_tps: 1.73,
        running: vec![
            RunningSnapshot {
                issue_id: "1".into(),
                identifier: "ISSUE-1".into(),
                state: "running".into(),
                session_id: Some("s1".into()),
                turn_count: 2,
                last_event: Some("turn_completed".into()),
                last_message: Some("Making progress".into()),
                started_at: "2024-01-01T00:00:00Z".into(),
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
            },
            RunningSnapshot {
                issue_id: "2".into(),
                identifier: "ISSUE-2".into(),
                state: "running".into(),
                session_id: Some("s2".into()),
                turn_count: 5,
                last_event: Some("notification".into()),
                last_message: Some("Thinking".into()),
                started_at: "2024-01-01T00:01:00Z".into(),
                input_tokens: 20,
                output_tokens: 10,
                total_tokens: 30,
            },
            RunningSnapshot {
                issue_id: "3".into(),
                identifier: "ISSUE-3".into(),
                state: "running".into(),
                session_id: Some("s3".into()),
                turn_count: 1,
                last_event: Some("session_started".into()),
                last_message: Some("Starting".into()),
                started_at: "2024-01-01T00:02:00Z".into(),
                input_tokens: 5,
                output_tokens: 2,
                total_tokens: 7,
            },
        ],
        retrying: vec![],
        agent_totals: TokenTotals {
            input_tokens: 35,
            output_tokens: 17,
            total_tokens: 52,
            seconds_running: 30.0,
        },
        rate_limits: None,
        project_url: None,
        next_tick_at: None,
    };

    let output = render_dashboard(&snapshot);

    assert!(output.contains("Agents: 3/10"));
    assert!(output.contains("ISSUE-1"));
    assert!(output.contains("ISSUE-2"));
    assert!(output.contains("ISSUE-3"));
}

#[test]
fn render_dashboard_retry_entry_with_no_error() {
    let snapshot = OrchestratorSnapshot {
        running_count: 0,
        retrying_count: 1,
        max_agents: 10,
        throughput_tps: 0.0,
        running: vec![],
        retrying: vec![RetrySnapshot {
            issue_id: "1".into(),
            identifier: "ISSUE-1".into(),
            attempt: 3,
            due_at: "2024-06-01T12:00:00Z".into(),
            error: None,
        }],
        agent_totals: TokenTotals::default(),
        rate_limits: None,
        project_url: None,
        next_tick_at: None,
    };

    let output = render_dashboard(&snapshot);

    assert!(output.contains("── Retry Queue ──"));
    assert!(output.contains("ISSUE-1"));
    assert!(output.contains("attempt:3"));
    assert!(output.contains("-"));
}

#[test]
fn render_dashboard_running_entry_with_no_optional_fields() {
    let snapshot = OrchestratorSnapshot {
        running_count: 1,
        retrying_count: 0,
        max_agents: 10,
        throughput_tps: 0.0,
        running: vec![RunningSnapshot {
            issue_id: "1".into(),
            identifier: "ISSUE-4".into(),
            state: "starting".into(),
            session_id: None,
            turn_count: 0,
            last_event: None,
            last_message: None,
            started_at: "2024-01-01T00:00:00Z".into(),
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
        }],
        retrying: vec![],
        agent_totals: TokenTotals::default(),
        rate_limits: None,
        project_url: None,
        next_tick_at: None,
    };

    let output = render_dashboard(&snapshot);

    assert!(output.contains("ISSUE-4"));
    assert!(output.contains("[starting]"));
    assert!(output.contains("turns:0"));
}

#[test]
fn render_dashboard_truncates_long_messages_at_sixty_chars() {
    let long_message = format!("{}TRUNCATED", "a".repeat(60));
    let snapshot = OrchestratorSnapshot {
        running_count: 1,
        retrying_count: 0,
        max_agents: 10,
        throughput_tps: 3.0,
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
        rate_limits: None,
        project_url: None,
        next_tick_at: None,
    };

    let output = render_dashboard(&snapshot);

    assert!(output.contains(&"a".repeat(60)));
    assert!(!output.contains(&long_message));
}

#[test]
fn render_dashboard_shows_agents_ratio() {
    let snapshot = OrchestratorSnapshot {
        running_count: 3,
        retrying_count: 0,
        max_agents: 50,
        throughput_tps: 100.0,
        running: vec![],
        retrying: vec![],
        agent_totals: TokenTotals::default(),
        rate_limits: None,
        project_url: None,
        next_tick_at: None,
    };

    let output = render_dashboard(&snapshot);
    assert!(output.contains("Agents: 3/50"));
}

#[test]
fn render_dashboard_shows_throughput() {
    let snapshot = OrchestratorSnapshot {
        running_count: 0,
        retrying_count: 0,
        max_agents: 10,
        throughput_tps: 658.0,
        running: vec![],
        retrying: vec![],
        agent_totals: TokenTotals {
            input_tokens: 6580,
            output_tokens: 0,
            total_tokens: 6580,
            seconds_running: 10.0,
        },
        rate_limits: None,
        project_url: None,
        next_tick_at: None,
    };

    let output = render_dashboard(&snapshot);
    assert!(output.contains("Throughput: 658 tps"));
}

#[test]
fn render_dashboard_shows_project_url_when_set() {
    let snapshot = OrchestratorSnapshot {
        running_count: 0,
        retrying_count: 0,
        max_agents: 10,
        throughput_tps: 0.0,
        running: vec![],
        retrying: vec![],
        agent_totals: TokenTotals::default(),
        rate_limits: None,
        project_url: Some("https://github.com/orgs/test/projects/1".into()),
        next_tick_at: None,
    };

    let output = render_dashboard(&snapshot);
    assert!(output.contains("Project: https://github.com/orgs/test/projects/1"));
}

#[test]
fn render_dashboard_shows_rate_limits_na_when_none() {
    let snapshot = OrchestratorSnapshot {
        running_count: 0,
        retrying_count: 0,
        max_agents: 10,
        throughput_tps: 0.0,
        running: vec![],
        retrying: vec![],
        agent_totals: TokenTotals::default(),
        rate_limits: None,
        project_url: None,
        next_tick_at: None,
    };

    let output = render_dashboard(&snapshot);
    assert!(output.contains("Rate Limits: n/a"));
}

#[test]
fn render_dashboard_shows_next_refresh_when_set() {
    let snapshot = OrchestratorSnapshot {
        running_count: 0,
        retrying_count: 0,
        max_agents: 10,
        throughput_tps: 0.0,
        running: vec![],
        retrying: vec![],
        agent_totals: TokenTotals::default(),
        rate_limits: None,
        project_url: None,
        next_tick_at: Some("2026-03-14T10:01:50Z".into()),
    };

    let output = render_dashboard(&snapshot);
    assert!(output.contains("Next refresh: 2026-03-14T10:01:50Z"));
}
