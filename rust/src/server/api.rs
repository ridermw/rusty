use axum::{
    extract::{Path, State as AxumState},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use crate::orchestrator::{OrchestratorMsg, OrchestratorSnapshot};

#[derive(Clone)]
pub struct AppState {
    pub orchestrator_tx: mpsc::Sender<OrchestratorMsg>,
}

pub fn build_router(orchestrator_tx: mpsc::Sender<OrchestratorMsg>) -> Router {
    let state = AppState { orchestrator_tx };
    Router::new()
        .route("/", get(dashboard_handler))
        .route("/api/v1/state", get(get_state))
        .route("/api/v1/refresh", post(post_refresh))
        .route("/api/v1/:issue_identifier", get(get_issue))
        .fallback(fallback_handler)
        .with_state(state)
}

async fn get_state(AxumState(state): AxumState<AppState>) -> impl IntoResponse {
    match request_snapshot(&state.orchestrator_tx).await {
        Some(snapshot) => Json(json!({
            "generated_at": chrono::Utc::now().to_rfc3339(),
            "counts": {
                "running": snapshot.running_count,
                "retrying": snapshot.retrying_count
            },
            "max_agents": snapshot.max_agents,
            "throughput_tps": snapshot.throughput_tps,
            "running": snapshot.running,
            "retrying": snapshot.retrying,
            "codex_totals": snapshot.agent_totals,
            "rate_limits": snapshot.rate_limits,
            "project_url": snapshot.project_url,
            "next_tick_at": snapshot.next_tick_at
        }))
        .into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": {"code": "unavailable", "message": "orchestrator not responding"}
            })),
        )
            .into_response(),
    }
}

async fn get_issue(
    AxumState(state): AxumState<AppState>,
    Path(identifier): Path<String>,
) -> impl IntoResponse {
    match request_snapshot(&state.orchestrator_tx).await {
        Some(snapshot) => {
            if let Some(running) = snapshot
                .running
                .iter()
                .find(|entry| entry.identifier == identifier)
            {
                return Json(json!({
                    "issue_identifier": running.identifier,
                    "status": "running",
                    "running": running,
                }))
                .into_response();
            }

            if let Some(retry) = snapshot
                .retrying
                .iter()
                .find(|entry| entry.identifier == identifier)
            {
                return Json(json!({
                    "issue_identifier": retry.identifier,
                    "status": "retrying",
                    "retry": retry,
                }))
                .into_response();
            }

            (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "error": {
                        "code": "issue_not_found",
                        "message": format!("issue '{}' not found", identifier)
                    }
                })),
            )
                .into_response()
        }
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": {"code": "unavailable", "message": "orchestrator not responding"}
            })),
        )
            .into_response(),
    }
}

async fn post_refresh(AxumState(state): AxumState<AppState>) -> impl IntoResponse {
    let (tx, _rx) = oneshot::channel();
    let _ = state
        .orchestrator_tx
        .send(OrchestratorMsg::RefreshRequest { reply: tx })
        .await;
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "queued": true,
            "requested_at": chrono::Utc::now().to_rfc3339()
        })),
    )
}

async fn dashboard_handler() -> impl IntoResponse {
    Html(super::dashboard::render_html_dashboard())
}

async fn fallback_handler() -> impl IntoResponse {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(json!({
            "error": {"code": "method_not_allowed", "message": "method not allowed"}
        })),
    )
}

async fn request_snapshot(tx: &mpsc::Sender<OrchestratorMsg>) -> Option<OrchestratorSnapshot> {
    let (reply_tx, reply_rx) = oneshot::channel();
    tx.send(OrchestratorMsg::SnapshotRequest { reply: reply_tx })
        .await
        .ok()?;
    tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx)
        .await
        .ok()?
        .ok()
}
