use std::path::Path;
use std::time::Duration;

use rusty::agent::acp_client::{
    classify_event, extract_token_usage, AgentError, AgentEvent, ChildGuard, JsonRpcMessage,
    JsonRpcRequest, TurnResult,
};
use serde_json::json;
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

fn notification(method: &str, params: serde_json::Value) -> JsonRpcMessage {
    JsonRpcMessage {
        jsonrpc: Some("2.0".to_string()),
        id: None,
        method: Some(method.to_string()),
        result: None,
        error: None,
        params: Some(params),
    }
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
    match rusty::agent::AcpClient::launch(
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

#[test]
fn classify_event_turn_completed_returns_completed() {
    let event = classify_event(&notification("turn/completed", json!({})));
    assert!(matches!(event, AgentEvent::TurnCompleted));
}

#[test]
fn classify_event_turn_failed_returns_reason() {
    let event = classify_event(&notification(
        "turn/failed",
        json!({ "message": "model execution failed" }),
    ));

    match event {
        AgentEvent::TurnFailed(reason) => assert!(reason.contains("model execution failed")),
        other => panic!("expected TurnFailed, got {other:?}"),
    }
}

#[test]
fn classify_event_session_message_completed_returns_completed() {
    let event = classify_event(&notification("session/message/completed", json!({})));
    assert!(matches!(event, AgentEvent::TurnCompleted));
}

#[test]
fn classify_event_unknown_method_returns_notification() {
    let event = classify_event(&notification(
        "session/progress",
        json!({ "message": "still working" }),
    ));

    match event {
        AgentEvent::Notification { message } => assert_eq!(message, "still working"),
        other => panic!("expected Notification, got {other:?}"),
    }
}

#[test]
fn classify_event_approval_required_returns_payload() {
    let payload = json!({ "id": "approval-123", "kind": "tool" });
    let event = classify_event(&notification("session/approvalRequired", payload.clone()));

    match event {
        AgentEvent::ApprovalRequired(value) => assert_eq!(value, payload),
        other => panic!("expected ApprovalRequired, got {other:?}"),
    }
}

#[test]
fn classify_event_token_usage_returns_counts() {
    let event = classify_event(&notification(
        "session/tokenUsage",
        json!({
            "input_tokens": 11,
            "output_tokens": 7,
            "total_tokens": 18
        }),
    ));

    match event {
        AgentEvent::TokenUsage {
            input,
            output,
            total,
        } => {
            assert_eq!(input, 11);
            assert_eq!(output, 7);
            assert_eq!(total, 18);
        }
        other => panic!("expected TokenUsage, got {other:?}"),
    }
}

#[test]
fn extract_token_usage_reads_nested_usage_object() {
    let msg = notification(
        "thread/tokenUsage/updated",
        json!({
            "usage": {
                "input_tokens": 3,
                "output_tokens": 5,
                "total_tokens": 8
            }
        }),
    );

    assert_eq!(extract_token_usage(&msg), (3, 5, 8));
}

#[test]
fn extract_token_usage_reads_camel_case_top_level() {
    let msg = notification(
        "session/tokenUsage",
        json!({
            "inputTokens": 100,
            "outputTokens": 50,
            "totalTokens": 150
        }),
    );

    assert_eq!(extract_token_usage(&msg), (100, 50, 150));
}

#[test]
fn extract_token_usage_reads_camel_case_nested_token_usage() {
    let msg = notification(
        "thread/tokenUsage/updated",
        json!({
            "tokenUsage": {
                "total": {
                    "inputTokens": 200,
                    "outputTokens": 80,
                    "totalTokens": 280
                }
            }
        }),
    );

    assert_eq!(extract_token_usage(&msg), (200, 80, 280));
}

#[test]
fn turn_result_completed_pattern_matches() {
    let result = TurnResult::Completed {
        turn_id: "turn-42".to_string(),
    };

    match result {
        TurnResult::Completed { turn_id } => assert_eq!(turn_id, "turn-42"),
        other => panic!("expected Completed result, got {other:?}"),
    }
}
