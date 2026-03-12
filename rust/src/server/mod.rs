pub mod api;
pub mod dashboard;

use axum::Router;
use std::net::SocketAddr;
use tokio::sync::mpsc;
use tracing::info;

/// Start the HTTP server on the given port.
pub async fn start_server(
    port: u16,
    snapshot_tx: mpsc::Sender<crate::orchestrator::OrchestratorMsg>,
) -> Result<(), Box<dyn std::error::Error>> {
    let app: Router = api::build_router(snapshot_tx);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    info!(%addr, "starting HTTP server");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
