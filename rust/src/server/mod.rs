pub mod api;
pub mod dashboard;

use axum::Router;
use std::net::SocketAddr;
use tokio::sync::{broadcast, mpsc};
use tracing::info;

use crate::orchestrator::OrchestratorSnapshot;

/// Start the HTTP server on the given port.
pub async fn start_server(
    port: u16,
    snapshot_tx: mpsc::Sender<crate::orchestrator::OrchestratorMsg>,
) -> Result<(), Box<dyn std::error::Error>> {
    start_server_with_sse(port, snapshot_tx, None).await
}

/// Start the HTTP server with an optional SSE broadcast channel.
pub async fn start_server_with_sse(
    port: u16,
    snapshot_tx: mpsc::Sender<crate::orchestrator::OrchestratorMsg>,
    sse_tx: Option<broadcast::Sender<OrchestratorSnapshot>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let app: Router = api::build_router_with_sse(snapshot_tx, sse_tx);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    info!(%addr, "starting HTTP server");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
