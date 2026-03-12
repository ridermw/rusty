use std::path::Path;
use std::time::Duration;

use serde_json::json;
use symphony::agent::acp_client::{AgentError, ChildGuard, JsonRpcMessage, JsonRpcRequest};
use tokio::process::Command;
use tokio::time::sleep;

#[cfg(windows)]
fn sleep_process_command() -> (&'static str, Vec<&'static str>) {
    (
        "powershell",
        vec!["-NoProfile", "-Command", "Start-Sleep -Seconds 30"],
    )
}

#[cfg(not(windows))]
fn sleep_process_command() -> (&'static str, Vec<&'static str>) {
    ("sleep", vec!["30"])
}

#[cfg(windows)]
async fn process_exists(pid: u32) -> bool {
    let script = format!(
        "$p = Get-Process -Id {} -ErrorAction SilentlyContinue; if ($p) {{ 'true' }} else {{ 'false' }}",
        pid
    );

    let output = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .await
        .expect("process lookup should run");

    String::from_utf8_lossy(&output.stdout).trim() == "true"
}

#[cfg(not(windows))]
async fn process_exists(pid: u32) -> bool {
    let status = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .await
        .expect("process lookup should run");

    status.success()
}

#[tokio::test]
async fn child_guard_drop_kills_process() {
    let (command, args) = sleep_process_command();
    let child = Command::new(command)
        .args(args)
        .spawn()
        .expect("sleep process should spawn");
    let pid = child.id().expect("child should have pid");

    let guard = ChildGuard::new(child);
    drop(guard);

    for _ in 0..50 {
        if !process_exists(pid).await {
            return;
        }
        sleep(Duration::from_millis(100)).await;
    }

    panic!("child process {pid} was still running after guard drop");
}

#[test]
fn json_rpc_request_serializes_with_id_and_params() {
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(7),
        method: "agent.initialize".to_string(),
        params: Some(json!({ "cwd": "Q:\\git\\rusty\\rust" })),
    };

    let value = serde_json::to_value(&request).expect("request should serialize");
    assert_eq!(
        value,
        json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "agent.initialize",
            "params": { "cwd": "Q:\\git\\rusty\\rust" }
        })
    );
}

#[test]
fn json_rpc_request_serializes_without_optional_fields() {
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: None,
        method: "agent.shutdown".to_string(),
        params: None,
    };

    let value = serde_json::to_value(&request).expect("request should serialize");
    assert_eq!(
        value,
        json!({
            "jsonrpc": "2.0",
            "method": "agent.shutdown"
        })
    );
}

#[test]
fn launch_returns_not_found_for_missing_binary() {
    match symphony::agent::AcpClient::launch(
        "definitely-not-a-real-binary-for-acp-tests",
        &[],
        Path::new("."),
    ) {
        Err(AgentError::NotFound(binary)) => {
            assert_eq!(binary, "definitely-not-a-real-binary-for-acp-tests")
        }
        Err(other) => panic!("expected NotFound error, got {other:?}"),
        Ok(_) => panic!("launch unexpectedly succeeded"),
    }
}

#[test]
fn json_rpc_message_parse_line_sanitizes_inputs() {
    let blank = JsonRpcMessage::parse_line("   \n\t  ").expect("blank lines should be ignored");
    assert!(blank.is_none());

    let response =
        JsonRpcMessage::parse_line("  {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}  ")
            .expect("valid response should parse")
            .expect("response should be present");
    assert_eq!(response.jsonrpc.as_deref(), Some("2.0"));
    assert_eq!(response.id, Some(json!(1)));
    assert_eq!(response.result, Some(json!({ "ok": true })));

    let notification = JsonRpcMessage::parse_line(
        "{\"jsonrpc\":\"2.0\",\"method\":\"agent/event\",\"params\":{\"kind\":\"progress\"}}",
    )
    .expect("notification should parse")
    .expect("notification should be present");
    assert_eq!(notification.method.as_deref(), Some("agent/event"));
    assert_eq!(notification.params, Some(json!({ "kind": "progress" })));

    let err = JsonRpcMessage::parse_line("not-json").expect_err("invalid JSON should error");
    assert!(matches!(err, AgentError::Json(_)));
}
