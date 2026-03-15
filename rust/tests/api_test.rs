use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use chrono::{TimeZone, Utc};
use rusty::orchestrator::state::{OrchestratorState, RunningEntry, TokenTotals};
use rusty::orchestrator::{build_snapshot, OrchestratorMsg, OrchestratorSnapshot};
use rusty::server::api::build_router;
use rusty::tracker::Issue;
use serde_json::Value;
use tokio::sync::mpsc;
use tower::ServiceExt; // for oneshot

fn make_issue(id: &str, identifier: &str, state: &str) -> Issue {
    Issue {
        id: id.into(),
        identifier: identifier.into(),
        title: format!("Issue {identifier}"),
        description: Some("Test issue".into()),
        priority: Some(1),
        state: state.into(),
        branch_name: Some("feat/test".into()),
        url: Some(format!("https://example.test/issues/{identifier}")),
        labels: vec!["test".into()],
        blocked_by: vec![],
        created_at: Some(
            Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0)
                .single()
                .expect("valid created_at"),
        ),
        updated_at: Some(
            Utc.with_ymd_and_hms(2024, 1, 1, 12, 30, 0)
                .single()
                .expect("valid updated_at"),
        ),
    }
}

fn make_snapshot() -> OrchestratorSnapshot {
    let issue = make_issue("1", "ISSUE-1", "open");
    let worker = tokio::spawn(async {});
    let mut state = OrchestratorState::new(1_000, 2);
    state.running.insert(
        issue.id.clone(),
        RunningEntry {
            issue_id: issue.id.clone(),
            identifier: issue.identifier.clone(),
            issue,
            pid: None,
            session_id: Some("session-1".into()),
            last_event: Some("running".into()),
            last_event_at: Some(
                Utc.with_ymd_and_hms(2024, 1, 1, 12, 5, 0)
                    .single()
                    .expect("valid last_event_at"),
            ),
            last_message: Some("worker active".into()),
            input_tokens: 11,
            output_tokens: 22,
            total_tokens: 33,
            last_reported_input: 9,
            last_reported_output: 18,
            last_reported_total: 27,
            turn_count: 3,
            retry_attempt: None,
            started_at: Utc
                .with_ymd_and_hms(2024, 1, 1, 12, 0, 0)
                .single()
                .expect("valid started_at"),
            worker_handle: worker.abort_handle(),
        },
    );
    state.agent_totals = TokenTotals {
        input_tokens: 100,
        output_tokens: 50,
        total_tokens: 150,
        seconds_running: 12.5,
    };
    build_snapshot(&state)
}

fn test_app(snapshot: OrchestratorSnapshot) -> axum::Router {
    let (tx, mut rx) = mpsc::channel(8);
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            match msg {
                OrchestratorMsg::SnapshotRequest { reply } => {
                    let _ = reply.send(snapshot.clone());
                }
                OrchestratorMsg::RefreshRequest { reply } => {
                    let _ = reply.send(());
                }
                _ => {}
            }
        }
    });

    build_router(tx)
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    serde_json::from_slice(&bytes).expect("valid json response")
}

#[tokio::test]
async fn get_state_returns_200_with_json_snapshot() {
    let app = test_app(make_snapshot());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/state")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["counts"]["running"], 1);
    assert_eq!(json["counts"]["retrying"], 0);
    assert_eq!(json["running"][0]["identifier"], "ISSUE-1");
    assert_eq!(json["codex_totals"]["total_tokens"], 150);
    assert!(json["generated_at"].is_string());
}

#[tokio::test]
async fn get_issue_unknown_identifier_returns_404_with_error_envelope() {
    let app = test_app(make_snapshot());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/unknown-123")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "issue_not_found");
    assert_eq!(json["error"]["message"], "issue 'unknown-123' not found");
}

#[tokio::test]
async fn post_refresh_returns_202() {
    let app = test_app(make_snapshot());

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/refresh")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let json = body_json(response).await;
    assert_eq!(json["queued"], true);
    assert!(json["requested_at"].is_string());
}

#[tokio::test]
async fn get_root_returns_200_html() {
    let app = test_app(make_snapshot());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .expect("content-type header");
    assert!(content_type.starts_with("text/html"));

    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let html = String::from_utf8(bytes.to_vec()).expect("utf8 html");
    assert!(html.contains("<h1>Rusty Dashboard</h1>"));
    assert!(html.contains("/api/v1/state"));
}

#[tokio::test]
async fn dashboard_html_escapes_dynamic_content_to_prevent_xss() {
    let app = test_app(make_snapshot());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let html = String::from_utf8(bytes.to_vec()).expect("utf8 html");

    // Dashboard must define an HTML-escape helper
    assert!(
        html.contains("function esc("),
        "dashboard must define an esc() function for HTML escaping"
    );

    // The escape function must handle angle brackets and ampersands
    assert!(
        html.contains("&amp;") && html.contains("&lt;") && html.contains("&gt;"),
        "esc() must replace &, <, > with HTML entities"
    );

    // All dynamic interpolations in table rows must go through esc()
    assert!(
        html.contains("${esc(r.identifier)}"),
        "identifier must be escaped"
    );
    assert!(
        html.contains("${esc(r.state)}"),
        "state must be escaped"
    );
    assert!(
        html.contains("${esc(event)}"),
        "event text must be escaped"
    );
    assert!(
        html.contains("${esc(session)}"),
        "session must be escaped"
    );
    assert!(
        html.contains("${esc(pid)}"),
        "pid must be escaped"
    );
}

#[tokio::test]
async fn fallback_returns_error_envelope_shape() {
    let app = test_app(make_snapshot());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/not-found")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    let json = body_json(response).await;
    assert_eq!(
        json,
        serde_json::json!({
            "error": {
                "code": "method_not_allowed",
                "message": "method not allowed"
            }
        })
    );
}
