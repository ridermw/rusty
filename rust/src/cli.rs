//! CLI entry point for Symphony.

use clap::Parser;
use std::path::PathBuf;
use tracing::{error, info};

#[derive(Parser, Debug)]
#[command(name = "symphony", about = "Symphony orchestration daemon")]
pub struct Args {
    /// Path to WORKFLOW.md file
    #[arg(default_value = "WORKFLOW.md")]
    pub workflow_path: PathBuf,

    /// HTTP server port (overrides server.port in WORKFLOW.md)
    #[arg(long)]
    pub port: Option<u16>,

    /// Log files directory
    #[arg(long)]
    pub logs_root: Option<PathBuf>,

    /// Required safety acknowledgement flag
    #[arg(long = "i-understand-that-this-will-be-running-without-the-usual-guardrails")]
    pub guardrails_acknowledged: bool,
}

pub async fn run() -> anyhow::Result<()> {
    let args = Args::parse();

    if !args.guardrails_acknowledged {
        anyhow::bail!(
            "Symphony requires the --i-understand-that-this-will-be-running-without-the-usual-guardrails flag.\n\
             This acknowledges that Symphony will run coding agents autonomously."
        );
    }

    let _log_guard = crate::logging::init_logging(args.logs_root.as_deref())
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    info!(workflow = %args.workflow_path.display(), port = ?args.port, "Symphony starting");

    if !args.workflow_path.exists() {
        anyhow::bail!("Workflow file not found: {}", args.workflow_path.display());
    }

    let workflow = crate::workflow::load_workflow(&args.workflow_path)?;

    let config: crate::config::schema::SymphonyConfig =
        serde_yaml::from_value(serde_yaml::to_value(&workflow.config)?)?;

    crate::config::validate_dispatch_config(&config)?;
    info!("Configuration validated");

    let (orch_tx, mut orch_rx) =
        tokio::sync::mpsc::channel::<crate::orchestrator::OrchestratorMsg>(256);

    // Spawn a minimal orchestrator message consumer that handles snapshot/refresh
    // requests. Without this, API calls would fill the channel buffer and hang.
    // The full orchestrator loop (with dispatch/reconciliation) will replace this
    // when wired end-to-end.
    tokio::spawn(async move {
        use crate::orchestrator::state::OrchestratorState;
        use crate::orchestrator::{build_snapshot, OrchestratorMsg};

        let state = OrchestratorState::new(30000, 10);
        while let Some(msg) = orch_rx.recv().await {
            match msg {
                OrchestratorMsg::SnapshotRequest { reply } => {
                    let _ = reply.send(build_snapshot(&state));
                }
                OrchestratorMsg::RefreshRequest { reply } => {
                    let _ = reply.send(());
                }
                _ => {}
            }
        }
    });

    if let Some(port) = args.port.or(config.server.port) {
        let tx = orch_tx.clone();
        // Use a oneshot to detect server bind failures before continuing
        let (server_ready_tx, server_ready_rx) =
            tokio::sync::oneshot::channel::<Result<(), String>>();
        tokio::spawn(async move {
            let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
            match tokio::net::TcpListener::bind(addr).await {
                Ok(listener) => {
                    let _ = server_ready_tx.send(Ok(()));
                    let app = crate::server::api::build_router(tx);
                    info!(%addr, "HTTP server listening");
                    if let Err(e) = axum::serve(listener, app).await {
                        error!(error = %e, "HTTP server failed");
                    }
                }
                Err(e) => {
                    let _ = server_ready_tx.send(Err(e.to_string()));
                }
            }
        });
        // Wait for bind result — fail fast if port is unavailable
        match server_ready_rx.await {
            Ok(Ok(())) => info!(port, "HTTP server started"),
            Ok(Err(e)) => anyhow::bail!("HTTP server failed to bind port {port}: {e}"),
            Err(_) => anyhow::bail!("HTTP server task died before binding"),
        }
    }

    info!("Symphony running. Press Ctrl+C to stop.");
    tokio::signal::ctrl_c().await?;
    info!("Shutdown signal received");

    Ok(())
}
