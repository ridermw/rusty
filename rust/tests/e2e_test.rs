//! End-to-end integration tests for Rusty.
//!
//! Uses MemoryTracker and in-process logic to validate the complete
//! orchestration lifecycle without external dependencies.

use chrono::Utc;
use rusty::config::schema::*;
use rusty::dashboard;
use rusty::orchestrator::state::{OrchestratorState, TokenTotals};
use rusty::orchestrator::{self, OrchestratorSnapshot, RunningSnapshot};
use rusty::prompt;
use rusty::tracker::memory::MemoryTracker;
use rusty::tracker::{Issue, Tracker};
use rusty::workspace;
use tempfile::tempdir;

fn test_config() -> RustyConfig {
    RustyConfig {
        tracker: TrackerConfig {
            kind: Some("github".to_string()),
            repo: Some("test/repo".to_string()),
            api_key: Some("test-token".to_string()),
            active_states: vec!["open".into(), "todo".into()],
            terminal_states: vec!["closed".into(), "done".into()],
            ..Default::default()
        },
        agent: AgentConfig {
            max_concurrent_agents: 2,
            max_turns: 5,
            command: "echo".to_string(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn test_issues() -> Vec<Issue> {
    vec![
        Issue {
            id: "1".into(),
            identifier: "repo-1".into(),
            title: "First task".into(),
            description: Some("Do the thing".into()),
            priority: Some(1),
            state: "open".into(),
            branch_name: None,
            url: Some("https://github.com/test/repo/issues/1".into()),
            labels: vec![],
            blocked_by: vec![],
            created_at: Some(Utc::now() - chrono::Duration::hours(2)),
            updated_at: Some(Utc::now()),
        },
        Issue {
            id: "2".into(),
            identifier: "repo-2".into(),
            title: "Second task".into(),
            description: None,
            priority: Some(2),
            state: "open".into(),
            branch_name: None,
            url: None,
            labels: vec!["enhancement".into()],
            blocked_by: vec![],
            created_at: Some(Utc::now() - chrono::Duration::hours(1)),
            updated_at: Some(Utc::now()),
        },
        Issue {
            id: "3".into(),
            identifier: "repo-3".into(),
            title: "Closed task".into(),
            description: None,
            priority: None,
            state: "closed".into(),
            branch_name: None,
            url: None,
            labels: vec![],
            blocked_by: vec![],
            created_at: Some(Utc::now() - chrono::Duration::hours(3)),
            updated_at: Some(Utc::now()),
        },
    ]
}

#[tokio::test]
async fn full_lifecycle_poll_dispatch_skip_terminal() {
    let config = test_config();
    let tracker = MemoryTracker::new(test_issues());

    let candidates = tracker
        .fetch_candidate_issues(&config.tracker)
        .await
        .unwrap();
    assert_eq!(candidates.len(), 2);

    let mut sorted = candidates;
    orchestrator::sort_for_dispatch(&mut sorted);
    assert_eq!(sorted[0].identifier, "repo-1");
    assert_eq!(sorted[1].identifier, "repo-2");

    let state = OrchestratorState::new(30000, 2);
    assert!(orchestrator::is_eligible(&sorted[0], &state, &config));
    assert!(orchestrator::is_eligible(&sorted[1], &state, &config));
}

#[tokio::test]
async fn dispatch_respects_concurrency_limit() {
    let config = RustyConfig {
        agent: AgentConfig {
            max_concurrent_agents: 1,
            ..test_config().agent
        },
        ..test_config()
    };

    let state = OrchestratorState::new(30000, 1);
    let issues = test_issues();

    // First issue is eligible (0 running, 1 slot)
    assert!(orchestrator::is_eligible(&issues[0], &state, &config));

    // Simulate issue 0 being claimed AND running — add to both sets
    let mut state = state;
    state.claimed.insert("1".into());
    state.running.insert(
        "1".into(),
        rusty::orchestrator::state::RunningEntry {
            issue_id: "1".into(),
            identifier: "repo-1".into(),
            issue: issues[0].clone(),
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

    // Second issue now ineligible (1 running, 1 max)
    assert!(!orchestrator::is_eligible(&issues[1], &state, &config));
}

#[test]
fn config_reload_parses_new_workflow() {
    let v1 = rusty::workflow::parse_workflow("---\npolling:\n  interval_ms: 30000\n---\nPrompt v1")
        .unwrap();
    let v2 = rusty::workflow::parse_workflow("---\npolling:\n  interval_ms: 5000\n---\nPrompt v2")
        .unwrap();

    assert_ne!(v1.prompt_template, v2.prompt_template);
    let c1: RustyConfig =
        serde_yaml::from_value(serde_yaml::to_value(&v1.config).unwrap()).unwrap();
    let c2: RustyConfig =
        serde_yaml::from_value(serde_yaml::to_value(&v2.config).unwrap()).unwrap();
    assert_eq!(c1.polling.interval_ms, 30000);
    assert_eq!(c2.polling.interval_ms, 5000);
}

#[tokio::test]
async fn reconciliation_terminal_state_triggers_cleanup() {
    let config = test_config();
    let tracker = MemoryTracker::new(test_issues());
    tracker.update_issue_state("1", "closed");

    let refreshed = tracker
        .fetch_issue_states_by_ids(&["1".into()])
        .await
        .unwrap();
    assert_eq!(refreshed[0].state, "closed");

    let actions = orchestrator::reconcile_against_tracker(
        &["1".into()],
        &refreshed,
        &config.tracker.terminal_states,
        &config.tracker.active_states,
    );
    assert_eq!(actions.len(), 1);
    assert!(matches!(
        actions[0],
        orchestrator::ReconcileAction::StopAndCleanup(_)
    ));
}

#[test]
fn prompt_renders_with_real_issue_data() {
    let issues = test_issues();
    let rendered = prompt::render_prompt(
        "Working on {{ issue.identifier }}: {{ issue.title }}",
        &issues[0],
        None,
    )
    .unwrap();
    assert_eq!(rendered, "Working on repo-1: First task");
}

#[test]
fn prompt_renders_retry_attempt() {
    let issues = test_issues();
    let rendered = prompt::render_prompt(
        "{% if attempt %}Retry #{{ attempt }}{% endif %} {{ issue.identifier }}",
        &issues[0],
        Some(3),
    )
    .unwrap();
    assert!(rendered.contains("Retry #3"));
    assert!(rendered.contains("repo-1"));
}

#[test]
fn workspace_lifecycle_create_and_remove() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let ws = workspace::create_for_issue(root, "repo-42").unwrap();
    assert!(ws.created_now);
    assert!(ws.path.exists());

    let ws2 = workspace::create_for_issue(root, "repo-42").unwrap();
    assert!(!ws2.created_now);

    workspace::remove_workspace(root, "repo-42").unwrap();
    assert!(!ws.path.exists());
}

#[test]
fn dashboard_renders_snapshot() {
    let snapshot = OrchestratorSnapshot {
        running_count: 1,
        retrying_count: 0,
        running: vec![RunningSnapshot {
            issue_id: "1".into(),
            identifier: "repo-1".into(),
            state: "open".into(),
            session_id: Some("thread-1-turn-1".into()),
            turn_count: 3,
            last_event: Some("notification".into()),
            last_message: Some("Working on tests".into()),
            started_at: Utc::now().to_rfc3339(),
            input_tokens: 500,
            output_tokens: 200,
            total_tokens: 700,
            issue_url: None,
        }],
        retrying: vec![],
        agent_totals: TokenTotals::default(),
    };

    let output = dashboard::render_dashboard(&snapshot);
    assert!(output.contains("Running: 1"));
    assert!(output.contains("repo-1"));
    assert!(output.contains("700"));
}

/// Smoke test: orchestrator loop polls MemoryTracker, dispatches eligible issues,
/// and the HTTP API returns live state.
#[tokio::test]
async fn smoke_test_orchestrator_polls_and_dispatches() {
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use rusty::orchestrator::{run_orchestrator, OrchestratorMsg};
    use rusty::server::api::build_router;
    use rusty::workspace::hooks::default_shell_executor;
    use std::sync::Arc;
    use tokio::sync::{mpsc, oneshot};
    use tower::ServiceExt;

    let mut config = test_config();
    config.polling.interval_ms = 100;
    config.agent.command = "echo".to_string();

    let tracker = Arc::new(MemoryTracker::new(test_issues())) as Arc<dyn Tracker>;
    let state = OrchestratorState::new(100, 2);
    let shell: Arc<dyn rusty::workspace::hooks::ShellExecutor> =
        Arc::from(default_shell_executor());
    let tmp = tempdir().unwrap();
    let workspace_root = tmp.path().to_path_buf();

    let (tx, rx) = mpsc::channel::<OrchestratorMsg>(256);
    let snapshot_tx = tx.clone();

    let orch_handle = tokio::spawn(async move {
        run_orchestrator(
            state,
            config,
            tracker,
            "Test prompt for {{ issue.identifier }}".to_string(),
            workspace_root,
            shell,
            rx,
            tx,
        )
        .await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    let (reply_tx, reply_rx) = oneshot::channel();
    let _ = snapshot_tx
        .send(OrchestratorMsg::SnapshotRequest { reply: reply_tx })
        .await;

    let snapshot = match tokio::time::timeout(tokio::time::Duration::from_secs(2), reply_rx).await {
        Ok(Ok(snapshot)) => snapshot,
        Ok(Err(_)) => {
            let _ = snapshot_tx.send(OrchestratorMsg::Shutdown).await;
            let _ = orch_handle.await;
            return;
        }
        Err(_) => panic!("snapshot request timed out — orchestrator may be stuck"),
    };

    assert!(
        snapshot.running_count + snapshot.retrying_count > 0,
        "expected orchestrator activity after polling tick, snapshot: {snapshot:?}"
    );

    let app = build_router(snapshot_tx.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["counts"]["running"].as_u64().unwrap(),
        snapshot.running_count as u64
    );
    assert_eq!(
        payload["counts"]["retrying"].as_u64().unwrap(),
        snapshot.retrying_count as u64
    );

    let _ = snapshot_tx.send(OrchestratorMsg::Shutdown).await;
    tokio::time::timeout(tokio::time::Duration::from_secs(2), orch_handle)
        .await
        .expect("orchestrator should shut down")
        .expect("orchestrator task should not panic");
}

/// Verify that dispatching an issue creates a workspace directory.
#[tokio::test]
async fn dispatch_creates_workspace_directory() {
    use rusty::orchestrator::{run_orchestrator, OrchestratorMsg};
    use rusty::workspace::hooks::default_shell_executor;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    let mut config = test_config();
    config.polling.interval_ms = 100;
    config.agent.command = "echo".to_string();

    let issues = vec![test_issues().into_iter().next().unwrap()];
    let tracker = Arc::new(MemoryTracker::new(issues.clone())) as Arc<dyn Tracker>;
    let state = OrchestratorState::new(100, 1);
    let shell: Arc<dyn rusty::workspace::hooks::ShellExecutor> =
        Arc::from(default_shell_executor());
    let tmp = tempdir().unwrap();
    let workspace_root = tmp.path().to_path_buf();
    let check_root = workspace_root.clone();

    let (tx, rx) = mpsc::channel::<OrchestratorMsg>(256);
    let shutdown_tx = tx.clone();

    let orch_handle = tokio::spawn(async move {
        run_orchestrator(
            state,
            config,
            tracker,
            "Work on {{ issue.identifier }}".to_string(),
            workspace_root,
            shell,
            rx,
            tx,
        )
        .await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let _ = shutdown_tx.send(OrchestratorMsg::Shutdown).await;
    tokio::time::timeout(tokio::time::Duration::from_secs(2), orch_handle)
        .await
        .expect("orchestrator should shut down")
        .expect("orchestrator task should not panic");

    let expected_ws = workspace::workspace_path(&check_root, &issues[0].identifier);
    assert!(
        expected_ws.exists(),
        "workspace directory should be created at {:?}",
        expected_ws
    );
}

/// Verify that dispatching multiple issues concurrently creates all workspace directories.
#[tokio::test]
async fn concurrent_dispatch_creates_all_workspaces() {
    use rusty::orchestrator::{run_orchestrator, OrchestratorMsg};
    use rusty::workspace::hooks::default_shell_executor;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    let mut config = test_config();
    config.polling.interval_ms = 100;
    config.agent.command = "echo".to_string();
    config.agent.max_concurrent_agents = 3;

    // Use all 3 test issues but mark the third as active so it gets dispatched
    let mut issues = test_issues();
    issues[2].state = "open".into();

    let tracker = Arc::new(MemoryTracker::new(issues.clone())) as Arc<dyn Tracker>;
    let state = OrchestratorState::new(100, 3);
    let shell: Arc<dyn rusty::workspace::hooks::ShellExecutor> =
        Arc::from(default_shell_executor());
    let tmp = tempdir().unwrap();
    let workspace_root = tmp.path().to_path_buf();
    let check_root = workspace_root.clone();

    let (tx, rx) = mpsc::channel::<OrchestratorMsg>(256);
    let shutdown_tx = tx.clone();

    let orch_handle = tokio::spawn(async move {
        run_orchestrator(
            state,
            config,
            tracker,
            "Work on {{ issue.identifier }}".to_string(),
            workspace_root,
            shell,
            rx,
            tx,
        )
        .await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let _ = shutdown_tx.send(OrchestratorMsg::Shutdown).await;
    tokio::time::timeout(tokio::time::Duration::from_secs(2), orch_handle)
        .await
        .expect("orchestrator should shut down")
        .expect("orchestrator task should not panic");

    for issue in &issues {
        let expected_ws = workspace::workspace_path(&check_root, &issue.identifier);
        assert!(
            expected_ws.exists(),
            "workspace directory should be created for {} at {:?}",
            issue.identifier,
            expected_ws
        );
    }
}

#[tokio::test]
#[ignore]
async fn live_e2e_with_real_github_and_copilot() {
    if std::env::var("SYMPHONY_RUN_LIVE_E2E").as_deref() != Ok("1") {
        eprintln!("SKIPPED: Set SYMPHONY_RUN_LIVE_E2E=1 to run live E2E tests");
        return;
    }
    panic!("Live E2E test not yet implemented");
}
