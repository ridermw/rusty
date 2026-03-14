use std::path::Path;
use std::sync::Arc;

use rusty::agent::{run_agent_attempt, WorkerResult};
use rusty::config::schema::RustyConfig;
use rusty::session::{SessionRecord, SessionStore};
use rusty::tracker::memory::{test_issue, MemoryTracker};
use rusty::tracker::Tracker;
use rusty::workspace::hooks::ShellExecutor;
use rusty::workspace::WorkspaceError;
use tempfile::tempdir;
use tokio::sync::mpsc;

struct NoopExecutor;

impl ShellExecutor for NoopExecutor {
    fn execute(
        &self,
        _script: &str,
        _cwd: &Path,
        _timeout: std::time::Duration,
    ) -> Result<(), WorkspaceError> {
        Ok(())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ── Fake ACP server that supports session/load ────────────────────────

fn write_fake_acp_server_with_load(script_path: &Path) {
    let script = r#"import json
import sys

session_id = "session-1"
turn_id = "turn-1"

for raw in sys.stdin:
    raw = raw.strip()
    if not raw:
        continue

    message = json.loads(raw)
    method = message.get("method")
    message_id = message.get("id")

    if method == "initialize":
        sys.stdout.write(json.dumps({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {
                "protocolVersion": 1,
                "capabilities": {
                    "loadSession": True,
                    "sessionCapabilities": ["list"]
                }
            }
        }) + "\n")
        sys.stdout.flush()
    elif method == "initialized":
        continue
    elif method == "session/load":
        params = message.get("params", {})
        loaded_id = params.get("sessionId", session_id)
        sys.stdout.write(json.dumps({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {"sessionId": loaded_id}
        }) + "\n")
        sys.stdout.flush()
    elif method == "session/new":
        sys.stdout.write(json.dumps({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {"sessionId": session_id}
        }) + "\n")
        sys.stdout.flush()
    elif method == "session/prompt":
        sys.stdout.write(json.dumps({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {"turn": {"id": turn_id}}
        }) + "\n")
        sys.stdout.write(json.dumps({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {"status": "completed", "turnId": turn_id}
        }) + "\n")
        sys.stdout.flush()
    else:
        sys.stdout.write(json.dumps({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {}
        }) + "\n")
        sys.stdout.flush()
"#;
    std::fs::write(script_path, script).expect("write fake ACP server script");
}

fn write_fake_acp_server_load_fails(script_path: &Path) {
    let script = r#"import json
import sys

session_id = "session-new-1"
turn_id = "turn-1"

for raw in sys.stdin:
    raw = raw.strip()
    if not raw:
        continue

    message = json.loads(raw)
    method = message.get("method")
    message_id = message.get("id")

    if method == "initialize":
        sys.stdout.write(json.dumps({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {"protocolVersion": 1, "capabilities": {}}
        }) + "\n")
        sys.stdout.flush()
    elif method == "initialized":
        continue
    elif method == "session/load":
        sys.stdout.write(json.dumps({
            "jsonrpc": "2.0",
            "id": message_id,
            "error": {"code": -32600, "message": "session not found or expired"}
        }) + "\n")
        sys.stdout.flush()
    elif method == "session/new":
        sys.stdout.write(json.dumps({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {"sessionId": session_id}
        }) + "\n")
        sys.stdout.flush()
    elif method == "session/prompt":
        sys.stdout.write(json.dumps({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {"turn": {"id": turn_id}}
        }) + "\n")
        sys.stdout.write(json.dumps({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {"status": "completed", "turnId": turn_id}
        }) + "\n")
        sys.stdout.flush()
    else:
        sys.stdout.write(json.dumps({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {}
        }) + "\n")
        sys.stdout.flush()
"#;
    std::fs::write(script_path, script).expect("write fake ACP server script");
}

fn fake_acp_command(script_path: &Path) -> String {
    let script = script_path.to_string_lossy().into_owned();
    let candidates: &[(&str, &[&str])] = if cfg!(windows) {
        &[("python", &[]), ("py", &["-3"]), ("py", &[])]
    } else {
        &[("python3", &[]), ("python", &[])]
    };

    for (command, args) in candidates {
        if std::process::Command::new(command)
            .args(*args)
            .arg("--version")
            .output()
            .is_ok()
        {
            let mut parts = vec![(*command).to_string()];
            parts.extend(args.iter().map(|arg| (*arg).to_string()));
            parts.push(script.clone());
            return parts.join(" ");
        }
    }

    panic!("no Python interpreter available for fake ACP test server");
}

// ── Tests ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn session_load_resumes_existing_session() {
    let workspace_root = tempdir().expect("create temp dir");
    let script_path = workspace_root.path().join("fake_acp_load.py");
    write_fake_acp_server_with_load(&script_path);

    let issue = test_issue("42", "rusty-42", "Test issue", "open", Some(1));
    let (update_tx, mut update_rx) = mpsc::channel(32);
    let mut config = RustyConfig::default();
    config.agent.command = fake_acp_command(&script_path);
    config.agent.max_turns = 1;

    let result = run_agent_attempt(
        issue,
        None,
        config,
        "Test prompt".to_string(),
        workspace_root.path().to_path_buf(),
        Arc::new(NoopExecutor),
        update_tx,
        Some("prev-session-xyz".to_string()),
    )
    .await;

    assert!(
        matches!(result, WorkerResult::Completed),
        "expected Completed, got {result:?}"
    );

    // Verify session_started event was emitted with the loaded session ID
    let mut found_session = false;
    while let Ok(update) = update_rx.try_recv() {
        if update.event == "session_started" {
            // The fake server echoes back the loaded session ID
            assert_eq!(
                update.session_id.as_deref(),
                Some("prev-session-xyz"),
                "session should be the loaded one"
            );
            found_session = true;
        }
    }
    assert!(found_session, "should have emitted session_started event");
}

#[tokio::test]
async fn session_load_falls_back_to_new_on_failure() {
    let workspace_root = tempdir().expect("create temp dir");
    let script_path = workspace_root.path().join("fake_acp_fail_load.py");
    write_fake_acp_server_load_fails(&script_path);

    let issue = test_issue("42", "rusty-42", "Test issue", "open", Some(1));
    let (update_tx, mut update_rx) = mpsc::channel(32);
    let mut config = RustyConfig::default();
    config.agent.command = fake_acp_command(&script_path);
    config.agent.max_turns = 1;

    let result = run_agent_attempt(
        issue,
        None,
        config,
        "Test prompt".to_string(),
        workspace_root.path().to_path_buf(),
        Arc::new(NoopExecutor),
        update_tx,
        Some("expired-session-abc".to_string()),
    )
    .await;

    assert!(
        matches!(result, WorkerResult::Completed),
        "should complete via fallback to session/new, got {result:?}"
    );

    // Verify session_started event was emitted with the NEW session ID
    let mut found_session = false;
    while let Ok(update) = update_rx.try_recv() {
        if update.event == "session_started" {
            assert_eq!(
                update.session_id.as_deref(),
                Some("session-new-1"),
                "should have fallen back to session/new"
            );
            found_session = true;
        }
    }
    assert!(found_session, "should have emitted session_started event");
}

#[tokio::test]
async fn session_store_round_trip() {
    let dir = tempdir().unwrap();
    let store = SessionStore::new(dir.path());

    store
        .save(SessionRecord {
            issue_id: "42".to_string(),
            session_id: "sess-abc".to_string(),
            created_at: chrono::Utc::now(),
            workspace_path: Some("/tmp/ws".to_string()),
        })
        .unwrap();

    let loaded = store.load("42").unwrap();
    assert_eq!(loaded.session_id, "sess-abc");

    store.delete("42").unwrap();
    assert!(store.load("42").is_none());
}

#[tokio::test]
async fn memory_tracker_session_persistence() {
    let tracker = MemoryTracker::new(vec![]);

    // Save session
    tracker
        .save_session_id("42", "sess-123")
        .await
        .expect("save should succeed");

    // Load session
    let loaded = tracker
        .load_session_id("42")
        .await
        .expect("load should succeed");
    assert_eq!(loaded.as_deref(), Some("sess-123"));

    // Delete session
    tracker
        .delete_session_id("42")
        .await
        .expect("delete should succeed");

    let loaded = tracker
        .load_session_id("42")
        .await
        .expect("load should succeed");
    assert!(loaded.is_none());
}

#[tokio::test]
async fn session_marker_extraction() {
    use rusty::tracker::github::adapter::extract_session_marker;

    assert_eq!(
        extract_session_marker("<!-- rusty:session:sess-abc123 -->"),
        Some("sess-abc123")
    );
    assert_eq!(
        extract_session_marker("  <!-- rusty:session:sess-abc123 -->  "),
        Some("sess-abc123")
    );
    assert!(extract_session_marker("some random comment").is_none());
    assert!(extract_session_marker("").is_none());
    assert!(extract_session_marker("<!-- other:tag -->").is_none());
}
