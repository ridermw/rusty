use axum::{
    extract::{Path, State as AxumState},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, IntoResponse,
    },
    routing::{get, post},
    Json, Router,
};
use futures_util::stream::Stream;
use serde_json::json;
use std::convert::Infallible;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::orchestrator::{OrchestratorMsg, OrchestratorSnapshot};

#[derive(Clone)]
pub struct AppState {
    pub orchestrator_tx: mpsc::Sender<OrchestratorMsg>,
    pub sse_tx: broadcast::Sender<OrchestratorSnapshot>,
}

pub fn build_router(orchestrator_tx: mpsc::Sender<OrchestratorMsg>) -> Router {
    build_router_with_sse(orchestrator_tx, None)
}

pub fn build_router_with_sse(
    orchestrator_tx: mpsc::Sender<OrchestratorMsg>,
    sse_tx: Option<broadcast::Sender<OrchestratorSnapshot>>,
) -> Router {
    let sse_tx = sse_tx.unwrap_or_else(|| broadcast::channel(64).0);
    let state = AppState {
        orchestrator_tx,
        sse_tx,
    };
    Router::new()
        .route("/", get(dashboard_handler))
        .route("/api/v1/state", get(get_state))
        .route("/api/v1/events", get(get_events))
        .route("/api/v1/refresh", post(post_refresh))
        .route("/api/v1/:issue_identifier", get(get_issue))
        .fallback(fallback_handler)
        .with_state(state)
}

async fn get_state(AxumState(state): AxumState<AppState>) -> impl IntoResponse {
    match request_snapshot(&state.orchestrator_tx).await {
        Some(snapshot) => Json(snapshot_to_json(&snapshot)).into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": {"code": "unavailable", "message": "orchestrator not responding"}
            })),
        )
            .into_response(),
    }
}

async fn get_events(
    AxumState(state): AxumState<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.sse_tx.subscribe();

    let stream = async_stream::stream! {
        // Send initial snapshot so client has immediate state
        if let Some(snapshot) = request_snapshot(&state.orchestrator_tx).await {
            let json = snapshot_to_json(&snapshot);
            if let Ok(data) = serde_json::to_string(&json) {
                yield Ok(Event::default().event("snapshot").data(data));
            }
        }

        loop {
            match rx.recv().await {
                Ok(snapshot) => {
                    let json = snapshot_to_json(&snapshot);
                    if let Ok(data) = serde_json::to_string(&json) {
                        yield Ok(Event::default().event("snapshot").data(data));
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn snapshot_to_json(snapshot: &OrchestratorSnapshot) -> serde_json::Value {
    json!({
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "counts": {
            "running": snapshot.running_count,
            "retrying": snapshot.retrying_count
        },
        "running": snapshot.running,
        "retrying": snapshot.retrying,
        "codex_totals": snapshot.agent_totals,
        "rate_limits": null
    })
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
