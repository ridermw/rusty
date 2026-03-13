//! Noop dispatch test — hello world.
//!
//! Validates that the orchestration pipeline can pick up a candidate issue,
//! determine eligibility, and sort it for dispatch without performing any
//! real work.  This is the simplest possible proof that the dispatch path
//! is wired correctly.
//!
//! Ref: https://github.com/ridermw/rusty/issues/39

use chrono::Utc;
use rusty::config::schema::*;
use rusty::orchestrator::{self, state::OrchestratorState};
use rusty::tracker::memory::MemoryTracker;
use rusty::tracker::{Issue, Tracker};

fn noop_config() -> RustyConfig {
    RustyConfig {
        tracker: TrackerConfig {
            kind: Some("github".to_string()),
            repo: Some("test/noop".to_string()),
            api_key: Some("noop-token".to_string()),
            active_states: vec!["todo".into()],
            terminal_states: vec!["done".into()],
            ..Default::default()
        },
        agent: AgentConfig {
            max_concurrent_agents: 1,
            max_turns: 1,
            command: "echo hello world".to_string(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn hello_world_issue() -> Issue {
    Issue {
        id: "39".into(),
        identifier: "noop-39".into(),
        title: "Test: noop dispatch - hello world".into(),
        description: Some("Noop test issue for verifying dispatch.".into()),
        priority: Some(0),
        state: "todo".into(),
        branch_name: None,
        url: Some("https://github.com/ridermw/rusty/issues/39".into()),
        labels: vec!["todo".into()],
        blocked_by: vec![],
        created_at: Some(Utc::now()),
        updated_at: Some(Utc::now()),
    }
}

#[tokio::test]
async fn noop_dispatch_hello_world() {
    let config = noop_config();
    let tracker = MemoryTracker::new(vec![hello_world_issue()]);

    // Fetch candidates — our single issue should appear.
    let candidates = tracker
        .fetch_candidate_issues(&config.tracker)
        .await
        .expect("fetch should succeed");
    assert_eq!(candidates.len(), 1, "expected exactly one candidate issue");
    assert_eq!(candidates[0].identifier, "noop-39");

    // Sort for dispatch — single-element sort is trivially stable.
    let mut sorted = candidates;
    orchestrator::sort_for_dispatch(&mut sorted);
    assert_eq!(sorted[0].identifier, "noop-39");

    // Eligibility check — fresh state, one slot, should be eligible.
    let state = OrchestratorState::new(30000, 1);
    assert!(
        orchestrator::is_eligible(&sorted[0], &state, &config),
        "noop issue should be eligible for dispatch"
    );
}
